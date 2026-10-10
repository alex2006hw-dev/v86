# Tech debt — v86 permissive firmware (`src/rust/firmware`)

Register of what the firmware deliberately does *not* do yet, why, and
what it takes to close each item.

Companion documents: `README.md` (what it is and how to use it),
`Changes.md` (what this session changed), `todo.md` (the plan), and
`docs/firmware-selection.md` (why we build ROMs instead of importing one).
`architecture.md`, covering how it all fits together, has not been written
yet.

## How to read this

| Status | Meaning |
|---|---|
| **Stub** | Code path exists and is reachable, but does nothing useful. Worst kind: it looks finished. |
| **Absent** | Not implemented at all; a guest using it gets an error or silence. |
| **Wired** | Works on the host, but the guest-facing half is missing. |
| **Blocked** | Cannot be built until something else lands. |
| **Broken** | Implemented, does not currently work. |

Severity is the *guest-visible* consequence, not the effort: a `Stub`
that reports success to a Windows 95 installer is worse than an honest
`Absent` that reports failure.

## Summary

| ID | Area | Status | Severity |
|---|---|---|---|
| **CD-2** | **`INT 13h AX=4B00h/4B02h` absent** — POST boots from CD, a loader cannot | Absent | Medium |
| **BOOT-1** | Debian/NetBSD installers do not complete boot, under any BIOS | Broken | High |
| **BOOT-2** | **32-bit protected-mode guests fault at boot handoff; SeaBIOS boots them** | Broken | **High** |
| **CD-3** | El Torito `terminate emulation` is a no-op | Stub | Medium |
| **CD-4** | No ATAPI `IDENTIFY` (`INT 15h AX=4F06h`) | Absent | Medium |
| **CD-5** | El Torito boot catalog not in the boot-config table | Absent | Low |
| **ROM-1** | **No loadable option-ROM infrastructure** | Absent | **High** |
| **USB-1** | **No USB device emulation in v86 at all** | Absent | **High** |
| **USB-2** | **No BIOS-side USB driver or mass-storage stack** | Absent | **High** |
| **USB-3** | **No WebUSB host↔guest mapping** | Absent | **High** |
| **DEV-2** | No device-change / hot-plug notification | Absent | Low |
| **HOST-1** | Trap ABI has no room for async or bulk transfer | Absent | Medium |
| **TEST-1** | Boot test is green, but nothing runs it | Wired | Medium |
| **TEST-3** | No boot matrix in CI | Absent | Medium |
| **TEST-4** | Only one test disk image exists | Absent | High |
| **TEST-5** | The self-test is only checked against itself | Wired | Medium |
| **RTC-1** | v86's CMOS RTC is not populated for third-party BIOSes | Absent | Medium |
| **BUILD-1** | Build needs an out-of-tree toolchain | Absent | Medium |
| **BUILD-2** | `tests/firmware/` harness is stale and cannot run | Broken | High |
| **BUILD-3** | Generated Rust sources are not committed | Absent | Low |
| **JIT-1** | **SeaBIOS cannot boot under the v86 JIT** | Broken | Medium |

---

## CD-ROM: booting from CD-ROM

Hard disks and CD-ROMs now reach the firmware, and El Torito boot works:
no-emulation images boot, verified against a purpose-built ISO and against
Debian's real catalogue. What is left is the part a *loader* needs when it
wants to boot from CD after POST has already run.

### CD-2 — INT 13h AH=4B00/4B02 are missing

This is the part that matters for real guests. `int13.rs:190-192`
dispatches AH=4Ah, 4Bh and 4Dh, and nothing else. A DOS CD-ROM driver
or a Windows 9x loader booting from CD uses:

| Service | Meaning | Status |
|---|---|---|
| `INT 13h AX=4B00h` | REQUEST BOOT — boot from the CD via El Torito | **Absent** |
| `INT 13h AX=4B02h` | EMULATED DRIVE ACCESS (via `INT 4Eh`) | **Absent** |
| `INT 13h AX=4B01h` | GET EMULATION STATUS | Stub (AL=0) |
| `INT 13h AX=4B05h` | TERMINATE EMULATION | **Stub** (see CD-3) |
| `INT 13h AX=4C00h` | RETURN BOOT CATALOG | Stub |
| `INT 13h AX=4D00h` | RETURN NEXT SIGNATURE | Stub |

`4B00h` must load and jump exactly as `post.rs::boot_from_cd` does, and
`4B02h` must service an emulated-drive register block the loader passes
in. Note the return convention differs from ordinary INT 13h: `4B00h`
reports through `CF` but must *not* leave the caller in a state where an
`AH=4B02h` is attempted after the jump.

**Why deferred:** needs CD-1.

**To finish:** factor `boot_from_cd`'s per-media-type body into a
function callable from both the POST path and a new `handle_int13`
`0x4B00 =>` arm; implement the emulation status/termination state
machine that `4B01`/`4B05` currently stub out; add the `INT 4Eh` entry
and its frame layout.

**Done when:** FreeDOS `CDROM.SYS`/`MSCDEX` and a `mkisofs`-produced
`isolinux` image both boot through the `4B00h` path with the emulator
using AH=4B02h afterwards to read the filesystem.

### CD-3 — `TERMINATE EMULATION` is a no-op

`eltorito_emulation_control` (`int13.rs:827`) accepts `AL=00h` and
returns success while leaving the emulated drive registered. A loader
that terminates emulation and then reads the emulated drive number gets a
drive that still works, so the bug is silent until something enumerates
`DriveKind::HardDisk` and finds a phantom disk.

**To finish:** give `Drives` (`int13.rs:54`) a `remove(number)` and
have `4B05h` drop the emulated drive and return its number in `BX`.

### CD-4 — No ATAPI `IDENTIFY`

`INT 15h AX=4F06h` (ATAPI/EIDE IDENTIFY DEVICE) is how software
discovers that drive `0xE0`+ is optical and what its packet size is.
`handle_int15` (`int15.rs:11`) implements AH=88h, AH=52h, AH=24h,
AH=C0h, AH=C1h, AX=E820h and AX=E801h, and nothing else. v86's IDE
device already implements a substantial ATAPI command set
(`src/ide.js`: `READ_CD`, `READ_CAPACITY`, `GET_CONFIGURATION`,
`READ_TOC`, `INQUIRY`), so the device side is not the blocker.

**To finish:** a `4F06h` handler that issues PACKET `IDENTIFY PACKET
DEVICE` (0xA1) to the selected port and copies the 512-byte response to
`ES:DI`, translating v86's ATAPI sense data into BIOS error codes.
This is also a prerequisite for USB-2's block-storage enumeration.

### CD-5 — Boot catalog not in the boot-config table

`eltorito_return_boot_catalog` (`int13.rs:846`) serves `4D00h` from a
cache, but the table returned by `INT 15h AH=C0h` has no boot catalog
entry (configuration table offset `0xE8`). Loaders that prefer to walk
the table never call `4D00h`.

**Severity:** low — `4D00h` covers it. Fix it when touching AH=C0h.

---

## ROM infrastructure

### ROM-1 — No loadable option-ROM infrastructure

This is the item that most limits how CD-1/CD-2 and USB-2 will be
built, so it is worth stating plainly.

`rom.rs` builds exactly two images: a 64 KiB system ROM at `F000:0000`
and a 32 KiB video option ROM at `C000:0000` (`rom.rs:49-56`). The
system ROM is **0.3% full** — 168 non-fill bytes — because essentially
all logic lives in Rust on the host and the ROM holds only a reset
vector, a POST entry, and `push id; int 0x66; iret` stubs.

That is a deliberate and comfortable position *for services the host
can answer*. It stops being one the moment a driver has to be visible to
the guest as guest code — a USB stack that Windows 95 pokes port by
port, or a CD driver that responds to a hardware IRQ, must exist as real
instructions in guest address space, not as a host call.

The ROM builder can emit fixed code, but nothing supports:

- `INT 15h AX=E800h` "install option ROM" / AX=E801h, AX=E802h
- `INT 15h AX=D000h` "get system ROM map" (which drives `ES:DI`)
- `INT 15h AX=4F08h` "get ISA/PCI information" (the parenthesised
  option-ROM list a loader parses to find class-code-matched ROMs)
- PCI/PCI Express option ROMs in the `0xC000`–`0xDFFF` window,
  addressed by `(bus, dev, fn)` in BX rather than by a flat base
- a relocation entry in the option-ROM header, needed as soon as a
  driver is not written to run at its link address

**To finish:** add a ROM *registry* (kind, base, size, class code,
payload source) that can hold a third "loadable" image, an
`INT 15h AX=E800h` implementation that copies it to the caller's
CS:0000, fixes up the checksum and relocations, and installs its INT
handler; and a slot in the config table for the ROM map.

**Done when:** a driver written as ordinary 16-bit assembly can be
dropped into the tree, is loaded by a real guest on demand, and is
reported in the AH=C0h ROM map.

---

## USB: BIOS driver and WebUSB host↔guest mapping

There is no USB anywhere in this repository today. `src/` has
`acpi.js`, `bus.js`, `dma.js`, `floppy.js`, `ide.js`, `ne2k.js`,
`parallel.js`, `pci.js`, `pit.js`, `ps2.js`, `rtc.js`, `sb16.js`,
`uart.js`, `vga.js`, `virtio*.js` and `vmware.js` — no `usb.js`, and
`navigator.usb` appears nowhere. A BIOS USB driver therefore has two
independently missing halves, and neither can be started in earnest
until the other has a shape.

### USB-1 — No USB device emulation in v86

**Status:** Absent. **Severity:** High — it blocks USB-2 and USB-3
entirely.

Scope, smallest useful increment first:

- A UHCI controller (`src/usb.js`) on the PCI bus: frame list in guest
  RAM, transaction descriptors, and the port/status registers at BAR4.
  UHCI 1.1 is the right first target — it is what Bochs, QEMU's `piix3-
  usb-uhci` and PCjs all implement, and its model is "the guest
  programs TD/QH rings in memory", which needs almost no firmware-side
  translation.
- Device model behind it: at minimum a USB mass-storage device
  (Bulk-Only Transport) over a `buffer.js`, and a USB keyboard/HID
  stub so `ps2.js` has a peer.
- Host↔device transport: a pluggable backend, because v86 runs in a
  browser and in Node and those have different available sources.

**Done when:** a guest program walking the UHCI frame list can read
`GET_DESCRIPTOR` from an emulated device over real DMA, with a test
that asserts on guest-visible register state rather than on internals.

### USB-2 — No BIOS-side USB driver or mass-storage stack

**Status:** Absent. **Severity:** High — with no controller to talk to,
and no room to put a driver in (ROM-1).

Two layers, and they have different homes:

1. *Controller driver* — init UHCI, set up the frame list, poll. Belongs
   in ROM-1's loadable option ROM as real guest code, because it must
   service the controller's IRQ. This is the first genuine test of
   ROM-1.
2. *Block transport* — enumerate the device, read its configuration
   descriptor to find the Bulk-Only Transport interface, wrap it as an
   INT 13h drive. This half *can* be a host service on the firmware's
   existing pattern, which is why it should be built second: it is the
   part that makes USB useful for booting a stick, and it needs no ROM
   space.

`INT 13h AH=4Eh` (get drive parameters, EDD) is the natural hand-off
point for a USB mass-storage drive, and is already dispatched. It also
interacts with CD-4: an OS enumerating "disks" wants `AX=4F06h` to work
for any drive it finds, not just optical.

**Done when:** FreeDOS boots from an emulated USB stick image and reads
the rest of the volume through INT 13h.

### USB-3 — No WebUSB host↔guest mapping

**Status:** Absent. **Severity:** High for the browser target.

The intent is that a physical device attached over WebUSB in the host
browser appears inside the guest as an emulated USB device, so a guest
that already speaks USB needs no knowledge that v86 is involved.

```mjs
const device = await navigator.usb.requestDevice({ filters: [{ classCode: 8 }] });
const v86 = new V86({ usb: { devices: [device] } });
```

This is a *mapping* problem, not an emulation problem, and the awkward
part is one level below the emulator:

- **Device identity.** A WebUSB device has no stable identifier across
  reconnects. `getDeviceInfo()` is optional; without it there is nothing
  to match a guest re-enumeration against. Needs a policy: match on
  `(vendorId, productId, interfaceClass)` plus an index, and accept that
  a guest cannot distinguish two identical devices.
- **Endpoint semantics.** WebUSB endpoints are in-band transfers, while
  the emulator wants a device model that answers control and bulk
  transfers. A straight adapter is a *transparent USB device* whose
  endpoints forward; a faithful one is a USB disk whose SCSI/command
  layer synthesises responses from the bulk transfers. The second is
  what makes a guest believe it is talking to real hardware, and it is
  much more work.
- **Buffering and latency.** WebUSB bulk transfers are async and may be
  slow (tens of ms). v86's emulation is synchronous inside the
  translation block. Anything that blocks risks stalling the CPU; the
  workable shape is a queue drained between blocks, which changes the
  device model's timing contract.
- **Permissions.** `requestDevice` needs a user gesture and a
  secure context, and permission does not survive a reload. The guest
  will see a device that disappears; the firmware needs to handle
  disconnect cleanly rather than wedging (see DEV-2).
- **Headless/Node.** The WebUSB path is browser-only. The Node CI
  boot tests need a non-WebUSB backend — `usb`/`node-usb` or a loopback
  backend over the same interface — or USB never gets regression
  tested.

**Why deferred:** it is strictly downstream of USB-1 and USB-2, and the
browser-only half means it cannot be covered by the headless test
harness without standing up a second transport.

**To finish:** define the USB backend interface so that WebUSB, node-usb
and a loopback fake are three implementations of one trait; land the
loopback fake first, so USB is testable in CI before it is testable in a
browser.

**Done when:** the same guest image boots from a USB stick backed by
(a) a `.img` file, (b) `node-usb` hardware, and (c) a WebUSB device,
with one test per transport.

---

## Plumbing and ABI

### DEV-2 — No device-change / hot-plug notification

`src/cpu.js:730-745` shows the IDE device being *reconstructed* during a
state restore, from the incoming state's own `cdrom`/`ide` buffers.
Nothing tells the firmware, so the BIOS drive table and the device's
backing store silently disagree. Also needed for the WebUSB disconnect
case in USB-3. **Severity:** Low today, Medium once USB-3 lands.

### HOST-1 — The trap ABI has no room for bulk transfer

`TRAP_VECTOR = 0x66` with a `push service_id; int 0x66; iret` stub is a
good fit for leaf services that read and write registers and a little
guest memory, and it correctly leaves vector `0x66` available to guests
(the emulator only honours the trap when the return address is inside a
firmware ROM window — `is_firmware_rom`). It does not fit:

- **sector I/O.** A 512-byte read is done byte-at-a-time through guest
  memory. Correct, but slow enough to matter for a USB boot chain.
- **anything asynchronous.** The service is fully synchronous; there is
  no way to say "not ready, retry on the next poll" without keeping all
  state in a register the guest must carry.
- **anything larger than the emulated register set.** No DMA-style
  scatter/gather.

**To finish:** add pointer-sized arguments via the trap (`BX`, `CX`,
`DX` already exist) with the firmware reading them as guest pointers —
the DAP path already does this — and a shared-memory ring for anything
that must be genuinely asynchronous. The `0x66` design survives all of
this; it just needs a convention.

**Done when:** a service moves more than 64 bytes through the host
without the firmware reading each byte individually.

---

## Tests

### BOOT-1 — Debian's installer does not complete boot, under any BIOS

**Status:** Broken. **Severity:** High.

```
$ FW_DEVICE=cdrom node examples/cd-boot.js debian-12.1.0-i386-netinst.iso
... firmware parses the catalogue and loads the boot image, then stops
```

The firmware's half demonstrably works — the trace shows it reading LBA 16
(primary volume descriptor), LBA 17 (boot record) and LBA 1119 (catalogue),
loading the no-emulation image and jumping to it, after which the guest
performs 16 further INT 13h reads of its own before stopping.

**Not a firmware regression**, checked rather than assumed: SeaBIOS fails on
the same image attached as `hda` from a local file, ending at the same
`cs:eip` with a garbage signature at `0000:7C00`. Whatever state Debian's
early loader reaches, v86 does not carry it past, and that is true of the
BIOS this work replaces.

Not yet diagnosed. The guest stops without another read, so it is executing
rather than waiting on I/O; the candidates are a device v86 does not emulate
that the loader probes for, or a timing assumption that never satisfies.

Note that SeaBIOS could not *load* a 640 MiB CD through
`tools/httpfs-v86-server.py` within 150 s, because that path copies the
image into wasm memory. The by-reference drive design streams it in ~20 s,
so this is at least not the same problem twice.

**Done when:** `debian-12.1.0-i386-netinst.iso` reaches an installer menu,
with SeaBIOS and the built-in firmware behaving the same.

### BOOT-2 — Protected-mode guests fault at the boot handoff

**Status:** Broken. **Severity:** High.

Two 32-bit El Torito images, both from v86's own Advent calendar — the page
publishes only images that "are 32-bit x86 and work in v86" — boot to
different places:

| Image | Size | El Torito | SeaBIOS | built-in firmware |
|---|---|---|---|---|
| `FreeNOS-1.0.3.iso` | 10.5 MiB | no-emulation, 4 sectors from LBA 3800 | **reaches `login:`** | **panics** |
| `HelenOS-0.11.2-ia32.iso` | 24.6 MiB | no-emulation, 56 sectors from LBA 64 | boots | **panics** |

Both panic identically:

```
Unimplemented: #GP handler                       (src/rust/cpu/cpu.rs:856)
  call_interrupt_vector -> trigger_gp -> switch_seg -> instr32_07
```

`instr32_07` is `POP ES` in 32-bit mode, so the guest has entered protected
mode and a segment load is being rejected.

**This is a firmware gap, not a v86 limitation** — which is the opposite of
what BOOT-1 turned out to be, and the reason the oracle exists. SeaBIOS boots
the same image in the same emulator to a login prompt.

What is ruled out, each checked rather than assumed:

- **Not the CD path.** Attaching FreeNOS as `hda` via its hybrid MBR panics
  identically, so it is the boot handoff rather than CD reading.
- **Not I/O.** Every read the firmware makes succeeds, and the trace shows it
  reading LBA 16 (PVD), 17 (boot record), 46 (catalogue) and 3800 (the boot
  image), after which the guest issues its own 16 KiB reads. The bytes
  written to `0000:7C00` match the ISO.
- **Not A20.** v86 does not implement A20 masking at all (`src/ps2.js:705`
  says where it would go), so memory is flat either way.

One real defect was found and fixed on the way: the handoff ran with
interrupts disabled, because a software `int 0x19` clears IF in the CPU core
and nothing re-enabled it. Every real BIOS enables interrupts before
transferring control. That fix is correct but did **not** resolve this.

Not yet diagnosed, but narrowed. Disassembling FreeNOS's first stage (2 KiB
at CD LBA 3800) shows it is a hand-written 16-bit loader that:

1. Reads the El Torito load RBA and sector count from a header at `cs:[bx+9]`
   and `cs:[bx+0xd]`, then computes `ceil(count / 2048)` — confirming the
   sector count is in **512-byte virtual sectors** and is rounded up to whole
   CD sectors, which is what `boot_from_cd` already does.
2. Loads the rest of itself with **`INT 13h AH=42h`**, building a disk
   address packet **on the stack at `SS:SP`** with `DS=0` and `SI=SP`, and a
   buffer at `0000:0800`.
3. Far-jumps to `0000:0820`.

The `jmp 0x820:0x0` at `0x7C6E` is a **far** jump, and in real mode that
would land at `0x8A0`, not `0x820`. So the loader is already in 32-bit
protected mode by then: it runs its BIOS call through a far call to a 16-bit
segment, then returns by popping the segment registers — which is exactly
where the `POP ES` #GP lands. The failure is in the protected-mode entry, not
the disk layer.

### Ruled out

- **The CD path** — FreeNOS as `hda` via its hybrid MBR panics identically.
- **Host I/O** — every firmware read returns success; bytes at `0000:7C00`
  match the ISO.
- **A20** — v86 does not implement A20 masking at all. Port `0x92` is a stub
  that stores a byte and does nothing (`src/cpu.js`), and the PS2 controller
  output port is explicitly a placeholder (`src/ps2.js:705`). Memory is
  always flat, so A20 cannot explain a guest that works under one BIOS and
  not another.
- **The BIOS's part of the checklist** — the canonical sequence before
  protected mode (disable interrupts and NMI, enable A20, load the GDT, set
  `CR0.PE`, far jump) is the *loader's* job. The BIOS's job is narrower.

### Leading hypothesis, not proven

`lookup_segment_selector` raises `OutsideOfTableLimit` when
`selector.descriptor_offset() > *gdtr_size`, and `gdtr_size` is only ever set
by `lgdt` (or reset to 0). `lgdt` is:

```rust
let size = return_on_pagefault!(safe_read16(addr));
```

On a page fault `return_on_pagefault!` returns **from the function**, leaving
`*gdtr_size` at whatever it was — 0 after reset. Every subsequent segment
load then #GPs, which is precisely the observed failure. A guest that never
notices, because the fault happens at the "pop all the segments" idiom.

Whether that is *this* bug is unproven. It is a real weakness and the first
thing to check.

### The experiment that would settle it

A synthetic reproducer, independent of any guest: extend the self-test boot
sector with `lgdt` → `mov cr0` → far jump → `pop es`, about sixty bytes.
Under the built-in firmware it should either pass or produce a minimal,
attributable failure — which separates "v86 cannot enter protected mode from
a BIOS boot sector" from "FreeNOS needs something specific".

### Reference material

[EDK II](https://github.com/tianocore/edk2) is **BSD-2-Clause-Patent** — the
same permissive class as v86's own base — and has no AI contribution policy,
so it is usable as a reference. `OvmfPkg/Library/LoadLinuxLib/LinuxGdt.c` is
the most directly relevant file: OVMF's own legacy-style handoff to a
protected-mode kernel installs a GDT via `LGDT` and a **null IDT** via
`LIDT`, then far-jumps through selector `0x10`. Worth comparing against our
handoff state. It is reference only — nothing has been copied.

EDK II cannot replace this firmware: it is UEFI, with no real-mode
`INT 10h`/`INT 13h`, so it would not serve DOS or Windows 3.x. See
`docs/firmware-selection.md`.

**Done when:** FreeNOS reaches its `login:` prompt on the built-in firmware,
with SeaBIOS as the oracle for the same image.

### TEST-1 — The boot test is green, but nothing runs it

`node examples/firmware.js` boots the hand-assembled self-test sector
from `examples/firmware-selftest.mjs` and asserts the guest printed
`RESULT: PASS`. It passes, with and without the JIT. It is not wired
into CI, so nothing stops it regressing.

**The bugs it caught**, all now fixed, listed because they are the
reason the test exists:

| Bug | Where | Consequence |
|---|---|---|
| `patch_saved_flags` wrote the carry bit at `SS:SP`, over the **saved return address** instead of FLAGS | `src/rust/fw_adapter.rs` | The caller's return IP was redirected into the middle of its next instruction. Hung under the interpreter; `table index is out of bounds` under the JIT. Three services also silently reported success. |
| EDD install check returned `AH=0` (the classic success code) rather than the Extensions major version, `DH=0x20`, and echoed the requested drive number in `DL` instead of the drive count | `src/rust/firmware/src/int13.rs` | A caller parsing the version got "version 0"; a caller counting drives got 1 drive regardless. Found by the oracle, not by the self-test. |
| Disk address packet fields written two bytes late, buffer segment derived from the buffer offset | `examples/firmware-selftest.mjs` | The EDD read landed at the wrong address and check S failed. |
| Boot-sector signature compared against the wrong byte of a word load | `examples/firmware-selftest.mjs` | A correct read reported failure. |
| `INT 10h AH=0Fh` column count read from `AL`; it lives in `AH` | `examples/firmware-selftest.mjs` | Tested the video mode instead of the column count. |
| RTC century read from `AH`; it lives in `CH` | `examples/firmware-selftest.mjs` | Passed against our firmware *for the wrong reason* — AH still held the `0x02` from the `mov ah` that made the call — and failed against SeaBIOS. See the oracle note below. |

**Why the oracle matters here.** Those last two were only visible
because the same boot sector was run against real BIOSes. See
TEST-5.

**Done when:** the test runs in CI (TEST-3), not just by hand.

### TEST-3 — No boot matrix in CI

`todo.md` §4.1–4.3 name the target matrix (MS-DOS 3.3/4.0/6.22,
Windows 1.0/2.0/3.1/95, Minix, DOS games). None of it runs
automatically. `todo.md` §4.4 — the SeaBIOS fallback still boots — is
also unverified, which matters because it is the escape hatch if this
firmware regresses.

**To finish:** once TEST-1 is green, add a headless boot runner
(`todo.md` §7.5's WASI build is the natural host) that boots each image,
screenshots the VGA memory the way `firmware-debug.js` does, and asserts
on guest output. QEMU stays a behavioural oracle only; its test code is
GPL and must never be copied (`todo.md` §4.5).

### TEST-4 — Only one test disk image exists

The only guest-side test program is `firmware-selftest.mjs` (and its
stale predecessor `tests/firmware/boot-sector.mjs`). There is no image
that exercises a real boot loader, so regressions in the parts that
matter most to users — El Torito, EDD/LBA, VBE modes — are invisible.
**Severity:** High. Blocked on CD-1 for the ISO case.

---

### TEST-5 — The self-test is only checked against itself

**Status:** Wired. **Severity:** Medium.

The self-test asserts what *we* believe a BIOS should do. Where our
belief is wrong, the test is wrong with it — twice over, because the
firmware and the test share the author's assumptions.

`examples/firmware-oracle.js` boots the same sector against SeaBIOS
and the Bochs BIOS from `bios/`, and
`examples/firmware-service-probe.mjs` calls one service and records the
registers a BIOS returns, so a disagreement about a specification can be
settled by measurement rather than argument. This is the pattern
`todo.md` §4.5 already sets for QEMU: an oracle to consult, never code
to copy. The images under `bios/` are GPL-2.0-or-later / LGPL-3.0 and
are only ever queried.

What it established:

- **`INT 1Ah AH=02h` has no dependable answer.** Both reference BIOSes
  return a day of `0` and a nonsensical century, because v86's CMOS RTC
  is not populated the way real hardware would be (RTC-1). Our firmware
  reads the emulated clock directly and is unaffected. A check written
  from our firmware's behaviour alone would have been a test of v86's
  CMOS, not of the RTC service.
- **EDD on a floppy is not a given.** SeaBIOS and Bochs both refuse
  `AH=41h` on drive 0 and leave `BX=55AAh` untouched. We accept it,
  because we really do implement EDD reads on floppies, but a caller
  must not assume either behaviour.
- **`INT 12h` legitimately differs.** We report the configured memory
  (32 MiB → `AX=8000h`); SeaBIOS and Bochs report conventional memory
  (`AX=027Fh`). Both are defensible; guests differ in which they need.

**To finish:** record each disagreement next to the check it concerns,
so the next person does not re-derive them.

---

## Emulator gaps

### RTC-1 — v86's CMOS RTC is not populated for third-party BIOSes

**Status:** Absent. **Severity:** Medium.

Our firmware answers `INT 1Ah` by reading the emulated clock directly
(`machine.rtc_time()`), and returns a correct date. SeaBIOS and Bochs read
the CMOS registers directly, and both return a day of `0` and a
nonsense century from the same machine:

| BIOS | CX (century:year) | DX (month:day) |
|---|---|---|
| built-in firmware | `1980h` | `0101h` |
| SeaBIOS | `0732h` | `2800h` |
| Bochs | `0732h` | `4800h` |

`src/rtc.js` defines `CMOS_CENTURY` and derives most fields on read, but
nothing populates the full MC146818 register file the way real hardware
would at POST. A BIOS that trusts CMOS gets garbage, and a guest running
such a BIOS gets a garbage date.

This is a v86-side gap, not a firmware bug, and it is invisible to every
test that uses the built-in firmware — which is precisely why
`examples/firmware-oracle.js` exists.

**To finish:** populate the CMOS time registers from `rtc_time()` during
POST, the way `src/rtc.js` already populates a few of them, and add an
oracle run to the boot matrix so the BIOS-independent path stays
honest.

**Done when:** the INT 1Ah row of the oracle comparison agrees across
all three BIOSes.

---

## Build and packaging

### BUILD-1 — The build needs an out-of-tree toolchain

The documented build (`Readme-orig.md` §"How to build, run and embed?",
and `Makefile`) could not run in the development container: no `clang`,
no Java, no root. The wasm was produced with a downloaded **Zig 0.14.1**
using `zig cc --target=wasm32-wasi` for `build/softfloat.o` and
`build/zstddeclib.o`, and `build/libv86.mjs` / `build/libv86.js` are
hand-written one-line shims re-exporting `src/main.js`. Neither the
Closure compilation nor its dead-code elimination has been exercised
against this firmware, which means the shipping bundle may well be much
larger than it needs to be.

Note this also breaks the two `firmware-build` / `firmware-test` targets
in TEST-2, since they want `wasm32-wasip1` output.

**To finish:** fix the toolchain story, run the real Closure build, and
compare bundle sizes. If Closure breaks on the firmware crate, that is
a finding worth recording here before anything is released.

### BUILD-2 — The `tests/firmware/` harness is stale and cannot run

`make firmware-test` (`Makefile:390`) does not work. Two independent
reasons:

1. **Hardcoded absolute paths.** `run.mjs:21-33` imports the PCjs CPU
   from `/Volumes/HOME/Users/dev/work/pcjs/...`, a developer's local
   macOS checkout. It fails at module resolution on every other machine,
   including this one.
2. **It predates the ROM design.** The harness hooks PCjs
   `addIntNotify()` and places a bare `IRET` at `F000:0000` — it does not
   map a ROM at all. The firmware now traps on `TRAP_VECTOR 0x66` from
   real ROM stubs (`rom.rs`), which needs the guest's return address to
   be inside a firmware ROM window (`is_firmware_rom`). A harness that
   has no ROM cannot exercise the actual delivery mechanism.

What *does* work is `make firmware-unit-test` (`Makefile:385`) — the
host-side Rust suite, 58 tests over the assembler, font, ROM images,
trace ring and the service layer. And `examples/firmware-debug.js` runs
the real thing in the real v86. The gap is the layer in between.

**To finish:** decide whether the PCjs harness is kept. If kept, resolve
PCjs by package or `PCJS_ROOT` env var, and give it a real 64 KiB ROM at
`F000:0000` so it tests the trap path rather than a synthetic `IRET`. If
dropped, delete it and remove the Makefile targets — a test target that
cannot run is worse than no target, because it looks like coverage.

**Done when:** `make firmware-test` exits 0 on a clean checkout.

3. **It fails the repository's own lint.** `npx eslint tests/firmware`
   reports 59 errors. This predates the current work — it is present at
   HEAD (`8a9f739d`) in files nobody has touched since — so `make eslint`
   was already red before the ROM firmware landed. Worth knowing before
   anyone reads a red lint run as a regression from this work.

### JIT-1 — SeaBIOS cannot boot under the v86 JIT

**Status:** Broken. **Severity:** Medium.

```
$ FW_ORACLE=seabios node examples/firmware-oracle.js
RuntimeError: table index is out of bounds
      at v86.wasm.main_loop
      at v86.do_tick (src/main.js:54:24)
```

The built-in firmware boots under the JIT; SeaBIOS does not. Verified not to
be firmware-related — it reproduces with `firmware` unset (so
`firmware_present() == 0`), and with a boot sector that is only `cli; hlt`,
so the crash is inside SeaBIOS's own POST. The fix belongs in the JIT.

It could not be bisected against `8a9f739d`, because HEAD does not compile
(48 errors, see `Changes.md` §1). "Pre-existing" is inferred, not proven.

**Done when:** `FW_ORACLE=seabios node examples/firmware-oracle.js` passes
without `FW_NO_JIT`.

### BUILD-3 — Generated Rust sources are not committed

`src/rust/gen/*.rs` (`jit`, `jit0f`, `interpreter`, `interpreter0f`,
`analyzer`, `analyzer0f`) are generated and gitignored, so a build
requires `node ./gen/generate_jit.js` and friends to have been run.
This is pre-existing, not firmware-introduced, but the new firmware
raises the cost of getting it wrong. **Severity:** Low.

---

## Deliberately not debt

Recorded so these are not re-litigated:

- **No UEFI.** OVMF/EDK II is BSD-2-Clause-Patent and is the right
  answer for a UEFI path (`todo.md` §2.7), but it does not replace a
  legacy BIOS and would not satisfy a DOS/Windows 3.x guest. See
  `docs/firmware-selection.md`.
- **No copyleft.** SeaBIOS (GPL-2.0-or-later), Bochs VGABIOS
  (LGPL-3.0), GLaBIOS (GPL-3.0) and coreboot stay out of the permissive
  build. The SeaBIOS fallback remains a separately-fetched asset, as
  `bios/README.md` already documents.
- **No third-party ROM images.** PCjs's MIT grant covers code, not the
  IBM ROM dumps in its repository — its `LICENSE.txt` excludes "all
  programs, images and documentation produced by other parties". PCjs's
  *code* is what this firmware is modelled on.
- **Not every INT 13h function.** Unimplemented services return a real
  error (`AH=01h`, CF set) rather than silently succeeding. That is a
  feature; guests fall back.
- **Thin ROMs.** A 168-byte system ROM is the design, not an oversight:
  leaf services live on the host. It only becomes debt at ROM-1, and
  that is recorded there.