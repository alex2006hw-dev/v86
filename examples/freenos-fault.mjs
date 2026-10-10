// Where does FreeNOS fault, under our firmware?
//
//   node examples/freenos-fault.mjs
//
// Boots FreeNOS from its CD image under the built-in firmware and
// catches the exception that stops it, then dumps the CPU state at
// the fault: the instruction pointer, the bytes there, the segment
// registers and their bases, the GDT and IDT pointers, and the
// stack. That is the smallest amount of information that can
// identify the faulting instruction, and it is what a differential
// against SeaBIOS needs to compare.
//
// FreeNOS reaches protected mode and then raises an exception that
// v86 has no handler for, so the emulator panics. The state below
// is read back through the panic, which is why it is taken from
// `uncaughtException` rather than after a timeout.

import fs from "node:fs";
import url from "node:url";
import { V86 } from "../build/libv86.mjs";

const IMAGE = "/localdisk/home/dev/work/iphone/cdrom/FreeNOS-1.0.3.iso";

const emulator = new V86({
    wasm_path: url.fileURLToPath(new URL("../build/v86.wasm", import.meta.url)),
    memory_size: 32 * 1024 * 1024,
    vga_memory_size: 2 * 1024 * 1024,
    disable_jit: !!process.env.FW_NO_JIT,
    firmware: "pcjs",
    bios: undefined,
    vga_bios: undefined,
    cdrom: { buffer: fs.readFileSync(IMAGE).buffer },
    autostart: true,
});

const SEGS = ["ES", "CS", "SS", "DS", "FS", "GS"];

process.on("uncaughtException", (e) =>
{
    const c = emulator.v86.cpu;
    const mem = c.mem8;
    const eip = c.get_real_eip() >>> 0;

    console.log("=== FreeNOS fault, " + (process.env.FW_NO_JIT ? "interpreter" : "JIT") + " ===");
    console.log("exception: " + String(e.message).split("\n")[0]);
    if(e.stack)
    {
        const frames = String(e.stack).split("\n")
            .filter((l) => l.includes("wasm-function"))
            .slice(6, 16)
            .map((l) => l.trim().replace(/^at /, ""))
            .join("\n  ");
        if(frames) console.log("  " + frames);
    }
    console.log("cs=" + c.sreg[1].toString(16) + "  eip=" + eip.toString(16) +
        "  (linear " + eip.toString(16) + ")");

    // The instruction that faulted, and a little around it.
    let code = "";
    for(let i = -8; i < 16; i++)
    {
        const b = mem[(eip + i) >>> 0];
        code += (i === 0 ? "[" : "") + (b === undefined ? "??" : b.toString(16).padStart(2, "0")) +
            (i === 0 ? "]" : "") + " ";
    }
    console.log("  bytes around eip: " + code);

    console.log("  cr0=" + (c.cr0 >>> 0).toString(16));
    console.log("  segs: " + SEGS.map((n, i) => n + "=" + c.sreg[i].toString(16)).join(" "));

    // Segment bases, if the emulator exposes them.
    if(c.segment_offsets)
    {
        console.log("  bases: " + SEGS.map((n, i) =>
            n + "=" + (c.segment_offsets[i] >>> 0).toString(16)).join(" "));
    }

    // General registers, for context.
    const NAMES = ["EAX", "ECX", "EDX", "EBX", "ESP", "EBP", "ESI", "EDI"];
    console.log("  regs: " + NAMES.map((n, i) =>
        n + "=" + (c.reg32[i] >>> 0).toString(16)).join(" "));

    // The stack, using the full ESP -- in 32-bit mode
    // masking to 16 bits names the wrong address.
    const sp = c.reg32[4] >>> 0;
    const ss = c.sreg[2];
    const ssBase = c.segment_offsets ? (c.segment_offsets[2] >>> 0) : (ss << 4);
    let stack = "";
    for(let i = 0; i < 10; i++)
    {
        const a = (ssBase + sp + i * 2) >>> 0;
        const w = (mem[a] | (mem[a + 1] << 8)) & 0xFFFF;
        stack += a.toString(16) + ":" + w.toString(16).padStart(4, "0") + " ";
    }
    console.log("  stack (esp=" + sp.toString(16) + "): " + stack);

    // Segment limits, when exposed -- the GDT entry limits,
    // which say which selectors are in range.
    if(c.segment_limits)
    {
        console.log("  limits: " + SEGS.map((n, i) =>
            n + "=" + (c.segment_limits[i] >>> 0).toString(16)).join(" "));
    }

    // The protected-mode BIOS-call thunk ping-pongs between
    // two addresses before the fault. Dump both, and the
    // fault site, so they can be disassembled.
    const DUMPS = [
        ["fault site", 0x5fd0, 0x6010],
        ["32-bit thunk", 0x82c0, 0x8390],
    ];
    for(const [name, from, to] of DUMPS)
    {
        console.log(name + " (" + from.toString(16) + ".." + to.toString(16) + "):");
        for(let o = from; o < to; o += 16)
        {
            let hex = "";
            for(let i = 0; i < 16; i++) hex += (mem[o+i]??0).toString(16).padStart(2,"0") + " ";
            console.log("  " + o.toString(16).padStart(5,"0") + ": " + hex);
        }
    }

    emulator.destroy();
    process.exit(0);
});

setTimeout(() =>
{
    console.log("no fault within 30s -- FreeNOS may have booted");
    emulator.destroy();
    process.exit(0);
}, 30000);
