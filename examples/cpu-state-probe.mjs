// What does each BIOS leave in the CPU when it hands over to a boot loader?
//
//   node examples/cpu-state-probe.mjs
//   FW_ORACLE=seabios node examples/cpu-state-probe.mjs
//   FW_ORACLE=bochs   node examples/cpu-state-probe.mjs
//
// This is the differential that matters for BOOT-2. FreeNOS and HelenOS fault
// on a protected-mode entry under the built-in firmware but boot to a login
// prompt under SeaBIOS, so whatever the two firmwares leave behind must
// differ. Rather than disassemble 128 KB of GPL-2.0 assembly to guess, this
// asks the only question that matters, empirically: a boot sector records the
// machine state the moment it is entered, and the host reads it back out of
// guest memory.
//
// Recorded: IDTR, GDTR, CR0, EFLAGS, and the segment and stack registers.
// IDTR and GDTR are the interesting pair -- `lookup_segment_selector` compares
// a selector against `gdtr_size`, and `trigger_gp` cannot deliver an exception
// at all when `idtr_size` is 0.

import fs from "node:fs";
import url from "node:url";
import { V86 } from "../build/libv86.mjs";
import { Asm } from "./firmware-selftest.mjs";

const SECTOR = 512;
const OUT = 0x0600;                 // segment 0, offset 0x600
const PHYS = OUT;

// Fields recorded, in order. The boot sector stores its results at OUT.
const F_IDTR = 0x00,   // 6 bytes: limit, base
      F_GDTR = 0x06,   // 6 bytes: limit, base
      F_CR0 = 0x0C,    // 4
      F_FLAGS = 0x10,  // 2
      F_SS_SP = 0x12,  // 4: SS, SP
      F_CS_IP = 0x16,  // 4: CS, IP
      F_DS_ES = 0x1A;  // 4: DS, ES

/**
 * A boot sector that records the CPU state it was entered with, then stops.
 *
 * It deliberately does nothing else: any other instruction risks faulting and
 * losing the measurement. `sidt`/`sgdt` are the two-instruction forms with a
 * 16-bit operand size, which v86 implements (0F 01 /1 and 0F 01 /0 with a
 * disp16 operand).
 */
function build_probe()
{
    const a = new Asm(SECTOR);

    a.label("start");
    a.cli();
    a.xor_ax_ax();
    a.mov_seg_ax(3);              // DS
    a.mov_seg_ax(0);              // ES
    a.mov_seg_ax(2);              // SS
    a.mov_r16(4, 0x7000);         // SP
    a.sti();

    // mov ax, OUT ; mov ds, ax -- but DS must stay 0 to record at 0x600.
    // Use CS-relative addressing instead: CS is the boot sector's segment and
    // the offsets below are within it.
    a.sidt_cs(OUT + F_IDTR);
    a.sgdt_cs(OUT + F_GDTR);
    a.mov_eax_cr0();
    a.mov_moffs_reg(OUT + F_CR0, 0);
    a.pushf();
    a.pop_ax();
    a.mov_moffs_reg(OUT + F_FLAGS, 0);
    a.mov_moffs_reg(OUT + F_SS_SP + 0, 2);   // SS
    a.mov_moffs_reg(OUT + F_SS_SP + 2, 4);   // SP
    a.mov_moffs_reg(OUT + F_CS_IP + 0, 1);    // CS
    a.mov_moffs_reg(OUT + F_DS_ES + 0, 3);    // DS
    a.mov_moffs_reg(OUT + F_DS_ES + 2, 0);    // ES

    a.label("done");
    a.cli();
    a.hlt();
    a.b(0xEB, 0xFD);

    const code = a.link();
    const image = new Uint8Array(2880 * SECTOR);
    image.set(code, 0);
    image[SECTOR - 2] = 0x55;
    image[SECTOR - 1] = 0xAA;
    return image.buffer;
}

function hex(v, n = 4)
{
    return "0x" + (v >>> 0).toString(16).toUpperCase().padStart(n, "0");
}

const which = process.env.FW_ORACLE || "none";

const settings = {
    wasm_path: url.fileURLToPath(new URL("../build/v86.wasm", import.meta.url)),
    memory_size: 32 * 1024 * 1024,
    vga_memory_size: 2 * 1024 * 1024,
    disable_jit: !!process.env.FW_NO_JIT,
    fda: { buffer: build_probe() },
    autostart: true,
};

if(which === "none")
{
    settings.firmware = "pcjs";
    settings.bios = undefined;
    settings.vga_bios = undefined;
}
else
{
    settings.bios = { url: url.fileURLToPath(new URL("../bios/seabios.bin", import.meta.url)) };
    settings.vga_bios = { url: url.fileURLToPath(new URL("../bios/vgabios.bin", import.meta.url)) };
}

const emulator = new V86(settings);

setTimeout(() =>
{
    const mem = emulator.v86.cpu.mem8;
    const d16 = a => mem[a] | (mem[a + 1] << 8);
    const d32 = a => (mem[a] | (mem[a + 1] << 8) | (mem[a + 2] << 16) | (mem[a + 3] << 24)) >>> 0;

    const report = {
        idt_limit: d16(PHYS + F_IDTR),
        idt_base: d32(PHYS + F_IDTR + 2),
        gdt_limit: d16(PHYS + F_GDTR),
        gdt_base: d32(PHYS + F_GDTR + 2),
        cr0: d32(PHYS + F_CR0),
        flags: d16(PHYS + F_FLAGS),
        ss: d16(PHYS + F_SS_SP),
        sp: d16(PHYS + F_SS_SP + 2),
        cs: d16(PHYS + F_CS_IP),
        ds: d16(PHYS + F_DS_ES),
        es: d16(PHYS + F_DS_ES + 2),
    };

    console.log("=== CPU state at boot handoff: " + which + " ===");
    console.log("  IDTR  limit=" + hex(report.idt_limit) + " base=" + hex(report.idt_base, 8));
    console.log("  GDTR  limit=" + hex(report.gdt_limit) + " base=" + hex(report.gdt_base, 8));
    console.log("  CR0   " + hex(report.cr0, 8) +
        (report.cr0 & 1 ? "  (PE set!)" : "  (PE clear, real mode)"));
    console.log("  EFLAGS " + hex(report.flags) + (report.flags & 0x200 ? "  (IF set)" : "  (IF clear)"));
    console.log("  CS=" + hex(report.cs) + " SS=" + hex(report.ss) +
        " SP=" + hex(report.sp) + " DS=" + hex(report.ds) + " ES=" + hex(report.es));

    fs.writeFileSync("/tmp/opencode/cpu-state-" + which + ".json", JSON.stringify(report, null, 2));
    emulator.destroy();
    process.exit(0);
}, 2000);
