#!/usr/bin/env node
// Boots the PCjs-derived v86 firmware inside the real PCjs x86 CPU (MIT)
// under Node.js, driven by a hand-assembled BIOS self-test boot sector.
//
//   node tests/firmware/run.mjs
//
// Architecture:
//   - CPU:  PCjs CPUx86 (real-mode 80186), the only x86 core involved.
//   - RAM:  PCjs Memoryx86 blocks covering the physical address space.
//   - BIOS: the Rust firmware crate, compiled to wasm32-wasip1, reaching
//           guest state through imported env::v86_* callbacks.
//   - Dispatch: PCjs addIntNotify() hooks the "INT n" opcode. Returning true
//     lets PCjs perform the real INT prologue using the IVT entry the
//     firmware installed (F000:0000, where the harness puts one IRET).
//     Returning false suppresses the ROM trap entirely, which INT 19h needs
//     because the bootstrap never returns to a caller.

import fs from "node:fs";
import url from "node:url";

import CPUx86 from "/Volumes/HOME/Users/dev/work/pcjs/machines/pcx86/modules/v2/cpux86.js";
import Memoryx86 from "/Volumes/HOME/Users/dev/work/pcjs/machines/pcx86/modules/v2/memory.js";
import X86 from "/Volumes/HOME/Users/dev/work/pcjs/machines/pcx86/modules/v2/x86.js";
// Side-effect imports, in dependency order. x86ops.js builds its dispatch
// tables (X86.aOps, X86.aOpGrp*) out of the X86.fn* primitives at module
// evaluation time, so x86func.js and x86help.js MUST be evaluated first or
// the tables capture undefined. The PCjs machine definition relies on its
// own import order for this; a standalone harness has to do it explicitly.
import "/Volumes/HOME/Users/dev/work/pcjs/machines/pcx86/modules/v2/x86func.js";
import "/Volumes/HOME/Users/dev/work/pcjs/machines/pcx86/modules/v2/x86help.js";
import "/Volumes/HOME/Users/dev/work/pcjs/machines/pcx86/modules/v2/x86ops.js";
import "/Volumes/HOME/Users/dev/work/pcjs/machines/pcx86/modules/v2/x86mods.js";
import "/Volumes/HOME/Users/dev/work/pcjs/machines/pcx86/modules/v2/x86op0f.js";

import { buildBootSector } from "./boot-sector.mjs";

const __dirname = url.fileURLToPath(new URL(".", import.meta.url));
const ROOT = __dirname + "/../..";
const WASM_PATH = ROOT + "/build/wasm32-wasip1/release/v86_firmware.wasm";

const BLOCK_SHIFT = 12;
const BLOCK_SIZE = 1 << BLOCK_SHIFT;
const BLOCK_TOTAL = 1024;                 // 4 MiB of physical address space
const ADDR_MASK = 0xFFFFFFFF;

const SEG = { ES: 0, CS: 1, SS: 2, DS: 3, FS: 4, GS: 5 };

// Flag bit positions, matching the firmware's Flag enum.
const FLAG = { CF: 0, PF: 2, AF: 4, ZF: 6, SF: 7, TF: 8, IF: 9, DF: 10, OF: 11 };

// ----------------------------------------------------------------------
// PCjs machine
// ----------------------------------------------------------------------

class PcjsMachine {
    constructor() {
        this.cpu = new CPUx86({ model: X86.MODEL_80186, autoStart: false });

        const blocks = new Array(BLOCK_TOTAL);
        for (let i = 0; i < BLOCK_TOTAL; i++) {
            blocks[i] = new Memoryx86(i * BLOCK_SIZE, BLOCK_SIZE, BLOCK_SIZE,
                                      Memoryx86.TYPE.RAM, null, this.cpu);
        }
        this.cpu.initMemory(blocks, BLOCK_SHIFT);
        this.cpu.setAddressMask(ADDR_MASK);

        // The step loop reaches these only through optional null checks.
        this.cpu.cmp = {
            getMachineComponent: () => null,
            getMachineBoolean: (k, d) => d,
            getMachineParm: () => null,
            getMachineParmValue: () => null,
            getMachineMemorySize: () => BLOCK_TOTAL * BLOCK_SIZE,
            setBinding: () => {},
        };
        this.cpu.bus = {
            saveMemory: () => null,
            restoreMemory: () => true,
            readBackTrack: () => 0,
            writeBackTrack: () => {},
            updateBackTrackCode: () => {},
        };
        this.cpu.chipset = null;
        this.cpu.dbg = null;

        this.keys = [];
        this.resetRequested = false;
        this.rtc = { sec: 30, min: 0, hour: 12, day: 9, month: 10, year: 2026, dow: 5 };

        this.cpu.reset();
        this.cpu.initProcessor();
        // Bind the ModRM decode function pointers. resetSizes() only does
        // this when the operand sizes disagree, which never happens on a
        // freshly reset 16-bit CPU; the real machine gets it via
        // setProtMode()/setDataSize().
        this.cpu.updateAddrSize();
        this.cpu.updateDataSize();
    }

    // ---- memory ----
    read8(addr) { return this.cpu.getByte(addr >>> 0); }
    write8(addr, val) { this.cpu.setByte(addr >>> 0, val & 0xFF); }
    read16(addr) { return this.read8(addr) | (this.read8(addr + 1) << 8); }
    write16(addr, v) { this.write8(addr, v & 0xFF); this.write8(addr + 1, (v >> 8) & 0xFF); }

    // ---- general registers; PCjs indices 0..7 match the Reg enum ----
    readReg(i) { return this.cpu.getReg(i) >>> 0; }
    writeReg(i, v) { this.cpu.setReg(i, v | 0); }

    // ---- segment registers ----
    readSeg(i) {
        switch (i) {
            case SEG.ES: return this.cpu.getES();
            case SEG.CS: return this.cpu.getCS();
            case SEG.SS: return this.cpu.getSS();
            case SEG.DS: return this.cpu.getDS();
            case SEG.FS: return this.cpu.getFS();
            case SEG.GS: return this.cpu.getGS();
        }
        return 0;
    }
    writeSeg(i, v) {
        switch (i) {
            case SEG.ES: this.cpu.setES(v); break;
            case SEG.CS: this.cpu.setCS(v); break;
            case SEG.SS: this.cpu.setSS(v); break;
            case SEG.DS: this.cpu.setDS(v); break;
            case SEG.FS: this.cpu.setFS(v); break;
            case SEG.GS: this.cpu.setGS(v); break;
        }
    }

    // ---- flags ----
    // PCjs models the condition codes as setXXX()/clearXXX() pairs that take
    // NO argument, plus a getXXX() that may recompute a pending arithmetic
    // result. There is no setCF(bool), so route through the pair. TF has no
    // setter at all, so it is edited in regPS directly.
    readFlag(bit) {
        switch (bit) {
            case FLAG.CF: return this.cpu.getCF() ? 1 : 0;
            case FLAG.PF: return this.cpu.getPF() ? 1 : 0;
            case FLAG.AF: return this.cpu.getAF() ? 1 : 0;
            case FLAG.ZF: return this.cpu.getZF() ? 1 : 0;
            case FLAG.SF: return this.cpu.getSF() ? 1 : 0;
            case FLAG.TF: return this.cpu.getTF() ? 1 : 0;
            case FLAG.IF: return this.cpu.getIF() ? 1 : 0;
            case FLAG.DF: return this.cpu.getDF() ? 1 : 0;
            case FLAG.OF: return this.cpu.getOF() ? 1 : 0;
        }
        return 0;
    }
    writeFlag(bit, v) {
        const b = !!v;
        switch (bit) {
            case FLAG.CF: b ? this.cpu.setCF() : this.cpu.clearCF(); break;
            case FLAG.PF: b ? this.cpu.setPF() : this.cpu.clearPF(); break;
            case FLAG.AF: b ? this.cpu.setAF() : this.cpu.clearAF(); break;
            case FLAG.ZF: b ? this.cpu.setZF() : this.cpu.clearZF(); break;
            case FLAG.SF: b ? this.cpu.setSF() : this.cpu.clearSF(); break;
            case FLAG.TF: b ? (this.cpu.regPS |= X86.PS.TF) : (this.cpu.regPS &= ~X86.PS.TF); break;
            case FLAG.IF: b ? this.cpu.setIF() : this.cpu.clearIF(); break;
            case FLAG.DF: b ? this.cpu.setDF() : this.cpu.clearDF(); break;
            case FLAG.OF: b ? this.cpu.setOF() : this.cpu.clearOF(); break;
        }
    }

    readIp() { return this.cpu.getIP() >>> 0; }
    writeIp(v) { this.cpu.setIP(v | 0); }

    rtcField(i) {
        const r = this.rtc;
        return [r.sec, r.min, r.hour, r.day, r.month, r.year, r.dow][i] >>> 0;
    }

    pushKey(scancode, pressed) {
        this.keys.push(((pressed ? 1 : 0) << 8) | scancode);
    }
    pollKey() { return this.keys.length ? this.keys.shift() : 0xFFFF; }

    // ---- 80x25 colour text screen at 0xB8000 ----
    videoText(cols = 80, rows = 25) {
        let out = "";
        for (let row = 0; row < rows; row++) {
            let line = "";
            for (let col = 0; col < cols; col++) {
                line += String.fromCharCode(this.read8(0xB8000 + (row * cols + col) * 2));
            }
            out += line.replace(/\s+$/, "") + "\n";
        }
        return out;
    }
}

// ----------------------------------------------------------------------
// Firmware loader
// ----------------------------------------------------------------------

/**
 * Instantiate the firmware with its guest-state callbacks bound to the live
 * PCjs machine. Only std's startup and panic paths touch WASI proper, so the
 * four preview1 entry points are stubbed.
 */
async function loadFirmware(m) {
    if (!fs.existsSync(WASM_PATH)) {
        throw new Error("firmware wasm not built: " + WASM_PATH +
            "\n  build it with:" +
            "\n    cd src/rust/firmware && cargo build --release --target wasm32-wasip1");
    }
    const bytes = fs.readFileSync(WASM_PATH);

    const env = {
        v86_read8: (a) => m.read8(a),
        v86_write8: (a, v) => m.write8(a, v),
        v86_read_reg: (i) => m.readReg(i),
        v86_write_reg: (i, v) => m.writeReg(i, v),
        v86_read_seg: (i) => m.readSeg(i),
        v86_write_seg: (i, v) => m.writeSeg(i, v),
        v86_read_flag: (i) => m.readFlag(i),
        v86_write_flag: (i, v) => m.writeFlag(i, v),
        v86_read_ip: () => m.readIp(),
        v86_write_ip: (v) => m.writeIp(v),
        v86_rtc: (i) => m.rtcField(i),
        v86_poll_key: () => m.pollKey(),
        v86_reset: () => { m.resetRequested = true; },
    };

    const wasi = {
        environ_get: () => 0,
        environ_sizes_get: () => 0,
        fd_write: () => 0,
        proc_exit: (c) => { throw new Error("firmware called proc_exit(" + c + ")"); },
    };

    const { instance } = await WebAssembly.instantiate(bytes, {
        env,
        wasi_snapshot_preview1: wasi,
    });
    return instance.exports;
}

// ----------------------------------------------------------------------
// The test
// ----------------------------------------------------------------------

async function main() {
    const m = new PcjsMachine();
    const api = await loadFirmware(m);
    const sector = buildBootSector();

    // ---- firmware instance ----
    const fwPtr = api.fw_create();
    api.fw_set_memory_kib(fwPtr, 640);
    api.fw_install_vbe(fwPtr);

    // A 1.44M floppy carrying the self-test boot sector.
    const drive = api.fw_add_floppy(fwPtr, 2880);
    if (drive === 0xFF) throw new Error("could not add a floppy drive");
    {
        const scratch = api.fw_scratch_ptr(fwPtr);
        const view = new Uint8Array(api.memory.buffer, scratch, sector.bytes.length);
        view.set(sector.bytes);
        if (!api.fw_load_scratch(fwPtr, drive, sector.bytes.length)) {
            throw new Error("could not write the boot sector into the floppy");
        }
    }

    // ---- reset, then POST (a real PC resets registers, then runs POST) ----
    m.cpu.reset();
    m.cpu.initProcessor();
    api.fw_post(fwPtr);

    // The boot sector's message strings live in low RAM (the 512-byte sector is
    // nearly full of code), so plant them the way a boot loader would.
    for (const str of sector.strings) {
        for (let i = 0; i <= str.text.length; i++) {
            m.write8(str.addr + i, i < str.text.length ? str.text.charCodeAt(i) : 0);
        }
    }

    // POST installed IVT entries at F000:0000; put one IRET there so every
    // serviced interrupt returns to its caller.
    m.write8(0xF0000, 0xCF);

    // An IVT entry is a far pointer: offset in the low word, segment in the
    // high word. POST must have pointed these at the firmware marker F000:0000.
    const ivt = (v) => ((m.read16(v * 4 + 2) << 16) | m.read16(v * 4)) >>> 0;
    for (const v of [0x10, 0x13, 0x15, 0x19]) {
        const p = ivt(v);
        const ok = p === 0xF0000000;
        console.log("IVT[0x" + v.toString(16).toUpperCase() + "] = " +
            (p >>> 16).toString(16).toUpperCase().padStart(4, "0") + ":" +
            (p & 0xFFFF).toString(16).toUpperCase().padStart(4, "0") +
            (ok ? "  (firmware marker)" : "  (UNEXPECTED)"));
    }
    console.log("BDA memory size  =", m.read16(0x413), "KiB");
    console.log("BDA equipment    = 0x" + m.read16(0x410).toString(16));
    console.log("VBE signature    =", String.fromCharCode(
        m.read8(0xC8000), m.read8(0xC8001), m.read8(0xC8002), m.read8(0xC8003)));

    // ---- INT dispatch hooks ----
    const serviced = new Map();
    for (const vec of [0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x19, 0x1A]) {
        m.cpu.addIntNotify(vec, () => {
            api.fw_interrupt(fwPtr, vec);
            serviced.set(vec, (serviced.get(vec) || 0) + 1);
            if (process.env.FW_TRACE_INT) {
                console.log("    INT 0x" + vec.toString(16) + ": AX=" +
                    m.readReg(0).toString(16).padStart(4, "0") + " BX=" +
                    m.readReg(3).toString(16).padStart(4, "0") + " CX=" +
                    m.readReg(1).toString(16).padStart(4, "0") + " DX=" +
                    m.readReg(2).toString(16).padStart(4, "0") + " CF=" +
                    (m.readFlag(0) ? 1 : 0) + " ES=" + m.readSeg(0) + " DI=" +
                    m.readReg(7).toString(16));
            }
            // INT 19h never returns: the firmware has already loaded a boot
            // sector and set CS:IP, so suppress the ROM trap entirely.
            return vec !== 0x19;
        });
    }

    // ---- enter at the real reset vector, F000:FFF0 (physical 0xFFFF0) ----
    m.cpu.setCS(0xF000);
    m.cpu.setIP(0xFFF0);
    m.cpu.setSS(0);
    m.cpu.setSP(0xFFFE);
    m.cpu.setDS(0);
    m.cpu.setES(0);
    // F000:FFF0 -> int 19h ; jmp $   (a bootstrap that never returns)
    m.write8(0xFFFF0, 0xCD);
    m.write8(0xFFFF1, 0x19);
    m.write8(0xFFFF2, 0xEB);
    m.write8(0xFFFF3, 0xFD);

    // ---- run ----
    const MAX_STEPS = 2_000_000;
    let steps = 0;
    let stalled = 0;
    let booted = false;

    while (steps < MAX_STEPS) {
        const before = m.cpu.getIP();
        try {
            m.cpu.stepCPU(64);
        } catch (e) {
            console.error("CPU fault at CS:IP = 0x" +
                m.cpu.getCS().toString(16) + ":0x" + m.cpu.getIP().toString(16) +
                " -- " + e.message);
            if (e && e.stack) console.error(e.stack.split("\n").slice(0, 8).join("\n"));
            throw e;
        }
        steps++;
        if (!booted && m.cpu.getCS() === 0x07C0) booted = true;
        if (m.cpu.getIP() === before) stalled++; else stalled = 0;
        if (stalled > 200) break;    // the boot sector's final "jmp $"
    }

    console.log("--- guest screen ---");
    process.stdout.write(m.videoText());

    const fail = m.read8(sector.failAddr);

    console.log("--- results ---");
    console.log("boot sector loaded :", booted);
    console.log("instructions stepped:", steps);
    console.log("checks run / failed:", sector.checks, "/", fail);
    console.log("interrupts serviced:",
        [...serviced.entries()].sort((a, b) => a[0] - b[0])
            .map(([v, n]) => "INT 0x" + v.toString(16).toUpperCase() + " x" + n).join(", "));
    console.log("final CS:IP = 0x" + m.cpu.getCS().toString(16) +
                ":0x" + m.cpu.getIP().toString(16));

    api.fw_destroy(fwPtr);

    if (!booted || fail !== 0) {
        console.error("\nFAIL: the BIOS self-test did not pass cleanly");
        process.exit(1);
    }
    console.log("\nPASS: PCjs-derived firmware booted from floppy and passed all BIOS self-tests");
}

main().catch((e) => {
    console.error(e);
    process.exit(1);
});