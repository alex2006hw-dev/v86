#!/usr/bin/env node
// Diagnostic: what did the built-in firmware do?
//
//   node examples/firmware-debug.js
//   FW_TRACE=1 node examples/firmware-debug.js
//   FW_NO_JIT=1 node examples/firmware-debug.js
//
// Dumps the state a boot failure leaves behind: the reset vector, the ROM
// images, the IVT the firmware installed, the BDA it filled in, whether a
// boot sector was loaded, where the CPU stopped, which services were
// dispatched, the POST banner and (with FW_TRACE=1) the firmware's own
// trace log.
//
// This is the example to reach for when the firmware goes quiet, which is
// what a boot failure looks like from the outside.

import url from "node:url";
import { V86 } from "../build/libv86.mjs";
import { build_self_test_floppy } from "./firmware-selftest.mjs";

const TRACE = !!process.env.FW_TRACE;

const emulator = new V86({
    wasm_path: url.fileURLToPath(new URL("../build/v86.wasm", import.meta.url)),
    memory_size: 32 * 1024 * 1024,
    vga_memory_size: 2 * 1024 * 1024,
    disable_jit: !!process.env.FW_NO_JIT,
    firmware: "pcjs",
    firmware_trace: TRACE,
    fda: { buffer: build_self_test_floppy() },
    autostart: true,
});

function hex(v, n = 4)
{
    return (v >>> 0).toString(16).toUpperCase().padStart(n, "0");
}

function dump(label, fn)
{
    console.log("=== " + label + " ===");
    try { fn(); } catch(e) { console.log("  (unavailable: " + e.message + ")"); }
}

/**
 * Read the 80x25 colour text page out of the emulated VGA.
 *
 * The 0xA0000 window is mmap'd over guest memory, so `cpu.mem8` does not
 * contain it -- the device's own buffer does. The window is split into four
 * memory spaces selected by the miscellaneous register; colour text is
 * space 3, which starts at 0xB8000, and `vga_memory` is addressed relative
 * to the selected space's base plus the CRTC start address.
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

    return rows;
}

setTimeout(function()
{
    const cpu = emulator.v86.cpu;
    const mem = cpu.mem8;
    const w = a => mem[a] | (mem[a + 1] << 8);
    const d = a => (mem[a] | (mem[a + 1] << 8) | (mem[a + 2] << 16) | (mem[a + 3] << 24)) >>> 0;

    dump("reset vector F000:FFF0", () =>
        console.log("  " + [...mem.slice(0xFFFF0, 0xFFFF6)].map(b => hex(b, 2)).join(" ")));

    dump("system ROM POST entry F000:0000", () =>
        console.log("  " + [...mem.slice(0xF0000, 0xF0018)].map(b => hex(b, 2)).join(" ")));

    dump("video option ROM header C000:0000", () =>
        console.log("  " + [...mem.slice(0xC0000, 0xC0008)].map(b => hex(b, 2)).join(" ")));

    dump("VBE signature", () =>
        console.log("  " + JSON.stringify(String.fromCharCode(...mem.slice(0xC0100, 0xC0104)))));

    dump("IVT", () => {
        for(const v of [0x08, 0x09, 0x10, 0x11, 0x12, 0x13, 0x15, 0x16, 0x19, 0x1A, 0x43])
        {
            const off = w(v * 4);
            const seg = w(v * 4 + 2);
            console.log(`  INT ${hex(v, 2)}h -> ${hex(seg)}:${hex(off)}`);
        }
    });

    dump("BDA", () => {
        console.log("  memory KiB (0x413):", w(0x413));
        console.log("  equipment  (0x410):", hex(w(0x410)));
        console.log("  video mode (0x449):", hex(mem[0x449], 2));
        console.log("  columns    (0x44A):", mem[0x44A]);
        console.log("  cursor row/col     :", mem[0x450], mem[0x451]);
        console.log("  timer ticks (0x46C):", d(0x46C));
    });

    dump("boot sector at 0000:7C00", () => {
        console.log("  first bytes:", [...mem.slice(0x7C00, 0x7C10)].map(b => hex(b, 2)).join(" "));
        // The 0x55AA signature is at offset 510 of the sector.
        console.log("  signature:", hex(mem[0x7C00 + 510], 2), hex(mem[0x7C00 + 511], 2));
    });

    dump("floppy drive 0 in wasm memory", () => {
        const ptr = cpu.firmware_drive_ptr(0);
        const len = cpu.firmware_drive_len(0);

        if(!ptr || !len)
        {
            console.log("  (no drive)");
            return;
        }

        const img = new Uint8Array(cpu.wasm_memory.buffer, ptr, len);
        console.log("  length:", len);
        console.log("  sector 0:", [...img.slice(0, 12)].map(b => hex(b, 2)).join(" "));
        console.log("  signature:", hex(img[510], 2), hex(img[511], 2));
    });

    dump("CPU", () => {
        console.log("  instruction_pointer:", hex(cpu.instruction_pointer[0], 6));
        console.log("  CS:", hex(cpu.sreg[1]), " DS:", hex(cpu.sreg[3]), " SS:", hex(cpu.sreg[2]),
            " SP:", hex(cpu.reg16[4]));
    });

    dump("firmware", () => {
        console.log("  installed:", cpu.firmware_present() === 1);
        console.log("  traps fired:", cpu.firmware_trap_count());
        console.log("  last service id:",
            cpu.firmware_last_service() === 0xFFFF
                ? "none" : "0x" + hex(cpu.firmware_last_service()));

        const n = cpu.firmware_trap_ring_len();
        console.log("  trap ring (return address / service / SP), last " + Math.min(n, 8) + ":");

        for(let i = Math.max(0, n - 8); i < n; i++)
        {
            console.log("    " + hex(cpu.firmware_trap_ring(i), 6) +
                "  svc=0x" + hex(cpu.firmware_trap_ring_service(i)) +
                "  sp=0x" + hex(cpu.firmware_trap_ring_stack(i), 5));
        }
    });

    dump("firmware trace", () => {
        const ptr = cpu.firmware_trace_ptr();
        const len = cpu.firmware_trace_len();

        if(!ptr || !len)
        {
            console.log("  (empty — rerun with FW_TRACE=1)");
            return;
        }

        const text = new TextDecoder().decode(new Uint8Array(cpu.wasm_memory.buffer, ptr, len));
        process.stdout.write(text.replace(/^/gm, "  "));
        console.log("  dropped entries:", cpu.firmware_trace_dropped());
    });

    dump("screen (80x25 text)", () => {
        for(const line of read_text_screen(cpu))
        {
            if(line.length)
            {
                console.log("  |" + line + "|");
            }
        }
    });

    emulator.destroy();
    process.exit(0);
}, 1500);
