#!/usr/bin/env node
// Boot a floppy on the built-in PCjs-derived firmware and report what the
// guest printed.
//
//   node examples/firmware.js [floppy.img]
//
// With no argument a self-testing boot sector is generated in memory, so
// this runs in a clean checkout with no disk images present. The boot
// sector exercises the services a real boot loader depends on and prints
// `RESULT: PASS` or `RESULT: FAIL`, which makes it an end-to-end smoke
// test of the firmware: reset vector, POST, INT 19h, and the INT
// 10h/11h/12h/13h/15h/16h/1Ah services.
//
// The same image boots under SeaBIOS, so any difference between the two
// runs is a firmware bug rather than a guest bug. That is the point of
// keeping the boot sector trivial.
//
// Environment:
//   FW_NO_JIT=1   run without the JIT, to tell a firmware bug from a
//                 translator bug when something goes wrong
//   FW_TRACE=1    ask the firmware for its internal trace log
//
// The firmware is selected with `firmware: "pcjs"`; leave that out and
// v86 loads `bios/seabios.bin` exactly as before.

import url from "node:url";
import { V86 } from "../build/libv86.mjs";
import { build_self_test_floppy } from "./firmware-selftest.mjs";

const image = process.argv[2];

const emulator = new V86({
    wasm_path: url.fileURLToPath(new URL("../build/v86.wasm", import.meta.url)),
    memory_size: 32 * 1024 * 1024,
    vga_memory_size: 2 * 1024 * 1024,
    disable_jit: !!process.env.FW_NO_JIT,
    firmware: "pcjs",            // use the built-in firmware, not a ROM image
    firmware_trace: !!process.env.FW_TRACE,
    bios: undefined,
    vga_bios: undefined,
    fda: image ? { url: image } : { buffer: build_self_test_floppy() },
    autostart: true,
});

/**
 * Read the 80x25 colour text page out of the emulated VGA.
 *
 * The VGA window at 0xA0000-0xBFFFF is mmap'd over guest memory, so
 * `cpu.mem8` does not contain it -- the device's own buffer does, at the
 * CRTC start address. There is also no ScreenAdapter in a headless Node
 * run, so the `screen-put-char` event never fires; reading the device is
 * the only way to see what the guest drew.
 *
 * @param {object} cpu
 * @return {string[]} one string per row
 */
function read_text_screen(cpu)
{
    const vga = cpu.devices.vga;
    const mem = vga.vga_memory;

    // The 0xA0000 window is split into four memory spaces selected by the
    // miscellaneous register; colour text lives in space 3, which starts
    // at 0xB8000. `vga_memory` is addressed relative to that space's base,
    // with the CRTC start address added, so the offset has to be computed
    // rather than assumed.
    const space_base = [0xA0000, 0xA0000, 0xB0000, 0xB8000][(vga.miscellaneous_graphics_register >> 2) & 3];
    const base = (0xB8000 - space_base) + ((vga.start_address || 0) << 1);
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

function screen_text()
{
    return read_text_screen(emulator.v86.cpu).filter(l => l.length).join("\n");
}

function finish(verdict)
{
    console.log("--- guest screen ---");
    console.log(screen_text());
    console.log("--------------------");

    const pass = verdict !== null && /RESULT: PASS/.test(verdict);

    if(pass)
    {
        console.log("PASS: the built-in firmware booted from floppy and served every BIOS service");
    }
    else
    {
        console.error("FAIL: " + (verdict || "the guest never printed a verdict"));
    }

    if(process.env.FW_TRACE)
    {
        const cpu = emulator.v86.cpu;
        const ptr = cpu.firmware_trace_ptr();
        const len = cpu.firmware_trace_len();

        if(ptr && len)
        {
            const text = new TextDecoder().decode(new Uint8Array(cpu.wasm_memory.buffer, ptr, len));
            console.log("--- firmware trace ---");
            process.stdout.write(text);
            console.log("---------------------");
            console.log("dropped entries:", cpu.firmware_trace_dropped());
        }
    }

    emulator.destroy();
    process.exit(pass ? 0 : 1);
}

// Poll the text page. The guest halts in its own spin loop once it has
// printed, so a bounded poll is enough and needs no screen adapter.
const deadline = Date.now() + 30000;
const timer = setInterval(function()
{
    let text = "";

    try
    {
        text = screen_text();
    }
    catch(e)
    {
        // The VGA device may not exist yet while start-up is in progress.
    }

    if(/RESULT:/.test(text))
    {
        clearInterval(timer);
        finish((text.match(/RESULT: \w+/) || [null])[0]);
        return;
    }

    if(Date.now() > deadline)
    {
        clearInterval(timer);
        console.error("timed out after 30s; the guest never printed a verdict");
        finish(null);
    }
}, 50);
