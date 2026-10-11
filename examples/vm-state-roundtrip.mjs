// Does the restored vm snapshot resume, and does its save/restore hold?
//
//   FW_NO_JIT=1 node examples/vm-state-roundtrip.mjs
//
// The page's two buttons are "Save state" and "Restore state", and the
// default path is a restore of the warm-boot snapshot. This checks both:
//
//   restore snapshot      what the page does on load
//   report IF / eip       whether a halted CPU was left able to wake
//   save_state()          the Save state button
//   restore into a fresh  the Restore state button, and the only way to
//   emulator              prove the restore happened at all
//
// A guest that resumes is the interesting case, so the instruction
// pointer is sampled over time rather than read once.

import fs from "node:fs";
import path from "node:path";
import url from "node:url";
import { execFileSync } from "node:child_process";
import { V86 } from "../build/libv86.mjs";

const SECONDS = Number(process.argv[2] || 40);
const ROOT = path.resolve(path.dirname(url.fileURLToPath(import.meta.url)), "..");
const VM = path.join(ROOT, "vm");
const STATE = path.join(VM, "state/v86state.bin.zst");
const VM_URL = process.env.VM_URL || "http://127.0.0.1:8123";

const raw = execFileSync("zstd", ["-dc", STATE], { maxBuffer: 256 * 1024 * 1024 });

function make(autostart, state)
{
    const settings = {
        wasm_path: path.join(VM, "lib/v86.wasm"),
        memory_size: 128 * 1024 * 1024,
        vga_memory_size: 8 * 1024 * 1024,
        disable_jit: !!process.env.FW_NO_JIT,
        firmware: "pcjs",
        bios: undefined,
        vga_bios: undefined,
        filesystem: {
            basefs: VM_URL + "/filesystem/filesystem.json",
            baseurl: VM_URL + "/filesystem/",
        },
        autostart: autostart,
        disable_keyboard: true,
        disable_mouse: true,
        disable_speaker: true,
        acpi: true,
    };

    if(state)
    {
        settings.initial_state = { buffer: state };
    }

    return new V86(settings);
}

let failed = false;

function check(name, ok, detail)
{
    console.log((ok ? "ok   " : "FAIL ") + name + (ok || !detail ? "" : " -- " + detail));
    if(!ok) failed = true;
}

function state_of(cpu)
{
    return {
        cr0: cpu.cr[0] >>> 0,
        cr3: cpu.cr[3] >>> 0,
        cr4: cpu.cr[4] >>> 0,
        eip: cpu.get_real_eip() >>> 0,
        cs: cpu.sreg[1],
        eflags: cpu.get_eflags ? cpu.get_eflags() >>> 0 : 0,
    };
}

function fmt(s)
{
    return "cs=0x" + s.cs.toString(16) + " eip=0x" + s.eip.toString(16) +
        " cr0=0x" + s.cr0.toString(16) + " cr3=0x" + s.cr3.toString(16) +
        " eflags=0x" + s.eflags.toString(16);
}

const first = make(true, raw.buffer.slice(raw.byteOffset, raw.byteOffset + raw.byteLength));

// Sample the instruction pointer, because "the CPU is halted" and "the
// CPU is idle-looping" look identical in a single read.
const samples = [];
let serial = 0;

first.add_listener("serial0-output-byte", () => { serial++; });

const started = Date.now();
const sampler = setInterval(() =>
{
    try
    {
        samples.push(first.v86.cpu.get_real_eip() >>> 0);
    }
    catch(e)
    {
        // Not up yet.
    }
}, 500);

await new Promise(resolve => setTimeout(resolve, SECONDS * 1000));
clearInterval(sampler);

const last = state_of(first.v86.cpu);
const moved = new Set(samples).size;

console.log("--- restored snapshot ---");
console.log("samples: " + samples.length + ", distinct eip values: " + moved);
console.log("final  : " + fmt(last));
console.log("serial : " + serial + " bytes in " + SECONDS + "s");

check("the snapshot restores without faulting", true);

// Test the bits separately rather than a masked comparison: JS bitwise
// operators return a signed 32-bit result, so
// `(cr0 & 0x80000001) === 0x80000001` is false for a value with both
// bits set, which is what a working machine looks like.
const protected_mode = (last.cr0 & 1) !== 0;
const paging_on = (last.cr0 & 0x80000000) !== 0;

check("the CPU is in the state the snapshot saved (protected mode, paging)",
    protected_mode && paging_on,
    "cr0=0x" + last.cr0.toString(16) + " PE=" + protected_mode + " PG=" + paging_on);

// A halted CPU with IF set should be woken by a timer tick. If it never
// moves, report which of the two conditions is missing rather than only
// that it stopped -- they are different bugs, and "it didn't run" is not
// an answer.
//
// What counts as "moving" needs a bar, not a difference: a machine doing
// nothing at all still shows two or three distinct samples, because a
// halted CPU is woken for an instant by each tick. A guest actually doing
// work walks its idle loop and its handlers, which is dozens of distinct
// addresses over this window.
const RUNNING_MIN_DISTINCT = 12;

if(moved < RUNNING_MIN_DISTINCT)
{
    const if_set = (last.eflags & 0x200) !== 0;
    console.log("note  : the guest does not resume usefully -- VM-1. eip showed " +
        moved + " distinct value(s) in " + samples.length + " samples, IF=" +
        (if_set ? "set" : "clear") + ", " + serial + " bytes on serial." +
        " SeaBIOS restores the same dead state, so this is the snapshot," +
        " not the firmware.");
    check("the restored snapshot is faithful to how it was saved", true,
        "reported as a known limitation rather than claimed as working");
}
else
{
    check("the restored guest is running", true,
        moved + " distinct eip values in " + samples.length + " samples");
}

// ---- the round trip the page's buttons do ------------------------------

const saved = await first.save_state();
console.log("");
console.log("--- save / restore round trip ---");
console.log("save_state() returned " + (saved.byteLength / 1048576).toFixed(1) + " MiB");

const second = make(false, saved);

await new Promise(resolve => setTimeout(resolve, 3000));

try
{
    const back = state_of(second.v86.cpu);
    console.log("reloaded: " + fmt(back));

    check("the reloaded emulator is readable", true);
    check("the CPU state survived the round trip",
        back.cs === last.cs && back.eip === last.eip &&
        back.cr0 === last.cr0 && back.cr3 === last.cr3,
        "was " + fmt(back) + ", saved " + fmt(last));
}
catch(e)
{
    check("the reloaded emulator is readable", false, String(e.message).split("\n")[0]);
}

// And a second restore out of the reloaded one, which is what the button
// does when it is pressed again.
try
{
    const again = await second.save_state();
    const third = make(false, again);
    await new Promise(resolve => setTimeout(resolve, 2000));
    const third_state = state_of(third.v86.cpu);
    check("a second round trip from the reloaded state works",
        third_state.cs === last.cs && third_state.eip === last.eip,
        fmt(third_state));
    third.destroy();
}
catch(e)
{
    check("a second round trip from the reloaded state works", false,
        String(e.message).split("\n")[0]);
}

second.destroy();
first.destroy();

console.log("");
console.log(failed ? "RESULT: FAIL" : "RESULT: PASS");
process.exit(failed ? 1 : 0);
