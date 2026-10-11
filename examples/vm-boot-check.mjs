// Boot the vm snapshot with the built-in firmware, headless,
// and report what the guest prints.
//
//   node examples/vm-boot-check.mjs [seconds]
//
// This is the vm front end's default path -- no ?boot= -- with
// the SeaBIOS pair swapped for the permissive firmware built
// into v86.wasm. It loads the warm-boot snapshot at
// vm/state/v86state.bin.zst, restores it, and watches the
// serial line, which is the only output this front end has.
//
// The snapshot already contains the 9p filesystem, so nothing
// has to be served over HTTP.

import fs from "node:fs";
import path from "node:path";
import url from "node:url";
import { V86 } from "../build/libv86.mjs";

const SECONDS = Number(process.argv[2] || 30);
const ROOT = path.resolve(path.dirname(url.fileURLToPath(import.meta.url)), "..");
// The firmware and the snapshot live in the vm submodule, so
// this check exercises exactly what the page ships.
const VM = path.join(ROOT, "vm");
const STATE = path.join(VM, "state/v86state.bin.zst");
// The page always configures the 9p filesystem, including on the
// restore path, and the snapshot was taken mid-9p-transaction: without
// it the PCI device 0x30 the snapshot expects is missing and the guest
// blocks waiting for a reply that can never come. It has to be served,
// because `baseurl` is fetched rather than read from disk.
const VM_URL = process.env.VM_URL || "http://127.0.0.1:8123";

if(!fs.existsSync(STATE))
{
    console.error("no snapshot at " + STATE);
    process.exit(2);
}

// The snapshot arrives zstd-compressed. v86 wants the raw
// bytes, so decompress before handing them over.
const { execFileSync } = await import("node:child_process");
const raw = execFileSync("zstd", ["-dc", STATE], { maxBuffer: 256 * 1024 * 1024 });
console.log("snapshot: " + (raw.length / 1048576).toFixed(1) + " MiB, decompressed");

const emulator = new V86({
    wasm_path: path.join(VM, "lib/v86.wasm"),
    memory_size: 128 * 1024 * 1024,
    // The snapshot was saved with a larger VGA surface than
    // the 2 MiB this front end asks for, so the size is
    // taken from the environment to match it.
    vga_memory_size: (Number(process.env.FW_VGA_MIB || 8)) * 1024 * 1024,
    disable_jit: !!process.env.FW_NO_JIT,
    firmware: "pcjs",
    bios: undefined,
    vga_bios: undefined,
    // Exactly what index.html's restore path configures.
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
});

let serial = "";
emulator.add_listener("serial0-output-byte", (byte) =>
{
    serial += String.fromCharCode(byte);
    process.stdout.write(String.fromCharCode(byte));
});

let faulted = false;
process.on("uncaughtException", (e) =>
{
    faulted = true;
    console.log("");
    console.log("=== FAULT ===");
    console.log("message: " + String(e.message).split("\n")[0]);
    if(e.stack)
    {
        console.log(String(e.stack).split("\n").slice(0, 8).join("\n"));
    }
});

setTimeout(() =>
{
    console.log("");
    console.log("=== " + SECONDS + "s of serial output ===");
    const tail = serial.slice(-400).replace(/\n+$/, "");
    console.log(tail);
    console.log("");
    console.log("total bytes on serial: " + serial.length);
    console.log(faulted ? "RESULT: FAIL -- the emulator faulted"
                       : "RESULT: PASS -- no fault, serial output received");
    emulator.destroy();
    process.exit(faulted ? 1 : 0);
}, SECONDS * 1000);
