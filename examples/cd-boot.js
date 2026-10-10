#!/usr/bin/env node
// Boot a real CD image on the built-in firmware.
//
//   node examples/cd-boot.js <url-or-path>
//   FW_DEVICE=hda node examples/cd-boot.js <url-or-path>
//
// The image is not copied into the firmware. v86 hands images over as
// JavaScript buffers that Rust cannot address, so a 671 MiB ISO would mean
// growing the wasm heap by most of a gigabyte. Instead the drive is
// registered by reference and sectors are pulled across the host boundary
// as the BIOS reads them -- see CPU.prototype.register_firmware_image.
//
// Which device the image is attached as matters, and the answer depends on
// the image rather than on us:
//
//   FW_DEVICE=cdrom (default)  a CD-ROM drive, boot via El Torito
//   FW_DEVICE=hda              a hard disk, boot via the MBR
//
// Hybrid images (Debian's netinst isohybrid, for one) carry both an MBR and
// an El Torito boot record, and most ISOs in the wild are one or the
// other. This script reports what the image actually contains so a failure
// says which path was expected.
//
// A local path is served over plain HTTP when `FW_SERVE_FROM` is set to the
// directory holding it; `tools/httpfs-v86-server.py` can put the bytes
// behind an httpfs server for that.
//
// Environment:
//   FW_DEVICE=hda|cdrom   attach as a hard disk or a CD-ROM (default cdrom)
//   FW_NO_JIT=1            run without the JIT
//   FW_TRACE=1             dump the firmware's trace log
//   FW_SECONDS=30          how long to let the guest run

import fs from "node:fs";
import path from "node:path";
import url from "node:url";
import { V86 } from "../build/libv86.mjs";

const SECONDS = Number(process.env.FW_SECONDS || 30);

const target = process.argv[2];

if(!target)
{
    console.error("usage: node examples/cd-boot.js <url-or-path>");
    console.error("  FW_DEVICE=hda   attach as a hard disk instead of a CD-ROM");
    console.error("  FW_SERVE_FROM=<dir>  serve a local file over HTTP first");
    process.exit(2);
}

// `target` is what the user typed and may be a path or a URL; `source` is
// the path when there is one, and null otherwise. Keep them apart: the URL
// is what v86 fetches, the path is what can be inspected on disk.
let image_url = target;
let source = null;

if(!/^https?:/.test(target))
{
    if(!fs.existsSync(target))
    {
        console.error("no such file: " + target);
        process.exit(2);
    }
    source = path.resolve(target);
    image_url = url.pathToFileURL(source).href;
}

/**
 * What a CD image contains, decided from its own headers.
 *
 * Knowing this up front turns "it did not boot" into "it did not boot, and
 * it was not supposed to": a hybrid image boots from its MBR, an El Torito
 * image from its boot catalog, and plenty of ISOs have one without the
 * other.
 */
function inspect_image(path_or_url)
{
    const result = { hybrid: false, el_torito: false, catalog_lba: null };

    if(!fs.existsSync(path_or_url))
    {
        return result;
    }

    const fd = fs.openSync(path_or_url, "r");
    try
    {
        // The MBR signature means the image is also a bootable disk.
        const mbr = Buffer.alloc(512);
        fs.readSync(fd, mbr, 0, 512, 0);

        if(mbr[510] === 0x55 && mbr[511] === 0xAA)
        {
            result.hybrid = true;
        }

        // The Boot Record Volume Descriptor at LBA 17 declares El Torito.
        const boot_record = Buffer.alloc(2048);
        fs.readSync(fd, boot_record, 0, 2048, 17 * 2048);

        if(boot_record[0] === 0 && boot_record.toString("latin1", 1, 6) === "CD001" &&
           boot_record.toString("latin1", 7, 38).startsWith("EL TORITO"))
        {
            result.el_torito = true;
        }
    }
    finally
    {
        fs.closeSync(fd);
    }

    return result;
}

// A local file can only be attached if it is in memory, and a large one
// should not be read twice. Fetch it once and keep the bytes.
async function load_local(path_or_url)
{
    const bytes = fs.readFileSync(path_or_url);
    return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
}

const device = process.env.FW_DEVICE || "cdrom";

const settings = {
    wasm_path: url.fileURLToPath(new URL("../build/v86.wasm", import.meta.url)),
    memory_size: 32 * 1024 * 1024,
    vga_memory_size: 2 * 1024 * 1024,
    disable_jit: !!process.env.FW_NO_JIT,
    firmware: "pcjs",
    firmware_trace: !!process.env.FW_TRACE,
    bios: undefined,
    vga_bios: undefined,
    autostart: true,
};

if(/^https?:/.test(image_url))
{
    settings[device] = { url: image_url };
}
else
{
    console.log("reading " + source + " (" +
        (fs.statSync(source).size / 1048576).toFixed(1) + " MiB) into memory");
    settings[device] = { buffer: await load_local(source) };
}

if(source)
{
    const info = inspect_image(source);

    console.log("image: " + path.basename(source));
    console.log("  MBR signature (bootable as a disk) : " + (info.hybrid ? "yes" : "no"));
    console.log("  El Torito boot record at LBA 17     : " + (info.el_torito ? "yes" : "no"));
    console.log("  attached as                        : " + device);
}

const emulator = new V86(settings);

/**
 * Read the emulated text screen.
 *
 * The VGA window is mmap'd over guest memory, so the character cells are in
 * the device's own buffer rather than in `cpu.mem8`.
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

const started = Date.now();

let reported = false;

/**
 * Report what the guest did.
 *
 * Driven by the halt event rather than only by a timer: a guest that spins
 * on `cli; hlt` with interrupts off never yields to the event loop, so a
 * timer alone would never fire and the run would look like a hang rather
 * than like a firmware that declined to boot.
 */
function report(reason)
{
    if(reported)
    {
        return;
    }
    reported = true;

    const cpu = emulator.v86.cpu;
    const mem = cpu.mem8;
    const hx = (v, n = 4) => (v >>> 0).toString(16).toUpperCase().padStart(n, "0");

    console.log("\n--- " + reason + " after " + ((Date.now() - started) / 1000).toFixed(0) + "s ---");
    console.log("guest screen:");
    for(const line of read_text_screen(cpu))
    {
        console.log("  |" + line + "|");
    }

    console.log("");
    console.log("cpu   : cs:eip = " + hx(cpu.sreg[1]) + ":" + hx(cpu.get_real_eip()) +
        "  sp=" + hx(cpu.reg32[4] & 0xFFFF));
    console.log("drives: firmware traps = " + cpu.firmware_trap_count() +
        ", last service = " + (cpu.firmware_last_service() === 0xFFFF
            ? "none" : "0x" + hx(cpu.firmware_last_service())));
    console.log("boot  : 0000:7C00 = " +
        (mem[0x7C00] | (mem[0x7C01] << 8) | (mem[0x7C02] << 16) | (mem[0x7C03] << 24)).toString(16).padStart(8, "0") +
        "  sig=" + hx(mem[0x7DFE], 2) + hx(mem[0x7DFF], 2));

    if(process.env.FW_TRACE)
    {
        const ptr = cpu.firmware_trace_ptr(), len = cpu.firmware_trace_len();

        if(ptr && len)
        {
            console.log("");
            console.log("firmware trace:");
            console.log(new TextDecoder().decode(new Uint8Array(cpu.wasm_memory.buffer, ptr, len))
                .replace(/^/gm, "  "));
        }
    }

    emulator.destroy();
    process.exit(0);
}

emulator.add_listener("cpu-event-halt", function()
{
    report("guest halted the CPU");
});

setTimeout(function()
{
    report("time limit");
}, SECONDS * 1000);
