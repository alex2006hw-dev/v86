// Ask a BIOS what it actually returns for a service, instead of arguing
// about the specification.
//
//   node examples/firmware-service-probe.mjs
//   FW_ORACLE=bochs node examples/firmware-service-probe.mjs
//   FW_ORACLE=none node examples/firmware-service-probe.mjs    (built-in)
//
// Boots a small hand-assembled boot sector that calls a service, records
// the registers it returns in low memory, and halts. The emulator reads the
// recorded words back out of guest memory, so nothing depends on the guest's
// idea of how to print.
//
// This exists because a specification and an implementation can disagree, and
// only one of them can be booted. It found a check in
// `examples/firmware-selftest.mjs` that passed against the built-in firmware
// for the wrong reason and failed against SeaBIOS and the Bochs BIOS.
//
// The reference images under `bios/` are GPL-2.0-or-later / LGPL-3.0. They
// are queried here as a behavioural oracle only -- nothing is copied out of
// them or disassembled into. See `bios/README.md`.

import fs from "node:fs";
import path from "node:path";
import url from "node:url";
import { V86 } from "../build/libv86.mjs";
import { Asm } from "./firmware-selftest.mjs";

const ROOT = path.resolve(path.dirname(url.fileURLToPath(import.meta.url)), "..");

const SECTOR = 512;
// Where the probe records its results: segment 0x0300, nowhere near the boot
// sector, the BIOS data area or the stack.
const RESULT_SEG = 0x0300;
const RESULT_BASE = RESULT_SEG << 4;

// Result words, in the order `describe()` prints them.
const OFF_CF = 0x00, OFF_AX = 0x02, OFF_BX = 0x04, OFF_CX = 0x06, OFF_DX = 0x08;

/**
 * The services worth a second opinion.
 *
 * Each one runs the same call sequence, then parks the returned registers and
 * the carry flag where the host can read them.
 */
const SERVICES = {
    "int13-edd-install-check": { vector: 0x13, ax: 0x4100, bx: 0x55AA, cx: 0x0000, dl: 0x00 },
    "int13-edd-get-params": { vector: 0x13, ax: 0x4800, bx: 0, cx: 0, dl: 0x00 },
    "int13-geometry": { vector: 0x13, ax: 0x0800, bx: 0, cx: 0, dl: 0x00 },
    "int13-reset": { vector: 0x13, ax: 0x0000, bx: 0, cx: 0, dl: 0x00 },
    "int12-memory": { vector: 0x12, ax: 0, bx: 0, cx: 0, dl: 0 },
    "int11-equipment": { vector: 0x11, ax: 0, bx: 0, cx: 0, dl: 0 },
    "int10-columns": { vector: 0x10, ax: 0x0F00, bx: 0, cx: 0, dl: 0 },
    "int15-a20-query": { vector: 0x15, ax: 0x2402, bx: 0, cx: 0, dl: 0 },
    "int1a-rtc-date": { vector: 0x1A, ax: 0x0200, bx: 0, cx: 0, dl: 0 },
};

const ORACLES = {
    none: null,
    seabios: { bios: "bios/seabios.bin", vga: "bios/vgabios.bin" },
    bochs: { bios: "bios/bochs-bios.bin", vga: "bios/bochs-vgabios.bin" },
};

/**
 * Assemble a boot sector that calls one service and records the result.
 *
 * It never prints: the point is to be readable from the host, so a BIOS that
 * hangs, renders nothing, or draws over its own output still produces an
 * answer.
 */
function build_probe_floppy(service)
{
    const a = new Asm(SECTOR);

    a.label("start");
    a.cli();
    a.xor_ax_ax();
    a.mov_seg_ax(3);                 // DS
    a.mov_seg_ax(0);                 // ES
    a.mov_seg_ax(2);                 // SS
    a.mov_r16(4, 0x7000);            // SP
    a.sti();

    // DS = the result segment, so the stores below land where we want.
    a.mov_r16(0, RESULT_SEG);
    a.mov_seg_ax(3);

    // Set up the request registers.
    a.mov_r16(0, service.ax);
    a.mov_r16(3, service.bx);
    a.mov_r16(1, service.cx);
    a.mov_dl(service.dl);
    a.int(service.vector);

    // Record the registers the call returned *first*. AX is the transport
    // for the carry flag below, so reading it afterwards would record the
    // pushed FLAGS and report a version number where AH should be.
    a.mov_moffs_reg(OFF_AX, 0);
    a.mov_moffs_reg(OFF_BX, 3);
    a.mov_moffs_reg(OFF_CX, 1);
    a.mov_moffs_reg(OFF_DX, 2);

    // Then the carry flag, while nothing else has touched it.
    a.pushf();
    a.pop_ax();
    a.mov_moffs_reg(OFF_CF, 0);

    a.label("done");
    a.cli();
    a.hlt();
    a.b(0xEB, 0xFD);                 // jmp $

    const code = a.link();
    const image = new Uint8Array(2880 * SECTOR);
    image.set(code, 0);
    image[SECTOR - 2] = 0x55;
    image[SECTOR - 1] = 0xAA;
    return image.buffer;
}

function describe(regs)
{
    const hx = v => "0x" + (v & 0xFFFF).toString(16).toUpperCase().padStart(4, "0");
    return `CF=${regs.cf}  AX=${hx(regs.ax)}  BX=${hx(regs.bx)}  `
        + `CX=${hx(regs.cx)}  DX=${hx(regs.dx)}`;
}

/**
 * Boot one probe sector and read the recorded registers back.
 *
 * `oracle` is null for the built-in firmware, which is selected with the
 * `firmware` option rather than by loading a ROM.
 */
function run_probe(oracle, service)
{
    return new Promise(resolve =>
    {
        const settings = {
            wasm_path: url.fileURLToPath(new URL("../build/v86.wasm", import.meta.url)),
            memory_size: 32 * 1024 * 1024,
            vga_memory_size: 2 * 1024 * 1024,
            disable_jit: !!process.env.FW_NO_JIT,
            fda: { buffer: build_probe_floppy(service) },
            autostart: true,
        };

        if(oracle)
        {
            settings.bios = { url: path.join(ROOT, oracle.bios) };
            settings.vga_bios = { url: path.join(ROOT, oracle.vga) };
        }
        else
        {
            settings.firmware = "pcjs";
        }

        const emulator = new V86(settings);

        setTimeout(() =>
        {
            const mem = emulator.v86.cpu.mem8;
            const w = a => mem[a] | (mem[a + 1] << 8);

            const regs = {
                cf: mem[RESULT_BASE + OFF_CF] & 1,
                ax: w(RESULT_BASE + OFF_AX),
                bx: w(RESULT_BASE + OFF_BX),
                cx: w(RESULT_BASE + OFF_CX),
                dx: w(RESULT_BASE + OFF_DX),
            };

            emulator.destroy();
            resolve(regs);
        }, 5000);
    });
}

const only = process.argv[2];
const which = process.env.FW_ORACLE || "none";

// `ORACLES.none` is null on purpose, so test for the key rather than the value.
if(!Object.prototype.hasOwnProperty.call(ORACLES, which))
{
    console.error("unknown oracle " + which + "; try one of: " + Object.keys(ORACLES).join(", "));
    process.exit(2);
}

const names = only ? [only] : Object.keys(SERVICES);

for(const name of names)
{
    const service = SERVICES[name];

    if(!service)
    {
        console.error("unknown service " + name);
        process.exit(2);
    }

    const regs = await run_probe(ORACLES[which], service);
    console.log(name.padEnd(26) + describe(regs));
}

process.exit(0);
