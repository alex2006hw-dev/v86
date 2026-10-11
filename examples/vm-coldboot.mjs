// Cold-boot the vm front end's guest and report what it prints.
//
//   node examples/vm-coldboot.mjs [seconds]
//
// This is the vm front end's `?boot=true` path -- the only one that can
// run with the built-in firmware, because the warm-boot snapshot was
// saved with the CPU halted and restoring it does not resume execution
// (VM-1). It boots the bzimage from the 9p filesystem, which is how a
// real cold boot of this guest works, and watches the serial line, which
// is the only output it has.
//
// Unlike the snapshot path, this needs the 9p filesystem served over
// HTTP, because `baseurl` is fetched rather than read from disk:
//
//     $ (cd vm && python3 -m http.server 8123)
//     $ VM_URL=http://127.0.0.1:8123 node examples/vm-coldboot.mjs 120
//
// Everything is taken from vm/index.html, so a difference between the two
// is in the page rather than in the settings.

import path from "node:path";
import url from "node:url";
import { V86 } from "../build/libv86.mjs";

const SECONDS = Number(process.argv[2] || 90);
const ROOT = path.resolve(path.dirname(url.fileURLToPath(import.meta.url)), "..");
const VM = path.join(ROOT, "vm");
const VM_URL = process.env.VM_URL || "http://127.0.0.1:8123";

const emulator = new V86({
    wasm_path: path.join(VM, "lib/v86.wasm"),
    memory_size: 128 * 1024 * 1024,
    vga_memory_size: 8 * 1024 * 1024,
    filesystem: {
        basefs: VM_URL + "/filesystem/filesystem.json",
        baseurl: VM_URL + "/filesystem/",
    },
    // The guest is headless: serial is the console, and the screen stays
    // off because there is no ScreenAdapter in Node.
    firmware: "pcjs",
    bios: undefined,
    vga_bios: undefined,
    bzimage: { url: VM_URL + "/filesystem/29a77969.bin" },
    cmdline: [
        "rw",
        "root=host9p rootfstype=9p rootflags=version=9p2000.L,trans=virtio,cache=loose quiet acpi=off",
        "console=ttyS0 tsc=reliable mitigations=off random.trust_cpu=on nowatchdog page_poison=on",
    ].join(" "),
    autostart: true,
    disable_keyboard: true,
    disable_mouse: true,
    disable_speaker: true,
    acpi: true,
});

let serial = "";
const started = Date.now();
let last_stamp = started;

emulator.add_listener("serial0-output-byte", (byte) =>
{
    const ch = String.fromCharCode(byte);

    serial += ch;

    // A timestamp every few seconds so a slow boot says where it spent
    // its time, which a bare log does not.
    if(Date.now() - last_stamp > 5000)
    {
        last_stamp = Date.now();
        process.stderr.write("\n[" + ((Date.now() - started) / 1000).toFixed(0) + "s] ");
    }

    process.stdout.write(ch);
});

process.on("uncaughtException", (e) =>
{
    console.log("");
    console.log("=== FAULT ===");
    console.log(String(e.message).split("\n")[0]);
});

setTimeout(() =>
{
    console.log("");
    console.log("=== " + ((Date.now() - started) / 1000).toFixed(0) + "s elapsed ===");
    console.log("serial bytes: " + serial.length);

    try
    {
        const cpu = emulator.v86.cpu;
        console.log("cpu     : cs:eip = " + cpu.sreg[1].toString(16) + ":" + cpu.get_real_eip().toString(16));
        console.log("firmware: present = " + cpu.firmware_present() +
            ", traps = " + cpu.firmware_trap_count() +
            ", last service = " + cpu.firmware_last_service().toString(16));
    }
    catch(e)
    {
        console.log("(state not readable: " + String(e.message).split("\n")[0] + ")");
    }

    // The kernel prints to the VGA screen as well as the serial line, and
    // the early console is on whichever the cmdline names first. The
    // screen is the more reliable of the two here.
    try
    {
        const vga = emulator.v86.cpu.devices.vga;
        const mem = vga.vga_memory;
        const space = [0xA0000, 0xA0000, 0xB0000, 0xB8000][(vga.miscellaneous_graphics_register >> 2) & 3];
        const base = (0xB8000 - space) + ((vga.start_address || 0) << 1);
        const cols = vga.max_cols || 80;
        const lines = [];

        for(let row = 0; row < 25; row++)
        {
            let line = "";

            for(let col = 0; col < cols; col++)
            {
                line += String.fromCharCode(mem[base + (row * cols + col) * 2]);
            }

            lines.push(line.replace(/\s+$/, ""));
        }

        console.log("--- vga text screen ---");
        console.log(lines.filter(l => l.length).join("\n") || "(blank)");
    }
    catch(e)
    {
        console.log("(vga not readable: " + String(e.message).split("\n")[0] + ")");
    }

    emulator.destroy();
    process.exit(0);
}, SECONDS * 1000);
