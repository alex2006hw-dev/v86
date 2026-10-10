// Assembles a 512-byte boot sector that self-tests the PCjs-derived BIOS.
//
// Hand-assembled (no assembler dependency). Two passes: emit code with
// symbolic fixups, then place the message strings and resolve.
//
// The sector runs from 0000:7C00 and exercises INT 10h, 11h, 12h, 13h (CHS
// plus EDD), 15h (E820 plus the A20 gate), 16h and 1Ah. Each passing check
// bumps a counter at 0x0050; each failure bumps 0x0051 and prints '!'.
// The verdict line reads "RESULT: PASS" or "RESULT: FAIL".
//
// Exports: buildBootSector() -> { bytes, passAddr, failAddr, codeSize }

// The boot sector itself is only 512 bytes, and the code fills nearly all of
// it, so the message strings live in low memory instead (as an ordinary
// program's data segment would). The harness writes them in after POST.
const STR_BANNER = 0x0600;
const STR_VERDICT = 0x0620;

// ------------------------------------------------------------------ state

let buf = new Uint8Array(512);
let org = 0;
const labels = new Map();       // name -> offset
const fixups = [];              // { at, size, name, rel?, str? }

function L(name) { labels.set(name, org); }
function db(...vals) { for (const v of vals) buf[org++] = v & 0xFF; }
function dw(v) { buf[org++] = v & 0xFF; buf[org++] = (v >> 8) & 0xFF; }
function emit(...b) { db(...b); }

function rel8(name) { fixups.push({ at: org, size: 1, name }); db(0); }
function rel16(name) { fixups.push({ at: org, size: 2, name, rel: true }); dw(0); }

function resolve() {
    for (const f of fixups) {
        const target = labels.get(f.name);
        if (target === undefined) throw new Error("unresolved label: " + f.name);
        if (f.size === 1) {
            const d = target - (f.at + 1);
            if (d < -128 || d > 127) throw new Error("rel8 out of range for " + f.name);
            buf[f.at] = d & 0xFF;
        } else {
            const t = f.rel ? (target - (f.at + 2)) & 0xFFFF : target;
            buf[f.at] = t & 0xFF;
            buf[f.at + 1] = (t >> 8) & 0xFF;
        }
    }
}

// ------------------------------------------------------------- primitives

// Conditional jumps: the opcode and the fixup must always go together,
// so these helpers own both.
const JB = 0x72, JNB = 0x73, JZ = 0x74, JNE = 0x75, JMP8 = 0xEB;
function jb(label) { emit(JB); rel8(label); }
function jz(label) { emit(JZ); rel8(label); }
function jne(label) { emit(JNE); rel8(label); }
function jmp8(label) { emit(JMP8); rel8(label); }

/**
 * INT 10h AH=0Eh: teletype a character. Accepts either a numeric byte or a
 * one-character string (check identifiers are easier to read as 'M' than 0x4D).
 */
function tty(ch) {
    const code = typeof ch === "string" ? ch.charCodeAt(0) : ch;
    emit(0xB4, 0x0E);           // mov ah,0x0E
    emit(0xB0, code & 0xFF);    // mov al,ch
    emit(0xB7, 0x00);           // mov bh,0
    emit(0xCD, 0x10);           // int 0x10
}

// Only failures are counted: each check jumps past its failure branch when it
// passes, so the pass path needs no code at all. (Counting passes as well would
// require an extra jump per check to keep the two paths from running into each
// other, and the 512-byte sector is nearly full.)
let CHECKS = 0;

/**
 * Record a failure. `id` is a single character naming the check, so the screen
 * shows exactly which service misbehaved instead of a row of '!'.
 */
function fail(id) {
    tty(id);
    emit(0xFE, 0x06, 0x51, 0x00);          // inc byte [0x0051]
}

/**
 * A check is laid out as: <test>, j<cc> pass_label, fail(), pass_label:
 * `fail` returns, so the pass path simply falls through.
 */
/**
 * A check is "<test>, j<pass condition> label, fail(id), label:", so the jump
 * must encode the PASSING case -- not the failing one.
 */
function check(cc, label, id) {
    CHECKS++;
    emit(cc); rel8(label);
    fail(id);
    L(label);
}

/** Print the NUL-terminated string at the absolute address `addr` (DS=0). */
function puts(addr) {
    emit(0xBE); dw(addr);          // mov si,addr
    emit(0xE8); rel16("@@puts");   // call @@puts
}

// ---------------------------------------------------------------- program

export function buildBootSector() {
    buf = new Uint8Array(512);
    org = 0;
    CHECKS = 0;
    labels.clear();
    fixups.length = 0;

    // ================= code (offset 0 is the entry point) =================
    L("start");
    puts(STR_BANNER);

    // --- INT 12h: conventional memory must be at least 512 KiB ---
    emit(0x31, 0xC0);                 // xor ax,ax
    emit(0xCD, 0x12);                 // int 0x12
    emit(0x3D, 0x00, 0x02);           // cmp ax,0x0200
    check(JNB, "k_mem_ok", 'M');

    // --- INT 11h: equipment word must report a floppy (bit 0) ---
    emit(0x31, 0xC0);                 // xor ax,ax
    emit(0xCD, 0x11);                 // int 0x11
    emit(0xA8, 0x01);                 // test ax,1
    check(JNE, "k_equip_ok", 'E');          // pass when the floppy bit is set

    // --- INT 10h AH=0Fh: text mode must report 80 columns ---
    emit(0xB4, 0x0F);                 // mov ah,0x0F
    emit(0xCD, 0x10);                 // int 0x10
    // AH holds the column count and AL the mode. There is no "cmp ah,imm8",
    // so route AH through BL.
    emit(0x88, 0xE3);                 // mov bl,ah   (reg=100=AH, rm=011=BL)
    emit(0x80, 0xFB, 0x50);           // cmp bl,80
    check(JZ, "k_cols_ok", 'C');

    // --- INT 13h AH=00h: reset floppy 0, CF must come back clear ---
    emit(0xB4, 0x00);                 // mov ah,0x00
    emit(0xB2, 0x00);                 // mov dl,0
    emit(0x31, 0xC9);                 // xor cx,cx
    emit(0xCD, 0x13);                 // int 0x13
    jb("k_reset_cf");                 // CF set -> failure
    jmp8("k_reset_done");
    fail('R');
    L("k_reset_cf");
    L("k_reset_done");

    // --- INT 13h AH=41h: EDD install check -> BX = AA55h, CF clear ---
    emit(0xBB, 0xAA, 0x55);           // mov bx,0x55AA
    emit(0xB4, 0x41);                 // mov ah,0x41
    emit(0xB2, 0x00);                 // mov dl,0
    emit(0xCD, 0x13);                 // int 0x13
    jb("k_edd_bad");
    emit(0x81, 0xFB, 0x55, 0xAA);     // cmp bx,0xAA55
    jne("k_edd_bad");
    jmp8("k_edd_ok");
    L("k_edd_bad");
    fail('D');
    L("k_edd_ok");

    // --- INT 13h AH=08h: drive parameters, 1.44M has 80 cylinders ---
    emit(0xB4, 0x08);                 // mov ah,0x08
    emit(0xB2, 0x00);                 // mov dl,0
    emit(0x31, 0xC9);                 // xor cx,cx
    emit(0xCD, 0x13);                 // int 0x13
    jb("k_cyl_cf");
    // AH=08h returns the MAXIMUM cylinder number (cylinders - 1) in CH, the
    // top two bits of it in CL 7:6, the sectors-per-track in CL 5:0, the
    // maximum head number in DH and the drive count in DL. BL carries the
    // media type, not geometry.
    emit(0x88, 0xEB);                 // mov bl,ch   (reg=101=CH, rm=011=BL)
    emit(0x80, 0xFB, 0x4F);           // cmp bl,79   (80 cylinders - 1)
    jne("k_cyl_cf");
    emit(0x88, 0xCB);                 // mov bl,cl   (reg=001=CL, rm=011=BL)
    emit(0x80, 0xFB, 0x12);           // cmp bl,18   (sectors per track)
    jne("k_cyl_cf");
    emit(0x88, 0xF3);                 // mov bl,dh   (reg=110=DH, rm=011=BL)
    emit(0x80, 0xFB, 0x01);           // cmp bl,1    (max head: 2 heads - 1)
    jne("k_cyl_cf");
    emit(0x88, 0xD3);                 // mov bl,dl   (reg=010=DL, rm=011=BL)
    emit(0x80, 0xFB, 0x01);           // cmp bl,1    (one floppy attached)
    jne("k_cyl_cf");
    jmp8("k_cyl_ok");
    L("k_cyl_cf");
    fail('G');
    L("k_cyl_ok");

    // --- INT 13h AH=42h: EDD read of LBA 0 through a DAP at 0000:0900 ---
    emit(0xC7, 0x06, 0x00, 0x09, 0x10, 0x00);               // DAP size  = 0x10
    emit(0xC6, 0x06, 0x02, 0x09, 0x01);                     // DAP count = 1
    emit(0xC7, 0x06, 0x04, 0x09, 0x00, 0x0A);               // DAP buffer = 0000:0A00
    // DAP buffer segment and both halves of the 64-bit LBA are already zero:
    // the machine's low RAM comes up zeroed, as it does on a real PC.
    emit(0xBE, 0x00, 0x09);                                 // mov si,0x0900
    emit(0xB4, 0x42);                                       // mov ah,0x42
    emit(0xB2, 0x00);                                       // mov dl,0
    emit(0xCD, 0x13);                                       // int 0x13
    check(JNB, "k_read_ok", 'B');
    // The sector we just read is our own boot sector, so its signature
    // must now be visible at 0000:0BFE.
    emit(0x81, 0x3E, 0xFE, 0x0B, 0x55, 0xAA);               // cmp word [0x0BFE],0xAA55
    check(JZ, "k_sig_ok", 'S');

    // --- INT 15h AX=E820h: first SMAP entry is usable memory at base 0 ---
    emit(0xB8, 0x20, 0xE8);             // mov ax,0xE820
    emit(0x31, 0xDB);                   // xor bx,bx
    emit(0x31, 0xF6);                   // xor si,si
    // ES:DI must point at a buffer for the entry; DI=0 would make the
    // firmware scribble over the interrupt vector table.
    emit(0xBF, 0x00, 0x08);             // mov di,0x0800
    emit(0xCD, 0x15);                   // int 0x15
    check(JNB, "k_e820_ok", '8');
    emit(0x8B, 0x07);                   // mov ax,[di]   (low word of base)
    emit(0x85, 0xC0);                   // test ax,ax
    check(JNE, "k_e820_base_ok", '0');

    // --- INT 15h AX=2402h: A20 status query must succeed ---
    emit(0xB8, 0x02, 0x24);             // mov ax,0x2402
    emit(0xCD, 0x15);                   // int 0x15
    check(JNB, "k_a20q_ok", 'Q');

    // --- INT 15h AX=2401h: enabling A20 must succeed ---
    emit(0xB8, 0x01, 0x24);             // mov ax,0x2401
    emit(0xCD, 0x15);                   // int 0x15
    check(JNB, "k_a20en_ok", 'A');

    // --- INT 15h AX=2400h: disabling A20 must succeed ---
    emit(0xB8, 0x00, 0x24);             // mov ax,0x2400
    emit(0xCD, 0x15);                   // int 0x15
    check(JNB, "k_a20dis_ok", 'X');

    // --- INT 1Ah AH=02h: RTC century byte must be nonzero ---
    emit(0xB4, 0x02);                   // mov ah,0x02
    emit(0xCD, 0x1A);                   // int 0x1A
    emit(0x80, 0xFC, 0x00);             // cmp ah,0
    check(JNE, "k_rtc_ok", 'T');

    // --- INT 16h AH=01h: peek key must return without blocking ---
    emit(0xB4, 0x01);                   // mov ah,0x01
    emit(0xCD, 0x16);                   // int 0x16
    CHECKS++;

    // ---------------------------- verdict ----------------------------
    puts(STR_VERDICT);
    emit(0x80, 0x3E, 0x51, 0x00, 0x00); // cmp byte [0x0051],0
    jne("k_failed");
    tty(0x50); tty(0x41); tty(0x53); tty(0x53);      // "PASS"
    tty(0x0D); tty(0x0A);
    jmp8("halt");
    L("k_failed");
    tty(0x46); tty(0x41); tty(0x49); tty(0x4C);      // "FAIL"
    tty(0x0D); tty(0x0A);
    L("halt");
    jmp8("halt");

    // ---- the print helper, after the code so offset 0 stays the entry ----
    L("@@puts");
    emit(0xAC);                         // lodsb
    emit(0x08, 0xC0);                   // or al,al
    jz("@@puts_ret");
    emit(0xB4, 0x0E, 0xB7, 0x00, 0xCD, 0x10);   // mov ah,0x0E; mov bh,0; int 10h
    jmp8("@@puts");
    L("@@puts_ret");
    emit(0xC3);                         // ret

    if (org > 510) throw new Error("boot sector code overflows 512 bytes: " + org);

    resolve();

    buf[510] = 0x55;
    buf[511] = 0xAA;

    const strings = [
        { addr: STR_BANNER, text: "v86 PCjs firmware self-test\r\n" },
        { addr: STR_VERDICT, text: "RESULT: " },
    ];

    return {
        bytes: buf,
        strings,
        checks: CHECKS,
        failAddr: 0x51,
        codeSize: labels.get("@@puts_ret"),
        entry: 0,
    };
}