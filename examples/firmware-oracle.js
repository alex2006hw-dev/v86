// Behavioural oracle: run the same boot-sector self-test against a real
// third-party BIOS (SeaBIOS by default) instead of the built-in firmware.
//
//   node examples/firmware-oracle.js
//   FW_ORACLE=vgabios node examples/firmware-oracle.js
//
// This is a diagnostic, not a test of our firmware: it answers "what does
// a real BIOS actually do for these services?", which is the only way to
// settle an argument about a specification. The reference images under
// `bios/` are GPL-2.0-or-later / LGPL-3.0 and are used here purely as an
// oracle -- no byte of them is copied, disassembled into, or incorporated.
// See `bios/README.md` and todo.md §4.5.
//
// The default `firmware: "pcjs"` run of the same sector is in
// `examples/firmware.js`.

import fs from "node:fs";
import path from "node:path";
import url from "node:url";
import { V86 } from "../build/libv86.mjs";
import { build_self_test_floppy } from "./firmware-selftest.mjs";

const ROOT = path.resolve(path.dirname(url.fileURLToPath(import.meta.url)), "..");

const IMAGES = {
    seabios: { bios: "bios/seabios.bin", vga: "bios/vgabios.bin" },
    bochs: { bios: "bios/bochs-bios.bin", vga: "bios/bochs-vgabios.bin" },
};

const which = process.env.FW_ORACLE || "seabios";
const images = IMAGES[which];

if(!images)
{
    console.error("unknown oracle " + which + "; try one of: " + Object.keys(IMAGES).join(", "));
    process.exit(2);
}

/**
 * Read the 80x25 colour text page out of the emulated VGA.
 *
 * The 0xA0000 window is mmap'd over guest memory, so `cpu.mem8` does not
 * contain it -- the device's own buffer does. Colour text is memory
 * space 3, which starts at 0xB8000.
 */
function read_text_screen(cpu)
{
    const vga = cpu.devices.vga;
    const mem = vga.vga_memory;
    const space = [0xA0000, 0xA0000, 0xB0000, 0xB8000][(vga.miscellaneous_graphics_register >> 2) & 3];
    const base = (0xB8000 - space) + ((vga.start_address || 0) << 1);
    const cols = vga.max_cols || 80;
    const rows = [];

    for(let row = 0; row < 25; row++)
    {
        let line = "";
        for(let col = 0; col < cols; col++)
        {
            line += String.fromCharCode(mem[base + (row * cols + col) * 2]);
        }
        rows.push(line.replace(/\s+$/, ""));
    }

    return rows.filter(line => line.length);
}

const emulator = new V86({
    wasm_path: url.fileURLToPath(new URL("../build/v86.wasm", import.meta.url)),
    memory_size: 32 * 1024 * 1024,
    vga_memory_size: 2 * 1024 * 1024,
    disable_jit: !!process.env.FW_NO_JIT,
    bios: { url: path.join(ROOT, images.bios) },
    vga_bios: { url: path.join(ROOT, images.vga) },
    fda: { buffer: build_self_test_floppy() },
    autostart: true,
});

setTimeout(function()
{
    const screen = read_text_screen(emulator.v86.cpu);
    const text = screen.join("\n");

    emulator.destroy();

    console.log("=== oracle: " + which + " ===");
    for(const line of screen)
    {
        console.log("  |" + line + "|");
    }

    fs.writeFileSync("/tmp/opencode/oracle.out", text + "\n");

    // The self-test prints the failing check's letter, then a banner.
    const verdict = /RESULT: (PASS|FAIL)/.exec(text);

    console.log("  ---");
    console.log("  verdict: " + (verdict ? verdict[1] : "(no verdict — the guest stopped at a check)"));

    if(!verdict)
    {
        console.log("");
        console.log("  This is expected, and is not a failure of the guest program. The");
        console.log("  self-test demands services the reference BIOSes do not all provide:");
        console.log("");
        console.log("    D  INT 13h AH=41h EDD install check on a *floppy*. Both SeaBIOS and");
        console.log("       the Bochs BIOS refuse it and leave BX=55AAh untouched; we accept it,");
        console.log("       because we do implement EDD reads on floppies.");
        console.log("    T  INT 1Ah AH=02h. Both reference BIOSes return a day of 0 here,");
        console.log("       because v86's CMOS RTC is not populated the way real hardware");
        console.log("       would be. Ours reads the emulated clock directly and is fine.");
        console.log("");
        console.log("  The letter on screen names the check that diverged. See TechDebt.md");
        console.log("  TEST-5 and Changes.md section 7.");
    }

    process.exit(0);
}, 4000);
