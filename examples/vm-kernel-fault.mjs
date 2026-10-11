// Where does the vm guest's kernel stop, under any firmware?
//
//   node examples/vm-kernel-fault.mjs [seconds]
//
// The cold boot of the `vm/` guest reaches "Booting the kernel" and then
// stops. This dumps the CPU state at that point in enough detail to say
// *where* it stopped, which is the prerequisite for saying why:
//
//   - the instruction pointer, translated through the guest's own page
//     tables, and the code the guest is actually running
//   - the segment registers, their bases, limits and access bytes
//   - the control registers, and whether paging is enabled
//   - how the control registers changed over time, because "CR3 was never
//     loaded" and "CR3 was loaded and then lost" are different bugs
//
// `FW_BIOS=seabios` runs the same boot against the real SeaBIOS pair,
// which is the control: if the state is the same under both, what stops
// the kernel is in the emulator rather than in this firmware.
//
// Needs vm/ served over HTTP:
//
//     $ (cd vm && python3 -m http.server 8123)
//     $ node examples/vm-kernel-fault.mjs 90

import path from "node:path";
import url from "node:url";
import { V86 } from "../build/libv86.mjs";

const SECONDS = Number(process.argv[2] || 90);
const ROOT = path.resolve(path.dirname(url.fileURLToPath(import.meta.url)), "..");
const VM = path.join(ROOT, "vm");
const VM_URL = process.env.VM_URL || "http://127.0.0.1:8123";

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
    bzimage: { url: VM_URL + "/filesystem/29a77969.bin" },
    cmdline: [
        "rw", "root=host9p rootfstype=9p rootflags=version=9p2000.L,trans=virtio,cache=loose quiet acpi=off",
        "console=ttyS0 tsc=reliable mitigations=off random.trust_cpu=on nowatchdog page_poison=on",
    ].join(" "),
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
console.log("booting with " + (use_seabios ? "SeaBIOS" : "the built-in firmware"));

let serial = "";
emulator.add_listener("serial0-output-byte", (b) =>
{
    serial += String.fromCharCode(b);
    process.stdout.write(String.fromCharCode(b));
});

process.on("uncaughtException", (e) =>
{
    console.log("");
    console.log("=== EXCEPTION during boot, " + (use_seabios ? "SeaBIOS" : "built-in") + " ===");
    console.log(String(e.message).split("\n")[0]);
    dump();
});

const SEGS = ["ES", "CS", "SS", "DS", "FS", "GS"];

function h(v, n)
{
    return (v >>> 0).toString(16).toUpperCase().padStart(n || 8, "0");
}

/**
 * Translate a guest virtual address through its own page tables.
 *
 * The kernel runs at `0xc0000000`-odd virtual addresses with paging on,
 * so the linear EIP is not a physical address and `mem8[eip]` is not the
 * code. Walking the page tables the guest installed is the only way to
 * read it, and which of the two paging modes applies comes from CR4.
 *
 * v86 stores the control registers with their hardware numbering, so the
 * slots are cr[0]=CR0, cr[1]=CR1, cr[2]=CR2, cr[3]=CR3, cr[4]=CR4 --
 * `set_cr3` in cpu.rs writes `cr.offset(3)`, which is what fixes it.
 *
 * @return {number|null} the physical address, or null if not present
 */
function translate(c, mem, virtual_addr)
{
    const cr0 = c.cr[0] >>> 0;
    const cr3 = c.cr[3] >>> 0;
    const cr4 = c.cr[4] >>> 0;

    if((cr0 & 0x80000000) === 0)
    {
        // Paging is off, so linear *is* physical.
        return virtual_addr >>> 0;
    }

    if((cr4 & 0x20) === 0)
    {
        // 32-bit paging: PDE at CR3 + (va >> 22) * 4, then a PTE.
        const pde_at = cr3 + ((virtual_addr >>> 22) << 2);
        const pde = (mem[pde_at] | (mem[pde_at + 1] << 8) |
            (mem[pde_at + 2] << 16) | (mem[pde_at + 3] << 24)) >>> 0;

        if((pde & 1) === 0)
        {
            return null;
        }

        // PSE: a PDE with bit 7 set is itself a 4 MiB page, so there is
        // no PTE to read and the offset fills the page.
        if((cr4 & 0x10) !== 0 && (pde & 0x80) !== 0)
        {
            return ((pde & 0xFFC00000) | (virtual_addr & 0x3FFFFF)) >>> 0;
        }

        const pte_at = (pde & 0xFFFFF000) + (((virtual_addr >>> 12) & 0x3FF) << 2);
        const pte = (mem[pte_at] | (mem[pte_at + 1] << 8) |
            (mem[pte_at + 2] << 16) | (mem[pte_at + 3] << 24)) >>> 0;

        if((pte & 1) === 0)
        {
            return null;
        }

        return ((pte & 0xFFFFF000) | (virtual_addr & 0xFFF)) >>> 0;
    }

    // PAE: a four-entry PDPTE table at CR3, then a PDE, then a PTE.
    const read64 = (at) =>
    {
        let v = 0;
        for(let i = 7; i >= 0; i--)
        {
            v = v * 256 + mem[at + i];
        }
        return v;
    };

    const pdpte = read64(cr3 + ((virtual_addr >>> 30) << 3));

    if((pdpte & 1) === 0)
    {
        return null;
    }

    const pde = read64((pdpte & 0xFFFFF000) + (((virtual_addr >>> 21) & 0x1FF) << 3));

    if((pde & 1) === 0)
    {
        return null;
    }

    if((pde & 0x80) !== 0)
    {
        // A 2 MiB page: the offset fills the page, not just its low 12 bits.
        return ((pde & 0xFFE00000) | (virtual_addr & 0x1FFFFF)) >>> 0;
    }

    const pte = read64((pde & 0xFFFFF000) + (((virtual_addr >>> 12) & 0x1FF) << 3));

    if((pte & 1) === 0)
    {
        return null;
    }

    return ((pte & 0xFFFFF000) | (virtual_addr & 0xFFF)) >>> 0;
}

function dump()
{
    const c = emulator.v86.cpu;
    const mem = c.mem8;

    console.log("serial bytes: " + serial.length);
    console.log("firmware: present=" + c.firmware_present() + " traps=" + c.firmware_trap_count() +
        " last_service=0x" + h(c.firmware_last_service(), 2));

    const eip = c.get_real_eip() >>> 0;
    console.log("");
    console.log("cs:eip = 0x" + c.sreg[1].toString(16) + ":0x" + eip.toString(16));

    const cr = [c.cr[0], c.cr[1], c.cr[2], c.cr[3], c.cr[4]].map(v => v >>> 0);
    console.log("  cr0=" + h(cr[0]) + "  cr2=" + h(cr[2]) +
        "  cr3=" + h(cr[3]) + "  cr4=" + h(cr[4]));
    console.log("  protected_mode=" + ((cr[0] & 1) !== 0) + "  paging=" + ((cr[0] & 0x80000000) !== 0) +
        "  pae=" + ((cr[4] & 0x20) !== 0) + "  eflags=" + h(c.get_eflags && c.get_eflags() >>> 0));

    // Translate the EIP and read the code the guest is actually running.
    // A page directory walk is shown alongside, because a bogus mapping
    // is otherwise invisible: the code would simply look like garbage.
    const phys = translate(c, mem, eip);

    if(phys !== null && phys < 0x8000000)
    {
        console.log("  eip 0x" + eip.toString(16) + " -> physical 0x" + phys.toString(16) +
            (phys < 0x1000000 ? "  (below the kernel image!)" : ""));

        let code = "";
        for(let i = -8; i < 24; i++)
        {
            const b = mem[(phys + i) >>> 0];
            code += (i === 0 ? "[" : "") + (b === undefined ? "??" : b.toString(16).padStart(2, "0")) +
                (i === 0 ? "]" : "") + " ";
        }
        console.log("  bytes around eip: " + code);
    }
    else
    {
        console.log("  eip 0x" + eip.toString(16) +
            (phys === null ? " does not translate (not present)" : " translates outside RAM: 0x" + phys.toString(16)));
    }

    console.log("  segs : " + SEGS.map((n, i) => n + "=0x" + c.sreg[i].toString(16)).join(" "));
    if(c.segment_offsets)
    {
        console.log("  bases: " + SEGS.map((n, i) => n + "=" + h(c.segment_offsets[i])).join(" "));
    }
    if(c.segment_access_bytes)
    {
        console.log("  access: " + SEGS.map((n, i) => n + "=" + h(c.segment_access_bytes[i], 2)).join(" "));
    }

    const NAMES = ["EAX", "ECX", "EDX", "EBX", "ESP", "EBP", "ESI", "EDI"];
    console.log("  regs: " + NAMES.map((n, i) => n + "=" + h(c.reg32[i])).join(" "));

    // The stack, through the SS segment: in 32-bit mode ESP is the full
    // register, and masking it to 16 bits names the wrong place.
    const sp = c.reg32[4] >>> 0;
    const ss_base = c.segment_offsets ? c.segment_offsets[2] >>> 0 : 0;
    let stack = "";
    for(let i = 0; i < 10; i++)
    {
        const a = (ss_base + sp + i * 4) >>> 0;
        stack += h(a) + ": " + h(mem[a] | (mem[a + 1] << 8) | (mem[a + 2] << 16) | (mem[a + 3] << 24)) + "  ";
    }
    console.log("  stack (ss + esp=0x" + sp.toString(16) + "):");
    console.log("    " + stack);

    // Where the decompressed kernel landed, so a comparison between the
    // code on screen and the code at EIP is possible.
    const kernel_at = translate(c, mem, 0xc0000000);

    if(kernel_at !== null && kernel_at < 0x8000000 && kernel_at >= 0x1000000)
    {
        let k = "";
        for(let i = 0; i < 16; i++) k += mem[kernel_at + i].toString(16).padStart(2, "0") + " ";
        console.log("");
        console.log("  virtual 0xc0000000 -> physical 0x" + kernel_at.toString(16));
        console.log("  kernel image base: " + k);
        console.log("  eip is " + ((eip - 0xc0000000) >>> 0).toString(16) + " into that");
    }
}

// The exit is on the clock, not on the guest: the guest never halts, so
// there is no event to key off, and an EIP that keeps changing is itself
// an answer. The control registers are logged as they change, because
// "CR3 was never loaded" and "CR3 was loaded and then lost" are different
// bugs and the final state cannot tell them apart.
const started_at = Date.now();
const deadline = started_at + SECONDS * 1000;
let last_eip = -1;
let stable_since = started_at;
let dumped = false;
let last_cr = [0, 0, 0, 0, 0];

const timer = setInterval(function()
{
    let eip = -1;

    try
    {
        const c = emulator.v86.cpu;
        eip = c.get_real_eip() >>> 0;
        // Slot, not register number: the array is indexed by the slot
        // v86 stores them in, so the printed name is looked up rather
        // than taken from the index.
        const cr = [c.cr[0], c.cr[1], c.cr[2], c.cr[3], c.cr[4]];
        const CR_NAME = ["cr0", "cr1", "cr2", "cr3", "cr4"];

        for(let i = 0; i < cr.length; i++)
        {
            if(cr[i] !== last_cr[i])
            {
                console.log(((Date.now() - started_at) / 1000).toFixed(2) + "s  " + CR_NAME[i] + ": 0x" +
                    (last_cr[i] >>> 0).toString(16) + " -> 0x" + (cr[i] >>> 0).toString(16));
                last_cr[i] = cr[i];
            }
        }
    }
    catch(e)
    {
        // The CPU object may not exist yet.
    }

    if(eip !== last_eip)
    {
        last_eip = eip;
        stable_since = Date.now();
    }

    const settled = Date.now() - stable_since > 5000;
    const elapsed = Date.now() - started_at;

    if(elapsed > 10000 && elapsed % 10000 < 250)
    {
        // A heartbeat, so a long boot says it is alive.
        console.log(elapsed + "s elapsed: cs:eip=0x" +
            (emulator.v86.cpu.sreg[1].toString(16)) + ":0x" + eip.toString(16) +
            " serial=" + serial.length);
    }

    if((settled || Date.now() > deadline) && !dumped)
    {
        dumped = true;
        clearInterval(timer);
        console.log("");
        console.log("=== " + (settled ? "instruction pointer stopped moving after"
            : "still running after") + " " + (elapsed / 1000).toFixed(0) + "s ===");
        dump();
        emulator.destroy();
        process.exit(0);
    }
}, 250);
