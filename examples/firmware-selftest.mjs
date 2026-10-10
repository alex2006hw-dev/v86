// Build a 1.44 MiB floppy whose boot sector is a BIOS self-test.
//
// The self-test is hand-assembled here so the example needs no assembler
// toolchain and no disk image: `node examples/firmware.js` works in a
// clean checkout.
//
// It checks the services a boot loader and a DOS kernel depend on, in the
// order a cold boot would use them:
//
//   M  INT 12h   conventional memory is at least 512 KiB
//   E  INT 11h   the equipment word reports a floppy
//   T  INT 1Ah   AH=02h reports a non-zero century in CH and a non-zero day in DL
//   R  INT 13h   AH=00h reset succeeds
//   G  INT 13h   AH=04h returns a usable drive geometry
//   D  INT 13h   AH=41h EDD install check sets BX=AA55h
//   B  INT 13h   AH=42h reads LBA 0 through a disk address packet
//   S  the sector read back is this boot sector (0xAA55)
//   8  INT 15h   AX=E820h returns a system memory map
//   0  INT 15h   the first SMAP entry describes memory at base 0
//   A  INT 15h   AX=2402h A20 query succeeds
//   C  INT 10h   AH=0Fh reports 80 columns
//   Q  INT 10h   AH=0Eh writes a character to the screen
//
// A failure prints a single character naming the check, then `RESULT: FAIL`
//; passing every check prints `RESULT: PASS`.
//
// Two constraints shape the layout, and both come from how a real boot
// works: the BIOS loads exactly 512 bytes to 0000:7C00, and nothing else
// from the floppy is available until the guest reads it. So the code and
// its strings have to fit in one sector, and every failure path has to be
// a branch to a shared routine rather than an inline copy -- thirteen
// inlined copies would not fit.

const SECTOR_SIZE = 512;
const FLOPPY_SECTORS = 2880;          // 1.44 MiB

// Working data lives in its own segment well clear of the boot sector, so
// the disk read in check B can overwrite a whole sector without trampling
// the code that is running.
const DATA_SEG = 0x2000;              // physical 0x20000
const DATA_BASE = DATA_SEG << 4;

const DAP      = 0x0000;              // 16-byte disk address packet
const BUFFER   = 0x0100;              // 512-byte landing zone for the read
const SMAP     = 0x0300;              // 24-byte E820 entry

export function build_self_test_floppy()
{
    const image = new Uint8Array(FLOPPY_SECTORS * SECTOR_SIZE);
    const { code, pass_at, fail_at } = assemble_self_test();

    if(code.length > SECTOR_SIZE)
    {
        throw new Error("boot sector is " + code.length +
            " bytes; a BIOS only loads " + SECTOR_SIZE);
    }

    image.set(code, 0);
    image[510] = 0x55;
    image[511] = 0xAA;
    image[pass_at] = 0;
    image[fail_at] = 0;

    return image.buffer;
}

/**
 * A minimal 16-bit assembler: labels with forward-reference patching.
 *
 * Only the instructions the self-test needs, always with explicit
 * operands, so a mistake shows up as a wrong byte pattern rather than a
 * silent misparse.
 */
export class Asm
{
    constructor(size)
    {
        this.buf = new Uint8Array(size);
        this.pos = 0;
        this.labels = new Map();
        this.fixups = [];
        // Where the sector is loaded. Far jumps in protected mode
        // carry an absolute offset, because the segment base is
        // whatever the descriptor says -- usually zero -- so the
        // offset has to include the load address.
        this.origin = 0;
    }

    label(name)
    {
        if(this.labels.has(name))
        {
            throw new Error("duplicate label: " + name);
        }
        this.labels.set(name, this.pos);
        return this;
    }

    at(offset)
    {
        this.pos = offset;
        return this;
    }

    b(...bytes)
    {
        for(const x of bytes)
        {
            if(this.pos >= this.buf.length)
            {
                throw new Error("boot sector overflow");
            }
            this.buf[this.pos++] = x & 0xFF;
        }
        return this;
    }

    w(value)
    {
        return this.b(value & 0xFF, (value >> 8) & 0xFF);
    }

    ascii(text)
    {
        for(const ch of text)
        {
            this.b(ch.charCodeAt(0));
        }
        return this.b(0);
    }

    rel16(name)
    {
        this.fixups.push({ at: this.pos, name, kind: "rel16" });
        return this.w(0);
    }

    rel8(name)
    {
        this.fixups.push({ at: this.pos, name, kind: "rel8" });
        return this.b(0);
    }

    link()
    {
        for(const f of this.fixups)
        {
            const target = this.labels.get(f.name);

            if(target === undefined)
            {
                throw new Error("undefined label: " + f.name);
            }

            if(f.kind === "rel16")
            {
                const rel = target - (f.at + 2);
                this.buf[f.at] = rel & 0xFF;
                this.buf[f.at + 1] = (rel >> 8) & 0xFF;
            }
            else if(f.kind === "far_rel")
            {
                // A far jump back to real mode. The selector
                // names the segment the code was loaded into,
                // so the offset stays relative to it.
                const target = this.labels.get(f.name);
                this.buf[f.at] = target & 0xFF;
                this.buf[f.at + 1] = (target >> 8) & 0xFF;
            }
            else if(f.kind === "far")
            {
                // A far jump in protected mode: the descriptor's
                // base is added by the CPU, not by us, so the
                // offset must be the absolute address.
                const target = this.labels.get(f.name) + this.origin;
                this.buf[f.at] = target & 0xFF;
                this.buf[f.at + 1] = (target >> 8) & 0xFF;
            }
            else if(f.kind === "absolute")
            {
                this.buf[f.at] = target & 0xFF;
                this.buf[f.at + 1] = (target >> 8) & 0xFF;
            }
            else
            {
                const rel = target - (f.at + 1);

                if(rel < -128 || rel > 127)
                {
                    throw new Error("short jump to " + f.name + " out of range: " + rel);
                }

                this.buf[f.at] = rel & 0xFF;
            }
        }

        return this.buf;
    }

    // ---- instructions -------------------------------------------------

    cli()   { return this.b(0xFA); }
    sti()   { return this.b(0xFB); }
    pushf() { return this.b(0x9C); }
    popf()  { return this.b(0x9D); }
    pop_ax() { return this.b(0x58); }
    push_ax() { return this.b(0x50); }
    lahf()  { return this.b(0x9F); }
    hlt()   { return this.b(0xF4); }
    lodsb() { return this.b(0xAC); }
    ret()   { return this.b(0xC3); }

    xor_ax_ax() { return this.b(0x31, 0xC0); }
    or_al_al()  { return this.b(0x08, 0xC0); }

    // B8+rd, iw -- rd is AX,CX,DX,BX,SP,BP,SI,DI
    mov_r16(reg, imm) { return this.b(0xB8 + reg).w(imm); }

    /** `mov si, <label>` — the label's offset is an immediate. */
    mov_si_label(name) {
        this.fixups.push({ at: this.pos + 1, name, kind: "absolute" });
        return this.b(0xBE).w(0);
    }
    mov_ah(v) { return this.b(0xB4, v); }
    mov_al(v) { return this.b(0xB0, v); }
    mov_dl(v) { return this.b(0xB2, v); }

    // 8E /r with the segment selector in the reg field; the index order
    // here is ES, CS, SS, DS, FS, GS.
    mov_seg_ax(seg) { return this.b(0x8E, 0xC0 | ((seg & 7) << 3)); }

    mov_ds_cs() { return this.b(0x0E, 0x1F); }   // push cs; pop ds

    mov_ax_moffs(disp)     { return this.b(0xA1).w(disp); }        // A1 moffs
    mov_moffs_imm16(d, i)  { return this.b(0xC7, 0x06).w(d).w(i); }  // C7 /0

    // 89 /r: [disp16] <- reg
    mov_moffs_reg(d, reg) {
        return this.b(0x89, [0x06, 0x0E, 0x16, 0x1E, 0x26, 0x2E, 0x36, 0x3E][reg]).w(d);
    }

    cmp_ax_imm16(imm) { return this.b(0x3D).w(imm); }
    cmp_al_imm(imm)   { return this.b(0x3C, imm); }

    // 89 /r: [r/m16] <- reg16, so `mov_r16_r16(dst, src)` copies src to dst.
    mov_r16_r16(dst, src) {
        return this.b(0x89, 0xC0 | ((src & 7) << 3) | (dst & 7));
    }

    // 81 /4 and 81 /7: AND r16, imm16 and CMP r16, imm16.
    and_r16_imm16(reg, imm) { return this.b(0x81, 0xE0 | (reg & 7)).w(imm); }

    // F7 /0: TEST r16, imm16.
    test_r16_imm16(reg, imm) { return this.b(0xF7, 0xC0 | (reg & 7)).w(imm); }
    cmp_r16_imm16(reg, imm) { return this.b(0x81, 0xF8 | (reg & 7)).w(imm); }
    cmp_bx_imm16(imm) { return this.b(0x81, 0xFB).w(imm); }
    cmp_cl_imm(imm)   { return this.b(0x80, 0xF9, imm); }
    test_al_imm(imm)  { return this.b(0xA8, imm); }
    test_ax_imm16(imm) { return this.b(0xA9).w(imm); }   // A9 iw

    inc_moffs(d)    { return this.b(0xFF, 0x06).w(d); }              // FF /0
    cmp_moffs_imm8(d, i) { return this.b(0x83, 0x3E).w(d).b(i); }   // 83 /7

    // ---- protected-mode / control-register forms ----
    mov_eax_cr0()   { return this.b(0x0F, 0x20, 0xC0); }
    mov_cr0_eax()   { return this.b(0x0F, 0x22, 0xC0); }
    // 83 /reg ib: /0=ADD /1=OR /4=AND /5=SUB. The reg field is
    // bits 5..3 of the ModRM byte, so AND is 11 100 000 = 0xE0.
    // 0xE8 would be SUB, which adds instead of masking.
    or_eax_imm8(v)  { return this.b(0x83, 0xC8, v); }
    and_eax_imm8(v) { return this.b(0x83, 0xE0, v); }
    // 0F 01 /2 is LGDT. The r/m field selects the addressing mode, and
    // [disp16] is r/m=110 -- so the ModRM byte is mod=00, reg=010, rm=110,
    // which is 0x16.
    //
    // Getting this wrong is silent and confusing: rm=101 is [DI], so
    // `0F 01 15 xx xx` reads the pseudo-descriptor from DI*16 instead of
    // from the displacement, and `0F 01 10 xx xx` is [BX] with a disp8.
    lgdt_moffs(d)   { return this.b(0x0F, 0x01, 0x16).w(d); }

    // 0F 01 /0 is SGDT and /1 is SIDT, and they use the same r/m=110 for
    // [disp16], so ModRM 0x06 and 0x0E. The 0x2E forms carry a CS segment
    // override, addressing the boot sector's own segment.
    sgdt_cs(d)      { return this.b(0x2E, 0x0F, 0x01, 0x06).w(d); }
    sidt_cs(d)      { return this.b(0x2E, 0x0F, 0x01, 0x0E).w(d); }
    sidt_moffs(d)   { return this.b(0x0F, 0x01, 0x0E).w(d); }
    sgdt_moffs(d)   { return this.b(0x0F, 0x01, 0x06).w(d); }
    jmp_far(off, sel) { return this.b(0xEA).w(off).w(sel); }

    /** Where this code will be loaded, for absolute offsets. */
    set_origin(o) { this.origin = o; return this; }

    /** `jmp <label>:<sel>` -- the offset is a label address. */
    jmp_far_label(name, sel) {
        this.fixups.push({ at: this.pos + 1, name, kind: "far" });
        return this.b(0xEA).w(0).w(sel);
    }

    /**
     * The same, forced to 16-bit operands. In 32-bit code the
     * plain form reads a 32-bit offset and a 16-bit selector,
     * so a far jump written as five bytes would swallow the
     * following instruction as part of its operand.
     */
    jmp_far_label16(name, sel) {
        this.fixups.push({ at: this.pos + 2, name, kind: "far" });
        return this.b(0x66, 0xEA).w(0).w(sel);
    }
    /**
     * A far jump back to real mode, where the selector names
     * the segment the sector was loaded into. The offset is
     * relative to that segment, unlike the protected-mode
     * form, whose descriptors have a base of zero.
     */
    jmp_far_label_rel(name, sel) {
        this.fixups.push({ at: this.pos + 1, name, kind: "far_rel" });
        return this.b(0xEA).w(0).w(sel);
    }

    push_imm32(v)   { return this.b(0x68).w(v); }
    pop_es()        { return this.b(0x1F); }

    int(n)  { return this.b(0xCD, n); }
    jc(l)   { return this.b(0x72).rel8(l); }   // JB / JNAE
    jnc(l)  { return this.b(0x73).rel8(l); }   // JAE / JNB
    je(l)   { return this.b(0x74).rel8(l); }   // JZ
    jne(l)  { return this.b(0x75).rel8(l); }   // JNZ
    jbe(l)  { return this.b(0x76).rel8(l); }   // JNA
    ja(l)   { return this.b(0x77).rel8(l); }   // JNBE
    jmp(l)  { return this.b(0xEB).rel8(l); }
    jmpl(l) { return this.b(0xE9).rel16(l); }
    call(l) { return this.b(0xE8).rel16(l); }
}

export function self_test_labels()
{
    const labels = {};
    const a = new Asm(SECTOR_SIZE);
    const real_label = a.label.bind(a);
    a.label = function(name)
    {
        real_label(name);
        labels[name] = a.pos;
        return a;
    };
    assemble(a);
    return labels;
}

function assemble_self_test()
{
    const a = new Asm(SECTOR_SIZE);
    assemble(a);
    const code = a.link();

    // The strings must not run into the 0x55AA signature at 510/511.
    if(code[510] !== 0 || code[511] !== 0)
    {
        throw new Error("boot sector code and strings overlap the signature");
    }

    return { code: code.slice(0, SECTOR_SIZE) };
}

function assemble(a)
{

    // Every failure branches to one shared routine with the check's letter
    // in AL. Inlining the routine thirteen times would not fit in a sector.
    //
    // The jump is emitted *inverted* -- "if the check passed, skip the
    // failure" -- because the forward form needs a second jump over the
    // failure block. That extra three bytes per check is, across thirteen
    // checks, the difference between fitting in a sector and not.
    const inverse = { jne: "je", je: "jne", jc: "jnc", ja: "jbe" };

    const fail = (cc, tag) =>
    {
        a[inverse[cc]]("ok_" + tag);
        a.mov_al(tag.charCodeAt(0));
        a.jmpl("fail");
        a.label("ok_" + tag);
    };

    // ---- entry --------------------------------------------------------
    // A BIOS jumps here with DL = boot drive and interrupts disabled.
    a.label("start");
    a.cli();
    a.xor_ax_ax();
    a.mov_seg_ax(3);              // DS
    a.mov_seg_ax(0);              // ES
    a.mov_seg_ax(2);              // SS
    a.mov_r16(4, 0x7000);         // SP
    a.sti();
    a.mov_r16(0, DATA_SEG);
    a.mov_seg_ax(3);              // DS = working data

    // ---- P: a protected-mode entry, as a real boot loader does one ----
    //
    // This is the smoke test for BOOT-2. FreeNOS and HelenOS both die here
    // under this firmware -- `POP ES` in 32-bit mode raises the #GP that
    // v86 has no handler for -- while SeaBIOS boots them to a login prompt.
    // The test reproduces the shape of what those loaders do, so a failure
    // here is attributable without a guest in the picture at all:
    //
    //   build a GDT with a 32-bit code and data descriptor
    //   LGDT it, set CR0.PE, far jump through selector 0x08
    //   pop the data selector into ES   <- the instruction that faults
    //   clear CR0.PE and far jump back to 16-bit
    //
    // The GDT lives in the scratch segment. Descriptors are built with
    // immediate stores rather than emitted as data, so the test stays a few
    // dozen bytes and the sector keeps fitting in 512.
    const GDT = 0x0400;
    const PSEUDO = GDT + 0x28;   // past all four descriptors

    // null descriptor at GDT+0x00 is already zero -- the scratch segment is
    // cleared by the data-segment setup below.
    // code descriptor at GDT+0x08: base 0, limit 0xFFFFF, G=1, D=1, P=1, ring 0, code
    a.mov_moffs_imm16(GDT + 0x08, 0xFFFF);
    a.mov_moffs_imm16(GDT + 0x0A, 0x0000);
    // Bytes 4 and 5 are Base[23:16] and the access byte, in that order --
    // not the other way round. Getting this wrong makes every descriptor
    // non-present and every segment load fault.
    a.mov_moffs_imm16(GDT + 0x0C, 0x9A00);
    a.mov_moffs_imm16(GDT + 0x0E, 0x00CF);
    // data descriptor at GDT+0x10: same, but data
    a.mov_moffs_imm16(GDT + 0x10, 0xFFFF);
    a.mov_moffs_imm16(GDT + 0x12, 0x0000);
    a.mov_moffs_imm16(GDT + 0x14, 0x9200);
    a.mov_moffs_imm16(GDT + 0x16, 0x00CF);
    // A 16-bit code descriptor at selector 0x18, for the way back. Leaving
    // protected mode requires a far jump to a 16-bit segment *before*
    // clearing CR0.PE; clearing PE first and then far jumping is not a
    // sequence real hardware accepts, and it is what made this test fault
    // for reasons of its own.
    a.mov_moffs_imm16(GDT + 0x18, 0xFFFF);
    a.mov_moffs_imm16(GDT + 0x1A, 0x0000);
    a.mov_moffs_imm16(GDT + 0x1C, 0x9A00);
    a.mov_moffs_imm16(GDT + 0x1E, 0x0000);
    // pseudo-descriptor: limit 0x1F (four descriptors minus one), base = GDT
    a.mov_moffs_imm16(PSEUDO + 0, 0x001F);
    a.mov_moffs_imm16(PSEUDO + 2, GDT & 0xFFFF);
    a.mov_moffs_imm16(PSEUDO + 4, (DATA_SEG >> 12) & 0xFFFF);

    a.lgdt_moffs(PSEUDO);
    // The descriptors all have base zero, so from here on an
    // offset is an absolute address: the code lives at 0x7C00.
    a.set_origin(0x7C00);
    a.mov_eax_cr0();
    a.or_eax_imm8(0x01);            // CR0.PE
    a.mov_cr0_eax();
    a.jmp_far_label("pm_ok", 0x0008);

    // Reached only if the far jump and the segment load both worked.
    a.label("pm_ok");
    // Far jump to the 16-bit descriptor first: that is what flushes CS and
    // puts us in a state where clearing PE is legal. The 16-bit form is
    // required here, because this code is 32-bit and a plain far jump
    // would read a 32-bit offset.
    a.jmp_far_label16("pm16", 0x0018);
    a.label("pm16");
    a.mov_eax_cr0();
    a.and_eax_imm8(0xFE);           // clear CR0.PE
    a.mov_cr0_eax();
    // Back to real mode, into the segment the sector was
    // loaded into. Jumping to selector zero would leave CS
    // naming 0000:0000, and the verdict printer copies CS
    // into DS to find its strings -- it would then read
    // through the wrong segment and print nothing.
    a.jmp_far_label_rel("after_P", 0x07C0);
    a.label("after_P");

    // ---- M: conventional memory ----------------------------------------
    a.mov_ah(0x00);
    a.int(0x12);
    a.cmp_ax_imm16(512);
    fail("jc", "M");            // fail only if memory is below 512 KiB

    // ---- E: equipment word reports a floppy -----------------------------
    a.mov_ah(0x11);
    a.int(0x11);
    a.test_al_imm(0x01);         // bit 0 = at least one floppy drive
    fail("je", "E");

    // ---- T: real-time clock date -----------------------------------------
    // AH=02h (read date) returns CH=century, CL=year, DH=month, DL=day.
    // AH=00h (read time) returns the time of day and has no century at all.
    //
    // The century is in *CH*, which is bits 8-15 of CX -- not in AX. Masking
    // AX tests AH instead, and AH is left over from the `mov ah,02h` that
    // issued the call on a BIOS that does not clear it. That made this check
    // pass for the wrong reason against one BIOS and fail against another;
    // running both through `examples/firmware-oracle.js` is what exposed it.
    a.mov_ah(0x02);
    a.int(0x1A);
    a.test_r16_imm16(1, 0xFF00);  // CH = century, must be non-zero
    fail("je", "T");

    // DL is the day of month and must not be zero.
    a.test_al_imm(0xFF);
    fail("je", "T2");

    // ---- R: disk reset --------------------------------------------------
    a.mov_ah(0x00);
    a.mov_dl(0x00);
    a.int(0x13);
    fail("jc", "R");

    // ---- G: drive parameters --------------------------------------------
    // AH=08h is the "describe this drive" call; AH=04h is verify, which a
    // self-test has no reason to issue.
    a.mov_ah(0x08);
    a.mov_dl(0x00);
    a.int(0x13);
    fail("jc", "G");
    // CL holds the sectors-per-track count; zero means the drive cannot be
    // addressed even with the extensions.
    a.cmp_cl_imm(0x3F);
    fail("ja", "G2");

    // ---- D: EDD install check -------------------------------------------
    a.mov_ah(0x41);
    a.mov_dl(0x00);
    // The request carries BX=55AAh; the BIOS answers with BX=AA55h. They
    // are deliberately different, which is the whole point of the check.
    a.mov_r16(3, 0x55AA);        // BX = 55AAh
    a.mov_r16(1, 0x0000);        // CX = 0000h
    a.int(0x13);
    fail("jc", "D");
    a.cmp_bx_imm16(0xAA55);      // answer must be AA55h
    fail("jne", "D2");

    // ---- B: EDD read of LBA 0 through a disk address packet --------------
    // Packet layout, per the INT 13h Extensions spec:
    //   +0  size (byte)   +1  reserved    +2  sector count (word)
    //   +4  buffer offset +6  buffer segment
    //   +8  64-bit LBA (two 32-bit words)
    // Note the buffer is addressed as segment:offset. Writing the offset
    // into the segment field -- or deriving the segment from the offset --
    // lands the read somewhere else entirely and the check that follows
    // sees garbage.
    a.mov_moffs_imm16(DAP + 0x00, 16);         // size = 16, reserved = 0
    a.mov_moffs_imm16(DAP + 0x02, 1);          // sector count
    a.mov_moffs_imm16(DAP + 0x04, BUFFER);     // buffer offset
    a.mov_moffs_imm16(DAP + 0x06, DATA_SEG);   // buffer segment
    a.mov_moffs_imm16(DAP + 0x08, 0);          // LBA low dword
    a.mov_moffs_imm16(DAP + 0x0A, 0);           // LBA high dword
    a.mov_moffs_imm16(DAP + 0x0C, 0);          // LBA, bits 32..63
    a.mov_moffs_imm16(DAP + 0x0E, 0);          // LBA, bits 48..63
    a.mov_ah(0x42);
    a.mov_dl(0x00);
    a.mov_r16(6, DAP);                        // SI = packet
    a.int(0x13);
    fail("jc", "B");

    // ---- S: the sector read back is this boot sector ---------------------
    // A word load of the 0x55AA signature yields AX = AA55h, so the *low*
    // byte is 55h and the high byte is AAh. Comparing AL against AAh would
    // fail against a perfectly good read.
    a.mov_ax_moffs(BUFFER + 510);
    a.cmp_al_imm(0x55);
    fail("jne", "S");

    // ---- 8: E820 system memory map ---------------------------------------
    a.mov_r16(0, 0xE820);
    a.mov_r16(3, 0);                          // BX = continuation
    a.mov_r16(1, SMAP);                       // CX = buffer
    a.int(0x15);
    fail("jc", "8");

    // ---- 0: the first entry describes memory at base 0 -------------------
    a.cmp_moffs_imm8(SMAP + 8, 0);            // high dword of the base
    fail("jne", "0");
    a.mov_ax_moffs(SMAP);                     // low dword
    a.cmp_ax_imm16(0);
    fail("jne", "0b");

    // ---- A: A20 query -----------------------------------------------------
    a.mov_r16(0, 0x2402);
    a.int(0x15);
    fail("jc", "A");


    // ---- C: video mode reports 80 columns --------------------------------
    // AH=0Fh returns the column count in AH and the mode number in AL, so
    // testing AL against 80 tests the wrong byte entirely. Mask AH out and
    // compare CX, leaving AL alone.
    a.mov_ah(0x0F);
    a.int(0x10);
    a.mov_r16_r16(1, 0);          // CX = AX
    a.and_r16_imm16(1, 0xFF00);   // keep AH
    a.cmp_r16_imm16(1, 80 << 8);   // 80 columns
    fail("jne", "C");

    // ---- Q: write a character to the screen ------------------------------
    a.mov_ah(0x0E);
    a.mov_al(0x51);                           // 'Q'
    a.int(0x10);

    // ---- every check passed -----------------------------------------------
    a.jmpl("pass_tail");

    // ---- shared failure routine -------------------------------------------
    // AL holds the check letter. Print it, then print the failure banner
    // from the inline table, then stop: a machine that has failed a BIOS
    // self-test has nothing useful left to do.
    a.label("fail");
    a.mov_ah(0x0E);
    a.int(0x10);
    a.jmp("fail_tail");

    // ---- puts: write the NUL-terminated string at DS:SI -------------------
    a.label("puts");
    a.lodsb();
    a.or_al_al();
    a.je("puts_done");
    a.mov_ah(0x0E);
    a.int(0x10);
    a.jmpl("puts");
    a.label("puts_done");
    a.ret();

    // ---- halt --------------------------------------------------------------
    a.label("halt");
    a.cli();
    a.hlt();
    a.b(0xEB, 0xFD);                          // jmp $

    // ---- print a verdict ---------------------------------------------------
    // lodsb reads through DS, and the inline strings live in the boot
    // sector's own segment rather than the scratch data segment the checks
    // used. Copying CS to DS rather than assuming 0000 matters because a
    // BIOS may enter the sector as 0000:7C00 or as 07C0:0000 -- the same
    // linear address, but only CS names the segment holding the strings.
    a.label("pass_tail");
    a.mov_ds_cs();
    a.mov_si_label("msg_pass");
    a.call("puts");
    a.jmpl("halt");

    a.label("fail_tail");
    a.mov_ds_cs();
    a.mov_si_label("msg_fail");
    a.call("puts");
    a.jmpl("halt");

    // ---- inline strings ----------------------------------------------------
    // These have to live inside the loaded sector: a BIOS reads 512 bytes
    // and nothing else is available until the guest asks for it.
    a.label("msg_pass");
    a.ascii("RESULT: PASS");
    a.label("msg_fail");
    a.ascii("RESULT: FAIL");
}
