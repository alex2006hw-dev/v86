# Architecture

How the permissive firmware fits together: the guest-visible ROM code,
the host-side services that do the work, the trap that connects them,
and the v86 integration around them.

Companion documents: [`README.md`](../README.md) (what it is and how to use
it), [`Changes.md`](../Changes.md) (what this session changed),
[`TechDebt.md`](../TechDebt.md) (what it deliberately does not do), and
[`firmware-selection.md`](firmware-selection.md) (why we build ROMs rather
than import one).

---

## 1. The two halves

A BIOS is a program the CPU can see: it has a reset vector, it takes
hardware interrupts through a real IDT, and it publishes INT vectors that
guest software is free to inspect, chain and replace. Only the *leaf
work* — the part that actually moves bytes — lives on the host.

That split is the whole design:

| Half | Where | What it is |
|---|---|---|
| **Guest-visible** | ROM at `F0000` and `C0000` | Reset vector, POST entry, hardware-interrupt stubs, INT-vector stubs. 168 of 65536 bytes are non-fill. |
| **Host-side** | Rust, inside the wasm | The services: INT 10h/13h/15h/16h/1Ah, VBE 4Fxx, IRQ 0/1/6/8/12, E820, A20, El Torito. |

The ROM is thin by design. A stub does not call into the host directly;
it pushes a service id and executes a trap:

```asm
int10:  push 0x0010      ; service id
        int  0x66        ; honoured only from inside a firmware ROM window
        iret
```

The CPU notices the trap only when the return address it just pushed
lies inside one of the firmware's own ROM windows. That is what makes
the firmware behave like firmware rather than a library: a program that
hooks INT 9h gets the hook, a program that rebases INT 13h keeps its
handler, and IRQ 0 still ticks whether or not anyone is looking.

---

## 2. The trap

Vector `0x66` is unassigned in the IBM/PC-AT vector map. The emulator
consults two conditions before honouring the trap:

```rust
if crate::fw_adapter::firmware_is_installed()
    && crate::fw_adapter::is_firmware_rom(*instruction_pointer as u32)
```

Both are required. The ROM check alone is not sufficient: SeaBIOS and
the Bochs BIOS live in exactly the address windows this firmware claims
(`F000:0000` and `C000:0000`), so an `int 0x66` executed by a
*third-party* ROM would otherwise be treated as a firmware trap,
serviced by nothing, and the interrupt would never reach the IVT.
SeaBIOS really does execute one, and swallowing it leaves CS:IP stale.

When the trap fires, `firmware_trap()` pops the service id off the guest
stack, runs the service, and lets the stub's `iret` continue. The stack
at that point is exactly the frame the guest's own `INT n` pushed:

```
SS:SP+0   IP        <- the return address, must be left byte-for-byte unchanged
SS:SP+2   CS
SS:SP+4   FLAGS     <- where the carry belongs
```

`patch_saved_flags` writes the carry bit at `SS:SP+4`. Writing it at
`SS:SP` — into the low byte of the saved return address — was the root
cause of every status-returning service silently reporting success and
of the guest hanging under the interpreter. See [`Changes.md`](../Changes.md)
§5.1.

---

## 3. The `Machine` trait

The firmware's services are pure logic over a `Machine` trait, so they
are testable on the host without the full emulator:

```rust
pub trait Machine {
    fn read_u8(&mut self, addr: u32) -> u8;
    fn write_u8(&mut self, addr: u32, val: u8);
    fn read_reg(&self, r: Reg) -> u32;
    fn write_reg(&mut self, r: Reg, v: u32);
    fn read_seg(&self, r: SegReg) -> u16;
    fn write_seg(&mut self, r: SegReg, v: u16);
    fn read_flag(&self, f: Flag) -> bool;
    fn write_flag(&mut self, f: Flag, v: bool);
    fn read_ip(&self) -> u32;
    fn write_ip(&mut self, v: u32);
    fn poll_key(&mut self) -> Option<KeyEvent>;
    fn yield_cpu(&mut self);
    fn rtc_time(&self) -> RtcReading;
    fn request_reset(&mut self);
    fn pop_stack_u16(&mut self) -> u16;
    fn peek_service_id(&self) -> u32;
    fn peek_stack_pointer(&self) -> u32;
    fn read_host_image(&mut self, image: u8, byte_offset: u64, buf: &mut [u8]) -> bool;
    fn patch_saved_flags(&mut self);
    // ...with default no-op implementations for the optional hooks
}
```

`EmulatorMachine` in `fw_adapter.rs` is the production implementation —
a thin translation layer over v86's register, segment and memory
accessors. The unit tests use an in-memory mock. The firmware never
knows which it is talking to.

---

## 4. The dispatch layer

`dispatch.rs` holds the `Firmware` struct, the `Config` that describes
the machine, and the top-level BIOS interrupt dispatcher.

The dispatcher intercepts real-mode software interrupts whose IVT entry
points into the firmware marker area (`0xF0000–0xFFFFF`). If guest
software re-vectors an interrupt elsewhere, the firmware declines and
the guest handler runs — matching real hardware.

```rust
pub struct Config {
    /// Conventional memory in KiB, written to the 16-bit BDA word and
    /// reported by INT 12h. Never the machine's total: that word cannot
    /// hold more than 64 MiB and a real BIOS never puts it there.
    pub memory_kib: u16,
    /// Total installed memory in KiB. This is what E820 and the INT 15h
    /// extended-memory services report, and a guest sizing its page
    /// tables from E820 puts them where this says RAM is.
    pub total_memory_kib: u32,
    pub boot_order: Vec<&'static str>,   // "floppy", "hd", "cd"
    pub floppy_count: u8,
    pub serial_count: u8,
    pub printer_count: u8,
    pub math_coprocessor: bool,
    pub game_io: bool,
    pub dma: bool,
    pub memory_test: bool,
    pub vga_memory_size: u32,
    pub lfb_address: u32,
}
```

---

## 5. The ROM images

Two images are produced, both assembled from Rust source at build time:

| Image | Address | Size | Holds |
|---|---|---|---|
| System BIOS | `F000:0000` | 64 KiB | Reset vector, POST, hardware-interrupt stubs, INT-vector stubs |
| Video option ROM | `C000:0000` | 32 KiB | Video INT 10h/43h/1Ah stubs, VBE information structures, character generator |

`rom.rs` defines the layout and `TRAP_VECTOR`. `asm.rs` is a 16-bit
assembler that emits both images — it needed reg/rm and rel-displacement
fixes before it could produce correct code. `font.rs` is the generated
8×16 character set for the video ROM.

v86's `reset_cpu` already lands at `F000:FFF0`, so the firmware's reset
vector is used with **no CPU change**.

### Option ROMs

A third class of code lives in guest memory too: the ROMs the *host*
registers. `kernel.js` builds a 512-byte stub for `bzimage` boot and
`multiboot` and pushes it into `cpu.option_roms`, expecting the firmware
to run it during POST — without that the kernel never starts.

`option_rom.rs` runs each registered ROM: validates the `55 AA` header,
the length in 512-byte blocks and the byte sum, copies the image into
`0xD0000`–`0xDFFFF` (above the video ROM and the VBE structures), and
enters it with a **far call**. The return address is a five-byte stub in
the system ROM that far-jumps to the INT 19h stub, so a ROM that returns
falls through to the bootstrap and a ROM that never returns — the Linux
stub ends in a far jump — simply never uses it.

The far call needs `Machine::push_u16`, which did not exist: it is the
counterpart to `pop_stack_u16`, and the firmware's only reason to build
a stack frame itself.

---

## 6. Drives: registered by reference

Hard disks and CD-ROMs are registered with the firmware and are **not**
copied into it. v86 hands images over as JavaScript buffers that Rust
cannot address, so a 640 MiB ISO would otherwise mean growing the wasm
heap by most of a gigabyte and holding it for the machine's lifetime.

Instead the firmware asks the host for each sector as it needs it:

```
guest INT 13h → firmware → Machine::read_host_image → js::read_host_image
             → CPU.prototype.firmware_read_image → JS buffer → wasm memory
```

`dest` is a Rust slice pointer — an address in the wasm linear memory —
so the copy is a direct `Uint8Array.set`. One host call per sector read,
invisible next to the emulated CPU work around it. Floppies keep the
copy-in path; 1.44 MiB is not worth a callback.

Drives are numbered the way hardware is: floppies `0x00`-, hard disks
`0x80`-, CD-ROMs `0xE0`-, with 2048-byte sectors for optical media.

---

## 7. The v86 integration

`fw_adapter.rs` is the boundary between the emulator and the firmware
crate. Two directions:

**In** — `firmware_trap()` is called by the CPU when a ROM stub executes
the trap interrupt. It pops the service id, runs the service, and lets
the stub's `iret` continue.

**Out** — the firmware reaches guest state through the `Machine`
implementation, a thin translation layer over v86's register, segment
and memory accessors.

The host ABI exposed to JavaScript:

| Symbol | Purpose |
|---|---|
| `v86_firmware_init` | Install the ROMs and run POST |
| `v86_firmware_add_floppy` | Register a floppy image |
| `v86_firmware_add_drive` | Register a hard disk or CD-ROM by reference |
| `v86_firmware_drive_ptr` / `_len` | Expose a drive's geometry |
| `v86_firmware_set_trace` / `_trace_ptr` / `_len` | The firmware's internal trace log |
| `v86_firmware_set_rtc` | Populate the CMOS RTC |
| `v86_firmware_trap_ring_*` | The last few traps: return address, service id, stack top |
| `v86_firmware_present` | Non-zero when the firmware is installed |

`src/cpu.js` snapshots `firmware_boot_drives` **before** `new IO(this)`
runs, because device setup detaches the disk buffers.

---

## 8. The `vm/` front end

The `vm/` submodule is a guest front end that boots a warm-boot snapshot
of a running Linux machine. It uses the built-in firmware — no
SeaBIOS/VGABIOS — and loads the snapshot at `vm/state/v86state.bin.zst`.

```
vm/
├── index.html          # the page; firmware: "pcjs", vga_memory_size: 8 MiB
├── lib/v86.wasm        # the emulator with the firmware built in
├── filesystem/         # submodule: the 9p filesystem the guest mounts
└── state/              # submodule: the warm-boot snapshot
```

The snapshot was saved with the CPU halted, so restoring it does not
resume execution — the guest does not boot from the snapshot alone. The
`?boot=` path cold-boots a kernel from the filesystem instead.

The `bios-seabios` branch preserves the SeaBIOS/VGABIOS implementation
for comparison; `main` is the firmware-only path.

---

## 9. Testing

The firmware has three layers of test:

| Layer | How | What it catches |
|---|---|---|
| **Unit** | `cargo test -D warnings` (firmware crate) | Pure logic: dispatch, El Torito parsing, E820, VBE, option ROMs. 86 tests. |
| **Integration** | `node examples/firmware.js` | A hand-assembled boot sector that exercises the services a real loader depends on. Prints `RESULT: PASS`. |
| **Browser** | `examples/firmware.html` | The same boot in a browser, with the VGA screen rendered. A dropdown also boots the same sector against SeaBIOS and the Bochs BIOS from `bios/`, so a difference between the rows is a firmware difference. `node examples/firmware-browser-check.mjs` checks the page's wiring headlessly. |
| **State** | `examples/vm-state-check.mjs` | A change to guest state must survive `save_state` → `restore_state` into a fresh emulator, JIT on and off. |
| **Differential** | `node examples/firmware-oracle.js` | Boots the same sector against SeaBIOS/Bochs. Divergence is a firmware difference, not a guest bug. Needs `FW_NO_JIT=1`. |

The self-test boot sector is assembled in `examples/firmware-selftest.mjs`
— no nasm, no disk image, nothing to download. It checks INT 10h, 11h,
12h, 13h, 15h and 1Ah, and prints the letter of the first check that
fails.

The oracle scripts need `FW_NO_JIT=1` because SeaBIOS does not boot under
the v86 JIT — it dies with `table index is out of bounds` before the
guest runs a single instruction. The built-in firmware has no such
problem. See [`TechDebt.md`](../TechDebt.md) JIT-1.
