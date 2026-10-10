// Can a guest install a GDT at all?
//
//   node examples/lgdt-probe.mjs
//   FW_ORACLE=seabios node examples/lgdt-probe.mjs
//
// A twenty-line boot sector that writes a pseudo-descriptor, executes
// `LGDT`, immediately reads it back with `SGDT`, and parks the result where
// the host can find it -- with a sentinel afterwards to prove it actually got
// there. No firmware, no boot loader, no protected mode.
//
// The answer is yes -- but it took a fix to get here.
// The ModRM byte for a [disp16] operand is rm=110; rm=101
// is [DI], which made an earlier version of this probe read
// its pseudo-descriptor from DI*16 and report the GDT as
// lost. That failure was silent, which is the point of
// keeping the probe: it pins the encoding down.

// `lgdt` raises #GP only when CPL is non-zero, and returns silently on a
// page fault, so "GDTR is still zero afterwards" means the store never
// happened rather than that something clobbered it.

import fs from "node:fs";
import url from "node:url";
import { V86 } from "/localdisk/home/dev/work/iphone/v86/build/libv86.mjs";
import { Asm } from "/localdisk/home/dev/work/iphone/v86/examples/firmware-selftest.mjs";

const SECTOR = 512;
const a = new Asm(SECTOR);

a.label("start");
a.cli();
a.xor_ax_ax();
a.mov_seg_ax(3); a.mov_seg_ax(0); a.mov_seg_ax(2);
a.mov_r16(4, 0x7000);
a.sti();
a.xor_ax_ax(); a.mov_seg_ax(3);   // DS = 0

// pseudo-descriptor for a GDT at 0000:0400 (linear 0x20400), limit 0x1F
a.mov_moffs_imm16(0x0800, 0x001F);
a.mov_moffs_imm16(0x0802, 0x0400);
a.mov_moffs_imm16(0x0804, 0x0000);

a.lgdt_moffs(0x0800);      // lgdt [0000:0800]
a.sgdt_moffs(0x0810);      // sgdt [0000:0810] -- read it back

a.mov_moffs_imm16(0x0820, 0xBEEF);   // sentinel: proves we got here
a.mov_al(0x5A); a.int(0x10);          // and drew something

a.label("done");
a.cli(); a.hlt(); a.b(0xEB, 0xFD);

const code = a.link();
const image = new Uint8Array(2880 * SECTOR);
image.set(code, 0);
image[SECTOR-2] = 0x55; image[SECTOR-1] = 0xAA;

const emu = new V86({
    wasm_path: "/localdisk/home/dev/work/iphone/v86/build/v86.wasm",
    memory_size: 32*1024*1024, vga_memory_size: 2*1024*1024,
    disable_jit: true,
    ...(process.env.FW_ORACLE === "none"
        ? { firmware: "pcjs", bios: undefined, vga_bios: undefined }
        : { bios: { url: "/localdisk/home/dev/work/iphone/v86/bios/seabios.bin" },
            vga_bios: { url: "/localdisk/home/dev/work/iphone/v86/bios/vgabios.bin" } }),
    fda: { buffer: image.buffer }, autostart: true,
});

process.on("uncaughtException", e => {
    console.log("PANIC: " + e.message);
    process.exit(1);
});

setTimeout(() => {
    const mem = emu.v86.cpu.mem8;
    // The CS-relative sgdt wrote 6 bytes at offset 0x430 of the boot sector,
    // which is at linear 0x7C00 + 0x430.
    const at = 0x810;
    const limit = mem[at] | (mem[at+1] << 8);
    const base = (mem[at+2] | (mem[at+3]<<8) | (mem[at+4]<<16) | (mem[at+5]<<24)) >>> 0;
    console.log("lgdt [0x0428]  then  sgdt ->");
    console.log("  gdtr limit = 0x" + limit.toString(16) + "   (asked for 0x1F)");
    console.log("  gdtr base  = 0x" + base.toString(16) + "  (asked for 0x0400)");
    const sentinel = mem[0x820] | (mem[0x821] << 8);
    console.log("  sentinel at 0x820 = 0x" + sentinel.toString(16) +
        (sentinel === 0xBEEF ? "  (probe reached the sgdt)" : "  (PROBE DID NOT REACH IT)"));
    console.log(limit === 0x1F && base === 0x0400 ? "RESULT: lgdt/sgdt round-trip OK"
                                                   : "RESULT: MISMATCH -- lgdt did not store");
    emu.destroy(); process.exit(0);
}, 3000);
