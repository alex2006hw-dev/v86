// Validate that a change to VM state survives a save/restore round-trip.
//
//   node examples/vm-state-check.mjs
//
// The question this answers: if the machine's state is changed and then
// saved, does reloading the saved state bring the change back, or does it
// bring back the pre-change machine?
//
// The guest is a boot sector that writes a marker into guest memory --
// the same thing a database does when it commits: the change lives in
// the machine's RAM, not in a file. A second value beside it stands in
// for the part a state save is expected to capture verbatim.
//
// The round-trip is then:
//
//   boot                        the guest writes its marker
//   change marker in place      a "committed transaction" made in RAM
//   save_state()                what the page's Save state button does
//   restore_state()             into a fresh emulator, as the page does
//   read marker back            it must be the changed value
//
// Restoring into a *fresh* emulator is the point: it rules out a run
// that keeps the old object and never restores anything.
//
// Both configurations are checked, JIT on and JIT off, because the
// translator is the part of v86 that could plausibly disagree with
// itself about what a restore leaves behind.

import path from "node:path";
import url from "node:url";
import { V86 } from "../build/libv86.mjs";

const ROOT = path.resolve(path.dirname(url.fileURLToPath(import.meta.url)), "..");

// Where the guest puts its marker, and the two values it writes. This is
// scratch memory above the IVT and the BDA, where a BIOS puts working
// data and nothing else claims.
const MARKER = 0x500;
const GUEST_MAGIC = 0x5AA5;   // written by the guest at boot
const VICINITY = 0x55AA;      // written by the guest at boot, beside it

// What the host changes the first value to, modelling a transaction the
// guest commits before the machine is saved.
const CHANGED = 0x1234;

/**
 * Build a floppy whose boot sector writes a marker into memory.
 *
 * Hand-assembled so this example needs no assembler toolchain and no
 * disk image, which is the same constraint `firmware-selftest.mjs`
 * answers the same way.
 *
 * @return {ArrayBuffer} a 1.44 MiB floppy image
 */
function build_floppy()
{
    const code = [];

    // mov ax, GUEST_MAGIC     ; B8 lo hi
    code.push(0xB8, GUEST_MAGIC & 0xFF, GUEST_MAGIC >> 8);
    // mov [0x500], ax         ; A3 00 05
    code.push(0xA3, MARKER & 0xFF, MARKER >> 8);
    // mov ax, VICINITY        ; B8 lo hi
    code.push(0xB8, VICINITY & 0xFF, VICINITY >> 8);
    // mov [0x502], ax         ; A3 02 05
    code.push(0xA3, (MARKER + 2) & 0xFF, (MARKER + 2) >> 8);

    // mov si, message         ; BE lo hi -- the offset is patched below
    code.push(0xBE, 0, 0);
    const message_slot = code.length - 2;

    // print loop: lodsb; or al,al; jz done; mov ah,0Eh; int 10h; jmp
    const loop_start = code.length;
    code.push(0xAC);                 // lodsb
    code.push(0x0C, 0x00);           // or al, al
    code.push(0x74, 0x05);           // jz +5 -> done
    code.push(0xB4, 0x0E);           // mov ah, 0Eh
    code.push(0xCD, 0x10);           // int 10h
    code.push(0xEB, (loop_start - (code.length + 2)) & 0xFF); // jmp loop

    // done: spin. A guest that halts with interrupts off never yields,
    // and a timer would carry the emulator on past this point.
    code.push(0xF4);                 // hlt
    code.push(0xEB, 0xFD);           // jmp $

    // The message has to live inside the loaded sector: a BIOS reads 512
    // bytes and nothing else from the floppy is available until the guest
    // asks for it.
    const message = "VMSTATE marker: the guest wrote this";
    const message_at = 0x7C00 + code.length;

    for(let i = 0; i < message.length; i++)
    {
        code.push(message.charCodeAt(i));
    }
    code.push(0);

    if(code.length > 510)
    {
        throw new Error("boot sector is " + code.length + " bytes; a BIOS only loads 512");
    }

    // Patch the message offset now that its position is known.
    code[message_slot] = message_at & 0xFF;
    code[message_slot + 1] = (message_at >> 8) & 0xFF;

    const image = new Uint8Array(2880 * 512);
    image.set(code, 0);
    image[510] = 0x55;
    image[511] = 0xAA;
    return image.buffer;
}

const FLOPPY = build_floppy();

/**
 * Build an emulator for the floppy, and nothing else.
 *
 * `autostart` is the caller's choice: the reloaded machine has to be
 * given its state before it runs, so it starts stopped.
 *
 * @param {boolean} use_jit
 * @param {boolean} autostart
 * @param {ArrayBuffer=} state
 * @return {V86}
 */
function make(use_jit, autostart, state)
{
    const settings = {
        wasm_path: path.join(ROOT, "build/v86.wasm"),
        memory_size: 32 * 1024 * 1024,
        vga_memory_size: 2 * 1024 * 1024,
        disable_jit: !use_jit,
        firmware: "pcjs",
        bios: undefined,
        vga_bios: undefined,
        fda: { buffer: FLOPPY },
        autostart: autostart,
        disable_keyboard: true,
        disable_mouse: true,
        disable_speaker: true,
    };

    if(state)
    {
        settings.initial_state = { buffer: state };
    }

    return new V86(settings);
}

function read_word(cpu, address)
{
    return cpu.mem8[address] | (cpu.mem8[address + 1] << 8);
}

function write_word(cpu, address, value)
{
    cpu.mem8[address] = value & 0xFF;
    cpu.mem8[address + 1] = (value >> 8) & 0xFF;
}

function hex(value)
{
    return "0x" + (value >>> 0).toString(16).toUpperCase().padStart(4, "0");
}

function screen_text(emulator)
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

/**
 * Wait until the guest has written its marker, or give up.
 *
 * The guest halts once it has printed, and a halted CPU with interrupts
 * off never yields, so this polls rather than waiting on an event.
 *
 * @return {Promise<boolean>} whether the marker appeared
 */
function wait_for_marker(emulator, timeout_ms)
{
    return new Promise((resolve) =>
    {
        const deadline = Date.now() + timeout_ms;

        const timer = setInterval(() =>
        {
            let magic = 0;

            try
            {
                magic = read_word(emulator.v86.cpu, MARKER);
            }
            catch(e)
            {
                // The CPU object may not exist yet while start-up runs.
            }

            if(magic === GUEST_MAGIC)
            {
                clearInterval(timer);
                resolve(true);
                return;
            }

            if(Date.now() > deadline)
            {
                clearInterval(timer);
                resolve(false);
            }
        }, 50);
    });
}

let failed = false;

function check(name, ok, detail)
{
    console.log((ok ? "ok   " : "FAIL ") + name + (ok || !detail ? "" : " -- " + detail));

    if(!ok)
    {
        failed = true;
    }
}

/**
 * One round-trip, and everything it proves or does not.
 *
 * @param {boolean} use_jit
 */
async function round_trip(use_jit)
{
    const label = use_jit ? "JIT on" : "JIT off";

    console.log("--- " + label + " ---");

    const emulator = make(use_jit, true);
    const booted = await wait_for_marker(emulator, 15000);

    if(!booted)
    {
        check(label + ": the guest booted and wrote its marker", false, "no marker after 15s");
        emulator.destroy();
        return;
    }

    check(label + ": the guest booted and wrote its marker", true);

    const cpu = emulator.v86.cpu;
    const before = read_word(cpu, MARKER);
    const near = read_word(cpu, MARKER + 2);

    check(label + ": the marker the guest wrote is " + hex(before),
        before === GUEST_MAGIC, "read " + hex(before));
    check(label + ": the value beside it is " + hex(near),
        near === VICINITY, "read " + hex(near));

    const saved_cpu = { cs: cpu.sreg[1], eip: cpu.get_real_eip() };
    const saved_screen = screen_text(emulator);

    // The change: a transaction committed in the machine's RAM, which is
    // what "update the postgres data" is once the guest is running.
    write_word(cpu, MARKER, CHANGED);

    const state = await emulator.save_state();
    check(label + ": save_state produced " + (state.byteLength / 1048576).toFixed(1) + " MiB",
        state.byteLength > 0);

    // A fresh emulator, so nothing above can be mistaken for the restore
    // having failed to happen at all.
    const reloaded = make(use_jit, false, state);

    // Let the restore land and the emulator settle before reading back.
    await new Promise(resolve => setTimeout(resolve, 2000));

    try
    {
        const rcpu = reloaded.v86.cpu;
        const after = read_word(rcpu, MARKER);
        const near_after = read_word(rcpu, MARKER + 2);

        check(label + ": the changed value survived the round-trip (" + hex(after) + ")",
            after === CHANGED, "read " + hex(after) + ", expected " + hex(CHANGED));
        check(label + ": the value beside it is untouched (" + hex(near_after) + ")",
            near_after === VICINITY, "read " + hex(near_after));
        check(label + ": the CPU resumed where it was saved",
            rcpu.sreg[1] === saved_cpu.cs && rcpu.get_real_eip() === saved_cpu.eip,
            "now " + rcpu.sreg[1].toString(16) + ":" + rcpu.get_real_eip().toString(16) +
            ", saved " + saved_cpu.cs.toString(16) + ":" + saved_cpu.eip.toString(16));

        const reloaded_screen = screen_text(reloaded);
        check(label + ": the screen came back",
            reloaded_screen === saved_screen, "screens differ");
    }
    catch(e)
    {
        check(label + ": the reloaded emulator is readable", false, String(e.message).split("\n")[0]);
    }

    reloaded.destroy();
    emulator.destroy();
}

await round_trip(false);
console.log("");
await round_trip(true);

console.log("");
console.log(failed ? "RESULT: FAIL -- a state change did not survive a save/restore"
                   : "RESULT: PASS -- every state change survived the round-trip");
process.exit(failed ? 1 : 0);
