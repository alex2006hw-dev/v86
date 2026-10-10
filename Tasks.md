# Tasks.md — Replace License-Restricted Firmware & Modernize PCjs BIOS for v86

**Version:** 1.0 · **Date:** 2026-10-09
**Goal:** completely remove all copyleft (GPL/LGPL) components from the
shipped v86 product by replacing SeaBIOS (GPL-2.0+) and Bochs VGABIOS
(LGPL) with a modernized, PCjs-derived (MIT) BIOS, and extend it with
modern firmware features and 2026 web platform capabilities.

**Inputs:** `todo.md` (project plan), `docs/pcjs-bios-analysis.md`
(PCjs architecture/service inventory), `docs/bios-alternatives.md`
(firmware selection), `docs/pcjs-modern-upgrade.md` (2026 feature plan),
`THIRD-PARTY-NOTICES.md` (license audit).

## Licensing guardrails (apply to every task)

1. v86 code is **BSD-2-Clause** — used as-is, retain `LICENSE`.
2. PCjs firmware components are **MIT** (Jeff Parsons © 2012–2026) —
   vendor with attribution; display "PCjs © 2012-2026 Jeff Parsons"
   (pcjs.org + GitHub links) in the host UI.
3. All new firmware features are implemented **from public
   specifications only** (EDD, El Torito/ISO9660, VESA VBE 3.0, PCI
   BIOS, ACPI 1.0+, USB 1.1/2.0, UEFI) → **new MIT work**.
4. **Never** copy SeaBIOS, VGABIOS, OpenBIOS, coreboot, or any
   GPL/LGPL firmware — no code, tables, register sequences, or
   structure. QEMU is a behavioral **oracle only** (its tests are
   GPL).
5. Any vendored third-party code must be MIT/BSD/Apache and listed in
   `THIRD-PARTY-NOTICES.md`.

---

## Phase 0 — Foundation & baseline audit (2–3 weeks)

### T0.1 Vendor PCjs firmware components
- **Description:** import PCjs firmware-relevant modules
  (`interrupts.js`, `chipset.js`, `rom.js`, `ram.js`, `keyboard.js`,
  `video.js`, `disk.js`, `fdc.js`, `hdc.js`, `serial.js`, `parallel.js`,
  `mouse.js`, `computer.js`, `component.js`, `defines.js`) into
  `src/rust/firmware/pcjs/` (JS reference) or a vendor directory.
- **Deliverable:** vendor tree + PCjs MIT notice + README attribution
  in `THIRD-PARTY-NOTICES.md`.
- **Acceptance:** all vendored files carry the MIT header comment;
  attribution string present.
- **Effort:** 2 days.

### T0.2 Baseline audit of existing PCjs support
- **Description:** verify and document what PCjs already implements:
  ATAPI CD-ROM (2019, incomplete for Win95-era), Microsoft Mouse
  2.0–6.x, VGA text/graphics (CGA/MDA/EGA/VGA), DPMI-era guests,
  MSCDEX-style CD data access. Identify exact gaps vs. this plan.
- **Deliverable:** audit notes appended to `docs/pcjs-bios-analysis.md`.
- **Acceptance:** gap list signed off; no feature reimplemented before
  audit confirms it missing.
- **Effort:** 1 week.

### T0.3 Firmware module scaffolding (Rust/WASM) ✅
- **Description:** create the firmware crate/module under
  `src/rust/firmware/` with: interrupt dispatch hook (CPU → BIOS
  service layer), BIOS Data Area (1 KB struct at physical 0x400),
  vector table (IVT at 0x0000), POST state machine, and the
  `Component`/`initBus()` equivalent (Rust device traits + bus
  registration).
- **Deliverable:** `firmware` module skeleton; bus trait; BDA struct.
- **Acceptance:** a hand-written boot sector (0xAA55) executes INT 10h
  AH=0Eh (tty print) through the new dispatch path in a test harness.
- **Effort:** 1 week.
- **Depends on:** T0.1, T0.3 scaffolding.
- **Status:** COMPLETE. Crate at `src/rust/firmware/` with 31 passing
  tests. All modules implemented: machine, backend, bda, ivt,
  eltorito, int13, int10, vbe, int15, int16, int1a, int14, int17,
  post, dispatch. Adapter at `src/rust/fw_adapter.rs`.

### T0.4 Emulator backend interfaces (v86-side glue)
- **Description:** define the WASM-import backend interface the firmware
  needs: `block_read(device, lba, sectors)`, `block_write`,
  `display_framebuffer(ptr, w, h, stride)`, `display_text(buf)`,
  `input_key(scancode, down)`, `input_mouse(dx, dy, buttons)`,
  `rtc_get/set`, `nvram_read/write`, `audio_sample(buf)`.
- **Deliverable:** backend trait + JS glue in `src/browser/starter.js`
  wiring to existing IDE/floppy/virtio, canvas, PS/2, speaker backends.
- **Acceptance:** every backend has a mock implementation for tests.
- **Effort:** 1 week.
- **Also:** extend `v86.d.ts` with `firmware: "pcjs" | "rom" | "none"`
  and machine profile options (`"pc" | "xt" | "at" | "compaq386"`).

---

## Phase 1 — Core BIOS service layer (PCjs port) (4–6 weeks)

Port order: services first, video last (largest/riskiest).

### T1.1 Interrupts & BIOS Data Area
- Port `interrupts.js`: vector definitions (INT 10h–1Fh, 0x6D VGA ROM
  convention, WINDBG/WINDBGRM vectors), BDA layout (0x400–0x4A8:
  RS232/PRINTER bases, EQUIP_FLAG, MEMORY_SIZE 0x413, keyboard flags
  and 32-byte buffer 0x417–0x41E, CRT_MODE 0x449, CRT_COLS 0x44A,
  cursor positions 0x450, 6845 base 0x463, timer 0x46C, reset flag
  0x472).
- **Acceptance:** BDA readable/writable from guest; vector reads return
  correct service addresses. **Effort:** 1 week. **Depends on:** T0.3.

### T1.2 Chipset / POST / boot sequencing
- Port `chipset.js` + `computer.js` supervision: POST memory test
  (skippable for speed), equipment word, RAM sizing, INT 19h
  bootstrap (load boot sector from boot device to 0x7C00, jump),
  add-on ROM scan (0xC0000–0xEFFFF, 2KB alignment, checksum 0).
- **Acceptance:** guest boots a boot sector from the emulated floppy
  and hard disk. **Effort:** 1 week. **Depends on:** T1.1, T0.4.

### T1.3 Keyboard (INT 16h)
- Port `keyboard.js`: scancode set 1, 16-entry keyboard buffer
  (0x41E–0x43D), shift states (0x417–0x418), INT 16h functions
  00h–03h (read/read-status/read-peek), BIOS keyboard interrupt
  (IRQ 1) injection from the WASM keyboard backend.
- **Acceptance:** DOS `TYPE CON` and `PROMPT` work end-to-end.
- **Effort:** 3 days. **Depends on:** T1.1, T0.4.

### T1.4 Floppy & disk (INT 13h CHS)
- Port `fdc.js` + `disk.js`: INT 13h AH=00h (reset), 01h (status),
  02h/03h (read/write sectors, CHS: DL/C/H/S/AL → ES:BX), 04h
  (verify), 05h (format track), 08h (drive parameters), 15h (DASD
  type), 16h (media change); DMA 8237 wiring; motor/seek state in
  BDA (0x43E–0x442).
- **Acceptance:** DOS boots from emulated floppy; disk image
  round-trip via INT 13h matches the image bytes. **Effort:** 1 week.
- **Depends on:** T1.2, T0.4.

### T1.5 Hard disk controller (hdc.js)
- Port `hdc.js`: ATA/IDE command interface, INT 13h hard-disk
  functions, Fixed Disk Parameter Table (0x41 vector, FDPT layout),
  hard-disk status bytes (BDA 0x474–0x48F).
- **Acceptance:** DOS partitions and formats an emulated IDE disk.
- **Effort:** 3 days. **Depends on:** T1.4.

### T1.6 Video (INT 10h) — largest task
- Port `video.js`: MDA/CGA/EGA/VGA text and graphics modes; INT 10h
  AH=00h–0Fh (set mode, cursor type/pos, display page, scroll,
  read/write char+attr, palette, write/read dot, tty); CRTC (6845)
  register model; EGA/VGA planar memory and palette (DAC); VGA ROM
  convention (INT 0x6D); font loading (INT 10h AH=11h); the
  CRT timing model (h/v sync, blanking) used by PCjs.
- **Acceptance:** DOS text mode 80×25, mode 13h (320×200×256), and
  the PCjs VGA "Black Book" test suite pass. **Effort:** 2–3 weeks.
- **Depends on:** T1.1, T0.4, T0.2 audit.

### T1.7 Serial/parallel (INT 14h/17h)
- Port `serial.js`/`parallel.js`: INT 14h (init/send/receive/status),
  INT 17h (print/status/reset), 16550A register model, BDA RS232
  bases (0x400) and printer bases (0x408).
- **Acceptance:** DOS prints to LPT1; serial echo via the WASM serial
  backend. **Effort:** 3 days.

### T1.8 Mouse
- Port `mouse.js`: Microsoft/Bus mouse protocol + PS/2 mouse via
  auxiliary port (0x60/0x64 commands A6h–A9h, sample rate, streaming),
  INT 15h C2xx functions, 3-wheel Explorer support.
- **Acceptance:** `mouse.com` works in DOS; 3-wheel reports.
- **Effort:** 4 days. **Depends on:** T0.4.

### T1.9 CMOS/RTC (MC146818)
- New implementation from the MC146818 datasheet: full register file
  (0x00–0x0F time/date, 0x11–0x1F status/control, 0x32 century),
  NVRAM 0x0E–0x7F + extended with persistence, INT 1Ah functions
  (00h RTC time, 01h/02h alarm, 03h memory size, 88h extended
  memory), update-ended interrupt, periodic alarm IRQ 8, UTC/local
  century handling.
- **Acceptance:** DOS `DATE`/`TIME` correct; NVRAM survives reload;
  alarm fires. **Effort:** 1 week.

**Phase 1 exit:** MS-DOS 6.22, Windows 3.1, and Minix boot and run
with the PCjs-derived firmware; SeaBIOS still available as fallback
for comparison.

---

## Phase 2 — Modern firmware features, P0 (boot-path completeness) (6–8 weeks)

### T2.1 INT 13h Extensions (EDD / LBA)
- **Spec:** BIOS Enhanced Disk Drive Services (EDD). Implement:
  AH=41h install check (BX=55AAh on success, CX = supported-function
  bitmask, DH = version), AH=42h extended read, AH=43h extended
  write, AH=44h verify, AH=45h lock/unlock/eject, AH=46h seek,
  AH=47h get drive parameters; **Disk Address Packet** (16 bytes:
  size, reserved, sector count ≤127, buffer seg:off, 8-byte LBA);
  CHS↔LBA translation from drive geometry.
- **Acceptance:** GRUB or Limine boots from a large (>8 GB) emulated
  disk via AH=42h; 64-sector DAP batching correct; error codes per
  spec. **Effort:** 2 weeks. **Depends on:** T1.4, T1.5.

### T2.2 El Torito ISO boot
- **Spec:** ISO9660 + El Torito (volume descriptor type 0 boot record,
  boot catalog: validation entry, default entry, section entries;
  no-emulation mode; 2048-byte sectors; emulated/1.2M/1.44M/2.88M
  floppy and hard-disk emulation modes). Expose the ISO as INT 13h
  drives 0x00–0x0F with the El Torito boot catalog parsed at POST.
- **Acceptance:** a bootable DOS ISO and a Linux ISO (no-emulation)
  boot with **no SeaBIOS present**; media-change detection on
  eject/swap. **Effort:** 2 weeks. **Depends on:** T2.1, T0.4
  (CD backend).

### T2.3 VBE 2.0+/3.0
- **Spec:** VESA BIOS Extensions 3.0. Implement INT 10h AX=4F00h
  (controller info, "VESA VBE 3.0" signature string), 4F01h (mode
  info: resolution, bpp, memory model, LFB address, windowing),
  4F02h (set mode, bit 14 = linear framebuffer, bit 15 = preserve
  display), 4F03h (get mode), 4F04h (save/restore state), 4F05h
  (CPU video memory window), 4F06h (scanline length), 4F07h
  (display start), 4F08h (DAC palette format), 4F09h (palette data);
  static mode table matching the emulated VGA.
- **Acceptance:** VBEtest/SVGATest pass; a VBE-aware DOS app
  (e.g., SciTech Display Doctor test) sets LFB modes; 640×480×16/24
  and 800×600/1024×768 modes render correctly. **Effort:** 2 weeks.
- **Depends on:** T1.6.

### T2.4 Memory map & A20
- INT 15h AX=E820h (SMAP entries: 24-byte descriptors, 3 entries
  minimum: 0–0x9FC00 usable, 0x9FC00–0x100000 reserved, extended
  RAM), AX=E801h (extended memory up to 4 GB, 16 KB granularity),
  AH=88h (extended memory, legacy), INT 15h AH=52h/A20 gate via
  port 0x92 and keyboard controller 0xD1/0xDF.
- **Acceptance:** Linux boot protocol `setup.S` memory queries succeed;
  A20 toggle verified from guest code. **Effort:** 1 week.
- **Depends on:** T1.9.

**Phase 2 exit:** ISO boot and large-disk boot work with the permissive
firmware only; SeaBIOS becomes an optional compatibility download.

---

## Phase 3 — Modern guests, P1 (5–7 weeks)

### T3.1 PCI BIOS (INT 1Ah B1xx)
- **Spec:** PCI BIOS specification (PCI SIG). Implement AX=B101h
  (find device by ID/class, iterative search), B102h (read config
  byte/word/dword), B103h (write config), B104h (interrupt routing
  table), B105h (generate special cycle), B106h (read interrupt
  line/pin); backed by the existing `pci.js` device bus; BIOS-provided
  config mechanism #1 (ports 0xCF8/0xCFC).
- **Acceptance:** a guest enumerates the emulated NE2000/IDE devices
  via B101h/B102h and assigns I/O ranges. **Effort:** 2 weeks.
- **Depends on:** T1.2, existing `pci.js`.

### T3.2 APIC (IOAPIC + LAPIC)
- **Spec:** Intel MP specification / ACPI MADT. IOAPIC: MMIO at
  0xFEC00000 (register select 0x00, window 0x10), 24 redirection
  entries (mask, trigger, polarity, delivery mode, destination).
  LAPIC: MMIO at 0xFEE00000 (ID 0x20, version 0x30, TPR 0x80,
  spurious 0xF0, LVT timer 0x320, timer initial/current 0x380/0x390,
  divide 0x3E0, error 0x370); x2APIC optional. IRQ routing from
  legacy PIC to IOAPIC redirection entries.
- **Acceptance:** Linux with `lapic` boot parameter boots; timer
  interrupts delivered; SMP guest detects CPUs. **Effort:** 2 weeks.
- **Depends on:** T3.1.

### T3.3 ACPI tables
- **Spec:** ACPI 1.0+ (OEM-facing parts of the spec are public).
  Build RSDP (signature "RSD PTR ", checksum, OEM ID, revision,
  RSDT/XSDT address), RSDT, FACP (PM base I/O ports, SCI IRQ,
  SMM/reset register), DSDT with minimal scope (power button, sleep
  control), MADT (from T3.2), HPET optional; expose via fw_cfg or
  guest-visible memory at a fixed address; ECDT/BOOT preferred where
  applicable.
- **Acceptance:** Linux boots with ACPI enabled (no `acpi=off`);
  power button event delivered. **Effort:** 2 weeks.
- **Depends on:** T3.2.

### T3.4 ATA/ATAPI hardening
- Complete the PCjs ATAPI path: packet commands (INQUIRY, READ
  CAPACITY, READ(10)/(12), MODE SENSE, TEST UNIT READY, REQUEST
  SENSE, START/STOP), media-change notification, LBA48 (48-bit) for
  large disks, DMA vs PIO modes.
- **Acceptance:** Win95-era CD software (per T0.2 audit findings)
  installs and runs; 48 GB+ disk image accessible. **Effort:** 1 week.
- **Depends on:** T1.5, T0.2.

### T3.5 PS/2 mouse completion
- PS/2 mouse: set sample rate, set resolution, set scaling 2:1,
  enable/disable data reporting, 4-byte packet format, wheel and
  5-button (Explorer) extensions, INT 15h C2xx mirror functions.
- **Acceptance:** DOS mouse drivers (mouse.com, Cutemouse) work with
  wheel. **Effort:** 3 days. **Depends on:** T1.8.

**Phase 3 exit:** modern Linux (kernel 5.x/6.x) boots to a shell via
the permissive firmware path (fw_cfg direct boot + APIC/ACPI for
full features).

---

## Phase 4 — Peripheral boot, P2 (3–4 weeks)

### T4.1 USB UHCI + mass-storage boot
- **Spec:** USB 1.1 UHCI controller (I/O ports 0x200–0x3FF, frame
  list, frame number, done-head), USB mass-storage BOT protocol
  (31-byte CBW, 13-byte CSW, command/data/status stages), USB HID
  keyboard/mouse; expose USB drives via INT 13h EDD (T2.1).
- **Acceptance:** a USB image boots; a USB keyboard types in DOS.
- **Effort:** 2 weeks. **Depends on:** T2.1.

### T4.2 OVMF/UEFI integration
- Integrate TianoCore EDK2/OVMF (**BSD-2-Clause-Patent**) as an
  alternative firmware image for UEFI guests; IA-32 build for 32-bit
  guests; ship as an optional permissive download.
- **Acceptance:** a UEFI Linux bootloader (e.g., GRUB EFI or the
  Linux EFI stub) boots. **Effort:** 1–2 weeks.
- **Depends on:** T3.1–T3.3 (PCI/ACPI for a realistic machine model).

**Phase 4 exit:** USB boot and UEFI boot available; all boot paths
(fw_cfg, PCjs BIOS, ROM images, UEFI) selectable via `firmware`
option.

---

## Phase 5 — Web platform 2026 integration (4–6 weeks, parallel with Phases 3–4)

### T5.1 Wasm SIMD / Relaxed SIMD hot paths
- Vectorize: mode-X/planar → RGBA conversion, font/glyph blits,
  scalers (2xSaI, scanline), softfloat x87 helpers, CRC32 and
  ISO9660 directory parsing, VBE palette LUTs. Use Emscripten
  `-msimd128` (SSE2 intrinsic mapping) or Rust `std::simd`/wasm
  intrinsics; Relaxed SIMD where edge-case semantics permit.
- **Acceptance:** ≥2× speedup on blit-bound VGA workloads (mode 13h
  games) with SIMD bundle vs. baseline; identical pixels. **Effort:** 2 weeks.

### T5.2 Threads + SharedArrayBuffer + OffscreenCanvas
- Split CPU, video, and audio into workers; share guest RAM via
  `SharedArrayBuffer` + `Atomics`; render on `OffscreenCanvas` in the
  video worker; require COOP/COEP headers with graceful degradation
  (single-thread fallback bundle).
- **Acceptance:** main thread stays <8 ms frame budget during CPU-heavy
  guests; input latency unchanged. **Effort:** 2 weeks.

### T5.3 Wasm exception handling + multi-bundle shipping
- Use Wasm 3.0 structured exceptions (exnref) for firmware error
  paths (INT 13h error returns, POST failures); ship 4 bundles —
  baseline, `threads`, `threads+simd`, `threads+simd+exceptions`
  (+ `memory64` variants) — selected at runtime with
  `wasm-feature-detect` (MIT).
- **Acceptance:** all bundles pass the Phase 1–3 test matrix
  identically (cross-bundle differential testing, T6.2). **Effort:** 1 week.

### T5.4 Memory64 + multiple memories
- Guest RAM >4 GB via Memory64 (16 GB browser cap); split guest RAM /
  VRAM / BDA / NVRAM into separate Wasm memories for save-state
  isolation and snapshot speed.
- **Acceptance:** a >4 GB guest RAM configuration boots a guest that
  detects the memory; save/restore round-trip with threads. **Effort:** 1 week.

### T5.5 Web APIs
- **File System Access:** drag-and-drop disk images, persistent file
  handles with write-back, save-state snapshots via
  `showSaveFilePicker`.
- **WebUSB:** optional USB device passthrough (user gesture +
  permission) for testing the T4.1 stack.
- **WebSerial:** serial-console passthrough for the 16550A backend.
- **WebAudio:** SB16/speaker via `AudioWorklet`.
- **Service worker:** offline cache of `v86.wasm`, firmware blobs,
  boot images (PWA behavior).
- **Acceptance:** each API has a feature-detected fallback; demo page
  exercises all five. **Effort:** 2 weeks.

### T5.6 WASI 0.3 server/CLI build
- Build a WASI 0.3 (Feb 2026, native async/futures) variant of the
  emulator + firmware for Node/Deno/CLI/server-side use; headless CI
  boot tests without a browser.
- **Acceptance:** the T6.1 test matrix runs headless in CI.
- **Effort:** 1 week.

---

## Phase 6 — Testing, licensing close-out & release (3–4 weeks)

### T6.1 Test matrix
- El Torito ISO boot (bootable DOS ISO, Linux ISO no-emulation,
  CD data access under MSCDEX).
- LBA: GRUB/Limine via AH=42h/43h; 127-sector DAP batching; >8 GB
  disk image; LBA48 (T3.4).
- VBE: VBEtest/SVGATest, LFB mapping, DAC palette accuracy.
- CMOS/RTC: DOS DATE/TIME, alarm IRQ, NVRAM persistence.
- PS/2 mouse: mouse.com, wheel/3-button.
- PCI: guest enumeration of NE2000/IDE via B101h/B102h.
- APIC/ACPI: Linux `lapic`/ACPI boot paths.
- Legacy: DOS 3.3/4.0/6.22, Windows 1.0/2.0/3.1/95, Minix 1.x/2.x
  (PCjs-era guests).
- Fallback: SeaBIOS + VGABIOS separate-fetch still boots (T2.8).
- **Acceptance:** all green on all wasm bundles. **Effort:** 2 weeks.

### T6.2 Cross-bundle differential testing
- Run the T6.1 matrix against every shipped bundle (baseline,
  threads, threads+simd, threads+simd+exceptions, memory64);
  pixel-exact and memory-state comparisons.
- **Acceptance:** zero behavioral divergence. **Effort:** 1 week.

### T6.3 Copyleft boundary verification
- Scan the shipped distribution (wasm, JS, firmware blobs, npm
  package) for GPL/LGPL code: `license-checker`/FOSSA-style scan +
  manual review of `bios/` (must be separate-fetch only) and
  `tests/qemu/` (oracle only, never bundled).
- **Acceptance:** scan report clean; `THIRD-PARTY-NOTICES.md` final.
- **Effort:** 3 days.

### T6.4 Attribution & UI
- Display "PCjs © 2012-2026 Jeff Parsons" with pcjs.org and GitHub
  links in the host UI (About panel/footer) per the PCjs README
  requirement; retain v86 BSD-2 notice; b-dmitry1/BIOS notice when
  the ROM path is used.
- **Acceptance:** attribution visible in the default UI and in
  headless/CLI mode. **Effort:** 2 days.

### T6.5 Release
- npm + GitHub releases (`v86.wasm`, `libv86.mjs`, firmware
  packages: `firmware-pcjs.wasm`, optional `firmware-ovmf.wasm`);
  tag `v0.2.0` (firmware-only change from the v86 0.5.x lineage);
  migration guide (v86 → permissive firmware; `firmware` option);
  docs update.
- **Effort:** 1 week.

---

## Dependency graph & critical path

```
T0.1/T0.2 → T0.3 → T1.1 → T1.2 → T1.4 → T1.5 → T1.6 → T2.3
                                  ↘ T1.3, T1.7, T1.8, T1.9
T1.4/T1.5 → T2.1 → T2.2 (El Torito)   ← removes SeaBIOS dependency
T1.6 → T2.3 (VBE)
T1.9 → T2.4 (E820/A20)
T1.2 + pci.js → T3.1 → T3.2 → T3.3 (ACPI)
T2.1 → T4.1 (USB boot);  T3.1–T3.3 → T4.2 (OVMF)
T0.4 → T5.x (web platform, parallel)
All → T6.1 → T6.2 → T6.3 → T6.4 → T6.5
```

**Critical path:** T0.3 → T1.1 → T1.2 → T1.4 → T2.1 → T2.2
(El Torito boot is the highest-value milestone: it eliminates the
last copyleft dependency for the primary ISO-boot use case).

## Effort summary

| Phase | Scope | Effort |
|---|---|---|
| 0 | Foundation & audit | 2–3 weeks |
| 1 | Core BIOS service layer (PCjs port) | 4–6 weeks |
| 2 | P0 modern firmware (EDD, El Torito, VBE, E820) | 6–8 weeks |
| 3 | P1 modern guests (PCI, APIC, ACPI, ATAPI) | 5–7 weeks |
| 4 | P2 peripheral boot (USB, OVMF) | 3–4 weeks |
| 5 | Web platform 2026 (SIMD, threads, APIs, WASI) | 4–6 weeks (parallel) |
| 6 | Testing, licensing, release | 3–4 weeks |
| **Total** | | **~6–9 months** (Phases 5 parallel) |

## Risks & mitigations

| Risk | Mitigation |
|---|---|
| PCjs video.js complexity (CRT timing, EGA/VGA planes) | Port last; reuse PCjs VGA "Black Book" tests; VGABIOS fallback during development |
| ATAPI CD-ROM incomplete for Win95-era software (known PCjs gap) | T0.2 audit first; T3.4 hardening from the ATAPI spec; oracle-test against QEMU |
| SIMD/threads bundle behavior divergence | T6.2 cross-bundle differential testing with pixel-exact comparison |
| SharedArrayBuffer requires COOP/COEP headers | Single-thread fallback bundle; hosting guide |
| PCjs README attribution clause interpretation | Counsel review (todo §1.4) before release; display attribution regardless |
| El Torito edge cases (multi-boot catalogs, section entries) | Test against a broad ISO corpus; QEMU oracle |
| SeaBIOS/VGABIOS accidental derivation | Guardrail #4; code review checks; T6.3 scan |

## Definition of done

1. Every boot path (fw_cfg direct, PCjs-derived BIOS, El Torito ISO,
   LBA disk, USB, UEFI/OVMF) works with **zero copyleft** in the
   shipped distribution.
2. Legacy guest suite (DOS 3.3–6.22, Windows 1.0–3.1/95, Minix)
   and modern Linux pass on all wasm bundles.
3. `THIRD-PARTY-NOTICES.md` final; PCjs attribution displayed in UI;
   counsel sign-off (todo §1.4) recorded.
4. SeaBIOS/VGABIOS remain available only as clearly-labeled,
   separately-fetched GPL/LGPL compatibility assets.
