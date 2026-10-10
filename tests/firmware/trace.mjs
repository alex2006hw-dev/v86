// Instruction-level trace of the firmware boot, for debugging.
import fs from "node:fs";
import url from "node:url";
import CPUx86 from "/Volumes/HOME/Users/dev/work/pcjs/machines/pcx86/modules/v2/cpux86.js";
import Memoryx86 from "/Volumes/HOME/Users/dev/work/pcjs/machines/pcx86/modules/v2/memory.js";
import X86 from "/Volumes/HOME/Users/dev/work/pcjs/machines/pcx86/modules/v2/x86.js";
// Side-effect imports, in dependency order. x86ops.js builds its dispatch
// tables (X86.aOps, X86.aOpGrp*) out of the X86.fn* primitives at module
// evaluation time, so x86func.js and x86help.js must be evaluated FIRST or
// the tables capture undefined. The PCjs machine definition relies on its
// own import order to get this right; a standalone harness must do it too.
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
const WASM = __dirname + "/../../build/wasm32-wasip1/release/v86_firmware.wasm";

const BLOCK_SHIFT = 12, BLOCK_SIZE = 4096, BLOCK_TOTAL = 1024;
const SEG = { ES: 0, CS: 1, SS: 2, DS: 3, FS: 4, GS: 5 };
const FLAG = { CF: 0, PF: 2, AF: 4, ZF: 6, SF: 7, TF: 8, IF: 9, DF: 10, OF: 11 };

const cpu = new CPUx86({ model: X86.MODEL_80186, autoStart: false });
const blocks = new Array(BLOCK_TOTAL);
for (let i = 0; i < BLOCK_TOTAL; i++)
    blocks[i] = new Memoryx86(i * BLOCK_SIZE, BLOCK_SIZE, BLOCK_SIZE, Memoryx86.TYPE.RAM, null, cpu);
cpu.initMemory(blocks, BLOCK_SHIFT);
cpu.setAddressMask(0xFFFFFFFF);
cpu.cmp = { getMachineComponent: () => null, getMachineBoolean: (k, d) => d, getMachineParm: () => null,
            getMachineParmValue: () => null, getMachineMemorySize: () => BLOCK_TOTAL * BLOCK_SIZE, setBinding: () => {} };
cpu.bus = { saveMemory: () => null, restoreMemory: () => true, readBackTrack: () => 0,
            writeBackTrack: () => {}, updateBackTrackCode: () => {} };
cpu.chipset = null; cpu.dbg = null;

const m = {
    read8: (a) => cpu.getByte(a >>> 0),
    write8: (a, v) => cpu.setByte(a >>> 0, v & 0xFF),
    readReg: (i) => cpu.getReg(i) >>> 0,
    writeReg: (i, v) => cpu.setReg(i, v | 0),
    readSeg(i) { switch (i) { case 0: return cpu.getES(); case 1: return cpu.getCS();
        case 2: return cpu.getSS(); case 3: return cpu.getDS();
        case 4: return cpu.getFS(); case 5: return cpu.getGS(); } return 0; },
    writeSeg(i, v) { switch (i) { case 0: cpu.setES(v); break; case 1: cpu.setCS(v); break;
        case 2: cpu.setSS(v); break; case 3: cpu.setDS(v); break;
        case 4: cpu.setFS(v); break; case 5: cpu.setGS(v); break; } },
    readFlag(b) { switch (b) { case 0: return cpu.getCF() ? 1 : 0; case 2: return cpu.getPF() ? 1 : 0;
        case 4: return cpu.getAF() ? 1 : 0; case 6: return cpu.getZF() ? 1 : 0; case 7: return cpu.getSF() ? 1 : 0;
        case 8: return cpu.getTF() ? 1 : 0; case 9: return cpu.getIF() ? 1 : 0;
        case 10: return cpu.getDF() ? 1 : 0; case 11: return cpu.getOF() ? 1 : 0; } return 0; },
    writeFlag(b, v) { const x = !!v; switch (b) {
        case 0: x ? cpu.setCF() : cpu.clearCF(); break;
        case 2: x ? cpu.setPF() : cpu.clearPF(); break;
        case 4: x ? cpu.setAF() : cpu.clearAF(); break;
        case 6: x ? cpu.setZF() : cpu.clearZF(); break;
        case 7: x ? cpu.setSF() : cpu.clearSF(); break;
        case 8: x ? (cpu.regPS |= X86.PS.TF) : (cpu.regPS &= ~X86.PS.TF); break;
        case 9: x ? cpu.setIF() : cpu.clearIF(); break;
        case 10: x ? cpu.setDF() : cpu.clearDF(); break;
        case 11: x ? cpu.setOF() : cpu.clearOF(); break; } },
    readIp: () => cpu.getIP() >>> 0,
    writeIp: (v) => cpu.setIP(v | 0),
    rtc: (i) => [30, 0, 12, 9, 10, 2026, 5][i],
    keys: [],
};
m.pollKey = () => m.keys.length ? m.keys.shift() : 0xFFFF;
void SEG; void FLAG;

const env = {
    v86_read8: m.read8, v86_write8: m.write8,
    v86_read_reg: m.readReg, v86_write_reg: m.writeReg,
    v86_read_seg: m.readSeg, v86_write_seg: m.writeSeg,
    v86_read_flag: m.readFlag, v86_write_flag: m.writeFlag,
    v86_read_ip: m.readIp, v86_write_ip: m.writeIp,
    v86_rtc: m.rtc, v86_poll_key: m.pollKey, v86_reset: () => {},
};
const wasi = { environ_get: () => 0, environ_sizes_get: () => 0, fd_write: () => 0, proc_exit: () => {} };
const api = (await WebAssembly.instantiate(fs.readFileSync(WASM), { env, wasi_snapshot_preview1: wasi }))
    .instance.exports;

cpu.reset(); cpu.initProcessor(); cpu.updateAddrSize(); cpu.updateDataSize();

const sector = buildBootSector();
const fw = api.fw_create();
api.fw_set_memory_kib(fw, 640);
api.fw_install_vbe(fw);
const drive = api.fw_add_floppy(fw, 2880);
{
    const p = api.fw_scratch_ptr(fw);
    new Uint8Array(api.memory.buffer, p, 512).set(sector.bytes);
    api.fw_load_scratch(fw, drive, 512);
}
api.fw_post(fw);
m.write8(0xF0000, 0xCF);

const LIMIT = Number(process.env.TRACE_N || 120);
let n = 0;
for (const v of [0x10, 0x11, 0x12, 0x13, 0x15, 0x16, 0x19, 0x1A]) {
    cpu.addIntNotify(v, () => {
        const lip = cpu.getIP();
        console.log("      >>> INT 0x" + v.toString(16).toUpperCase() +
            " at CS:IP=" + cpu.getCS().toString(16) + ":" + lip.toString(16) +
            " AX=" + cpu.getReg(0).toString(16) + " SP=" + cpu.getSP().toString(16));
        api.fw_interrupt(fw, v);
        return v !== 0x19;
    });
}

cpu.setCS(0xF000); cpu.setIP(0xFFF0);
cpu.setSS(0); cpu.setSP(0xFFFE); cpu.setDS(0); cpu.setES(0);
m.write8(0xFFFF0, 0xCD); m.write8(0xFFFF1, 0x19);
m.write8(0xFFFF2, 0xEB); m.write8(0xFFFF3, 0xFD);

for (let i = 0; i < LIMIT; i++) {
    const cs = cpu.getCS(), ip = cpu.getIP();
    const lin = (cs << 4) + ip;
    console.log("[" + String(i).padStart(4) + "] " + cs.toString(16).padStart(4, "0") + ":" +
        ip.toString(16).padStart(4, "0") + "  " +
        [...Array(6)].map((_, k) => m.read8(lin + k).toString(16).padStart(2, "0")).join(" ") +
        "   AX=" + (cpu.getReg(0) & 0xFFFF).toString(16).padStart(4, "0") +
        "  SS:SP=" + cpu.getSS().toString(16) + ":" + cpu.getSP().toString(16));
    try { cpu.stepCPU(Number(process.env.STEP_CYCLES || 64)); } catch (e) { console.log("  !! " + e.message); console.log(e.stack.split("\n").slice(0,8).join("\n")); break; }
    void n; n++;
}
console.log("checks=" + sector.checks + " failures=" + m.read8(sector.failAddr));
let line = "";
for (let c = 0; c < 40; c++) line += String.fromCharCode(m.read8(0xB8000 + c * 2));
console.log("screen: " + JSON.stringify(line));