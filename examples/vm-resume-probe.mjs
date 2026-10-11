// Probe what the vm snapshot actually does after a restore.
//
//   FW_NO_JIT=1 node examples/vm-resume-probe.mjs [seconds]
//
// The snapshot boots with the CPU halted, so the first question is
// whether it resumes at all. The second is where its output goes: the
// cold-boot cmdline carries `console=ttyS0`, but that is this page's own
// command line, not the one the snapshot's kernel was booted with, so
// the guest may be printing to the VGA screen instead. Both are watched
// here.
//
// This needs `vm/` served over HTTP: the restore configures the 9p
// filesystem, and `baseurl` is fetched.
//
//     $ (cd vm && python3 -m http.server 8123)
//     $ FW_NO_JIT=1 node examples/vm-resume-probe.mjs 90

import fs from "node:fs";
import path from "node:path";
import url from "node:url";
import { execFileSync } from "node:child_process";
import { V86 } from "../build/libv86.mjs";

const SECONDS = Number(process.argv[2] || 90);
const ROOT = path.resolve(path.dirname(url.fileURLToPath(import.meta.url)), "..");
const VM = path.join(ROOT, "vm");
const STATE = path.join(VM, "state/v86state.bin.zst");
const VM_URL = process.env.VM_URL || "http://127.0.0.1:8123";

const raw = execFileSync("zstd", ["-dc", STATE], { maxBuffer: 256 * 1024 * 1024 });

// FW_BIOS=seabios restores against the real SeaBIOS pair instead of the
// built-in firmware. That is the control for the whole experiment: the
// snapshot was saved while SeaBIOS was the BIOS, so if only the built-in
// firmware fails to resume it, the difference is ours.
const use_seabios = process.env.FW_BIOS === "seabios";

const settings = {
    wasm_path: path.join(VM, "lib/v86.wasm"),
    memory_size: 128 * 1024 * 1024,
    vga_memory_size: 8 * 1024 * 1024,
    disable_jit: true,
    filesystem: {
        basefs: VM_URL + "/filesystem/filesystem.json",
        baseurl: VM_URL + "/filesystem/",
    },
    initial_state: { buffer: raw.buffer.slice(raw.byteOffset, raw.byteOffset + raw.byteLength) },
    autostart: true,
    disable_keyboard: true,
    disable_mouse: true,
    disable_speaker: true,
    acpi: true,
};

if(use_seabios)
{
    settings.bios = { url: VM_URL + "/bios/seabios.bin" };
    settings.vga_bios = { url: VM_URL + "/bios/vgabios.bin" };
}
else
{
    settings.firmware = "pcjs";
    settings.bios = undefined;
    settings.vga_bios = undefined;
}

const emulator = new V86(settings);

console.log("restoring with " + (use_seabios ? "SeaBIOS (the snapshot's own BIOS)" : "the built-in firmware"));

let serial = "";

emulator.add_listener("serial0-output-byte", (byte) =>
{
    serial += String.fromCharCode(byte);
    process.stdout.write(String.fromCharCode(byte));
});

function read_vga_text()
{
    const vga = emulator.v86.cpu.devices.vga;
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

    return rows.filter(l => l.length).join("\n");
}

function snapshot(tag)
{
    let vga = "";

    try
    {
        vga = read_vga_text();
    }
    catch(e)
    {
        vga = "(unreadable: " + String(e.message).split("\n")[0] + ")";
    }

    console.log("");
    console.log("=== " + tag + " ===");
    console.log("serial bytes: " + serial.length);
    console.log("cpu: " + (emulator.v86.cpu.sreg[1].toString(16)) + ":" + emulator.v86.cpu.get_real_eip().toString(16));
    console.log("--- vga text screen ---");
    console.log(vga || "(blank)");
}

const timer = setInterval(function()
{
    snapshot(Date.now() % 100000 + "");
}, 10000);

setTimeout(function()
{
    clearInterval(timer);
    snapshot("final");
    console.log("");
    console.log("serial total: " + serial.length);
    emulator.destroy();
    process.exit(0);
}, SECONDS * 1000);
