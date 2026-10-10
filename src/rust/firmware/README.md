# v86-firmware

Permissively-licensed (MIT) PC/XT/AT BIOS firmware for the v86 emulator.

This crate is the clean-room replacement for the GPL/LGPL firmware images
(SeaBIOS, Bochs VGABIOS) previously used by v86. It is derived from:

- **PCjs** (MIT, © 2012–2026 Jeff Parsons) — the PCx86 BIOS service model
  (component architecture, interrupt service layout, BIOS Data Area usage).
  The PCjs attribution notice must be displayed by the host UI.
- **Public specifications** — all modern features are implemented from the
  specifications listed below, not from any GPL/LGPL firmware.

## Specifications implemented

| Feature | Specification |
|---|---|
| INT 10h/11h/12h/13h/14h/15h/16h/17h/18h/19h/1Ah services | IBM PC/XT/AT BIOS functional interface (interrupt vector numbers, register contracts, BIOS Data Area layout are functional facts) |
| INT 13h Extensions (EDD) | T10 "Enhanced Disk Drive Services" spec (BIOS EDD 1.0/3.0) |
| El Torito / ISO 9660 boot | Phoenix/IBM "El Torito Bootable CD-ROM Format Specification", ECMA-119 (ISO 9660) |
| VESA BIOS Extensions | VESA VBE 1.2/2.0/3.0 (INT 10h AX=4Fxx, mode table, mode info structure) |
| MC146818 CMOS/RTC | MC146818 datasheet |
| A20 gate | PC/AT keyboard controller (8042) output-port semantics; PS/2 system control port A (0x92) |
| E820 memory map | INT 15h AX=E820h "SMAP" interface (as standardized in the ACPI spec's System Address Map Interfaces chapter) |

## Architecture

The firmware is a **host-side service layer**, not guest ROM code:

- `Machine` (`machine.rs`) — abstract interface to the emulated machine
  (physical memory, registers, flags, keyboard/input hooks, RTC clock).
  Implemented by the host emulator adapter and by test doubles.
- `BlockBackend` (`backend.rs`) — abstract block device (floppy, hard disk,
  CD-ROM, El Torito virtual disks). Implemented by the host emulator's
  disk backends.
- `dispatch.rs` — `bios_interrupt()`: the entry point the emulator's CPU
  calls for real-mode software interrupts to BIOS service vectors.
- Service modules: `int10` (video), `int13` (disk, CHS + EDD/LBA +
  El Torito emulation control), `int15` (memory map, A20), `int16`
  (keyboard), `int1a` (RTC/CMOS), `post` (POST + INT 19h bootstrap),
  `vbe` (VESA BIOS Extensions), `eltorito` (ISO 9660/El Torito parser).

## Boot model

1. The emulator calls `Firmware::power_on()` (equivalent of reset + POST).
2. POST zeroes the IVT, initializes the BIOS Data Area, registers drives,
   parses El Torito boot catalogs of attached CD images, writes firmware
   marker entries into the IVT for the BIOS service vectors, and executes
   the INT 19h bootstrap.
3. INT 19h tries the configured boot order (floppy → hard disk → CD).
   Legacy boot loads sector 0 to 0000:7C00 and jumps there with DL set.
   El Torito no-emulation boot loads the boot image to its load segment
   and jumps with DL set to the CD-ROM drive number (0x00–0x0F range
   per the El Torito spec).
4. Guest software invokes BIOS services via `INT nn`. The emulator's CPU
   intercepts real-mode software interrupts whose IVT entry points into
   the firmware marker area (0xF0000–0xFFFFF) and dispatches them to
   `bios_interrupt()`. If guest software re-vectors an interrupt
   (points the IVT entry at its own code), the firmware declines to
   intercept and the guest handler runs — matching real hardware.

## Testing

`cargo test` runs the unit tests (El Torito parsing, CHS↔LBA, DAP
handling, BDA layout, EDD parameter structures) against an in-memory
test machine.
