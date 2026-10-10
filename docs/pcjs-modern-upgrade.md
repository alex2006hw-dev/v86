# PCjs Firmware — Modern Feature Upgrade Plan (2026)

**Scope:** upgrade the PCjs-derived (MIT) firmware adopted in
this project with modern firmware features and 2026 web
platform capabilities. New work is derived from **public
specifications only** (EDD, El Torito, VESA VBE 3.0, PCI
BIOS, ACPI, USB, UEFI) — never from SeaBIOS/VGABIOS
(GPL/LGPL). All new code is MIT.

## 1. Baseline audit (do first — PCjs already ships much of this)

PCjs (as of 2026) already provides:
- **ATAPI CD-ROM** (added 2019; IBM PC AT + ATAPI CD-ROM
  machine; known incomplete for Win95-era software — audit
  and harden before reimplementing).
- **Microsoft Mouse 2.0–6.x** (bus/serial mouse; PS/2
  mouse path to verify).
- **VGA "Black Book" tests**, PCx86 CPU tests, TestMonitor
  (INT 14h TSR) — reuse as test oracles.
- **DPMI spec documents** archived; DOS extender-era guests
  (Win 3.1/95) already run.
- Component model (`Component`/`initBus()`), machine
  catalog (PC/XT/AT, Compaq 286/386, Zenith Z-150,
  AT&T 6300, PCjr), disk-image tooling.

**Gaps (confirmed):** no INT 13h EDD/LBA, no El Torito
*boot* path (CD-ROM is data-only via MSCDEX-style), no VBE
2.0+/3.0 LFB, no PCI BIOS, no APIC/ACPI, no USB, no UEFI,
no CMOS/RTC NVRAM persistence, no E820 memory map.

## 2. Firmware feature upgrades (priority order)

### P0 — boot-path completeness (removes the SeaBIOS dependency)
- [ ] **INT 13h Extensions (EDD)** per the BIOS Enhanced
      Disk Drive spec: AH=41h install check (BX=55AAh,
      CX capability flags), AH=42h extended read, AH=43h
      extended write, AH=44h verify, AH=45h lock/unlock,
      AH=46h eject, AH=47h seek, AH=48h get drive
      parameters; **Disk Address Packet** (16 bytes: size,
      reserved, sector count, buffer seg:off, 8-byte LBA);
      CHS↔LBA translation per geometry.
- [ ] **El Torito ISO boot**: parse the ISO9660 boot
      record, "no emulation" mode, expose the CD as INT 13h
      drive 0x00–0x0F with 2048-byte sectors; boot catalog
      validation (validation entry, default entry, section
      entries). → **ISO boot with zero copyleft firmware.**
- [ ] **VBE 2.0+/3.0** (INT 10h AX=4Fxx): controller info
      (4F00h), mode info (4F01h), set mode (4F02h, incl.
      linear framebuffer bit 14), get/set DAC palette
      (4F08h/4F09h), get/set display start (4F07h),
      return VBE string "VESA VBE 3.0 ..." in the mode
      string; mode list from a static table matching our
      emulated VGA.
- [ ] **CMOS/RTC (MC146818)**: full register file + NVRAM
      persistence, INT 1Ah (0=RTC time, 1=alarm, 2=memory
      map legacy, 0x88 extended memory, 0xE820 system memory
      map with SMAP entries), century register, periodic
      alarm IRQ 8, update-ended interrupt.
- [ ] **E820/E801/88 memory map** + equipment word
      (0x410), BIOS Data Area completion, A20 gate control
      (INT 15h A2h / port 0x92), POST memory test (skip
      option for speed).

### P1 — modern guest support
- [ ] **PCI BIOS** (INT 1Ah B1xx): B101h find device,
      B102h read config byte/word/dword, B103h write config,
      B104h interrupt routing, B105h special cycle; backed
      by the existing `pci.js` device bus.
- [ ] **APIC**: IOAPIC (24 redirection entries, I/O APIC
      register window 0xFEC00000) + LAPIC (memory-mapped at
      0xFEE00000, spurious-vector enable, timer, error
      register) — gates Linux SMP and modern Linux boot.
- [ ] **ACPI**: RSDP/RSDT/FACP/DSDT/SSDT tables with a
      minimal DSDT (power button, sleep), PM base I/O ports,
      SCI IRQ; HPET table optional.
- [ ] **ATA/ATAPI hardening**: complete the PCjs ATAPI
      path (packet commands, INQUIRY, READ CAPACITY, READ
      10/12, MODE SENSE, TEST UNIT READY), media-change
      notification, LBA48 for large disks.
- [ ] **PS/2 mouse** via auxiliary-port 0x60/0x64 commands
      (A6h/A7h/A8h/A9h, set sample rate, enable streaming,
      INT 15h C2xx alternative) + IMPS/Explorer 3-wheel.

### P2 — peripheral boot
- [ ] **USB UHCI** controller + USB mass-storage boot
      (INT 13h on USB drive, BOT protocol) and USB HID
      keyboard/mouse; optional WebUSB passthrough (§3.5).
- [ ] **UEFI path**: integrate OVMF/EDK2 (BSD-2-Clause-
      Patent) as an alternative firmware image for modern
      guests (todo §2.7).

## 3. Web platform integration (2026 capabilities)

### 3.1 WebAssembly 3.0 features (standardized Sept 2025)
- [ ] **SIMD (128-bit, Relaxed SIMD)**: vectorize the
      hot paths — mode-X/planar → RGBA conversion,
      font/glyph rendering, scalers (2xSaI/scanline),
      softfloat x87 helpers, CRC32/ISO9660 parsing, VBE
      palette LUTs. Emscripten `-msimd128` maps SSE2
      intrinsics; measure VGA blit speedup (expect 2–8x
      on blit-bound guests).
- [ ] **Threads + SharedArrayBuffer**: run CPU, video
      and audio in separate workers; share guest RAM via
      `SharedArrayBuffer` + `Atomics` (requires COOP/COEP
      headers); render on **OffscreenCanvas** in the video
      worker to keep the main thread free for input.
- [ ] **Exception handling (exnref, Wasm 3.0)**: use
      structured Wasm exceptions for firmware error paths
      (INT 13h error returns, BIST failures) instead of
      JS-side exceptions; ship JS-exception fallback
      bundles for Safari <18.4 / older Firefox.
- [ ] **Memory64**: guest RAM >4 GB (browser cap 16 GB)
      for large-server guests; also **multiple memories**
      to split guest RAM / VRAM / BIOS Data Area / NVRAM
      for save-state isolation.
- [ ] **GC + Component Model**: not needed for the
      emulator core; revisit for JS-side tooling only.

### 3.2 Feature-detected multi-bundle shipping
- [ ] Ship 4 wasm bundles — baseline, `threads`,
      `threads+simd`, `threads+simd+exceptions`
      (+ `memory64` variants) — and select at runtime with
      `wasm-feature-detect` (the Emscripten pattern).
      Fall back gracefully on Safari (no SIMD/relaxed-SIMD
      in some versions) and legacy browsers.

### 3.3 Web APIs
- [ ] **File System Access API**: drag-and-drop and
      persistent file handles for disk images; write-back
      to the user's local files; save-state snapshots
      stored via `showSaveFilePicker`.
- [ ] **WebUSB**: optional USB device passthrough (user
      gesture + permission) for USB mass-storage and HID
      testing of the P2 USB stack.
- [ ] **WebSerial**: serial-console passthrough for the
      16550A backend (headless/debug builds).
- [ ] **WebAudio**: SB16/speaker output with
      `AudioWorklet` (replaces legacy ScriptProcessor).
- [ ] **Service worker**: offline cache of `v86.wasm`,
      firmware blobs, and boot images (progressive web
      app behavior).
- [ ] **WebGPU** (watchlist): future VGA compositing/
      scaling in the renderer; not blocking.

### 3.4 Server/standalone (WASI 0.3, Feb 2026)
- [ ] Build a **WASI 0.3** variant (native async, futures/
      streams) for Node/Deno/CLI/server-side use —
      headless batch testing of the firmware and CI boot
      tests without a browser.

## 4. Architecture upgrades
- [ ] PCjs `Component`/`initBus()` → Rust device traits;
      preserve the notification pattern (bus created first,
      devices register, `computer`-equivalent supervisor).
- [ ] Firmware capability table: a static "BIOS features"
      structure the install checks (INT 13h 41h, VBE 4F00h)
      report, so guests can query what the firmware
      supports (EDD functions bitmask, VBE version,
      El Torito drive map).
- [ ] Deterministic save/restore: full CMOS/NVRAM, video
      state (palettes, CRTC, planes), disk controller
      state, and thread-shared memory snapshot; round-trip
      test with threads enabled.
- [ ] Firmware configuration (`firmware: "pcjs" | "rom" |
      "none"`, machine profile: "pc" | "xt" | "at" |
      "compaq386", memory size, boot order, El Torito
      image slot).

## 5. Test matrix additions
- [ ] El Torito ISO boot: bootable DOS ISO, Linux ISO
      (no-emulation), CD-ROM data access under MSCDEX.
- [ ] LBA: GRUB/Limine bootloader reads via AH=42h/43h;
      127-sector DAP batching; >8 GB disk image.
- [ ] VBE: VBEtest/SVGATest mode sets, LFB mapping,
      DAC palette accuracy.
- [ ] CMOS/RTC: DOS `DATE`/`TIME`, alarm IRQ, NVRAM
      persistence across reloads.
- [ ] PS/2 mouse: Mouse drivers in DOS (mouse.com),
      3-wheel Explorer.
- [ ] PCI: guest enumerates the emulated NE2000/IDE
      devices via B101h/B102h.
- [ ] Threads/SIMD bundles: identical guest behavior
      across all four bundles (differential testing).
- [ ] QEMU as behavioral oracle for EDD/VBE/PCI (its test
      code is GPL — oracle only, never incorporated).

## 6. Licensing notes
- New firmware features are implemented from the public
  specs (EDD, El Torito/ISO9660, VESA VBE 3.0, PCI BIOS,
  ACPI 1.0+, USB 1.1/2.0, UEFI) → **new MIT work**;
  PCjs MIT notice + README attribution ("PCjs © 2012-2026
  Jeff Parsons", displayed in the host UI) retained.
- **Never** copy SeaBIOS/VGABIOS (GPL/LGPL) code, tables,
  or structure; QEMU is a behavioral oracle only.
- Vendor any third-party code only under MIT/BSD/Apache
  (e.g., `wasm-feature-detect` is MIT) with notices in
  `THIRD-PARTY-NOTICES.md`.

## 7. Roadmap
1. **Phase 1 (P0)** — EDD LBA + El Torito + VBE + CMOS/RTC
   + E820: ~2–3 months. Outcome: ISO and large-disk boot
   with no copyleft firmware; SeaBIOS becomes optional.
2. **Phase 2 (P1)** — PCI BIOS + APIC + ACPI + ATAPI
   hardening + PS/2 mouse: ~2 months. Outcome: modern
   Linux guests.
3. **Phase 3 (P2)** — USB + OVMF/UEFI integration: ~2
   months. Outcome: USB boot, UEFI guests.
4. **Phase 4 (web platform)** — SIMD/threads/exceptions
   bundles, OffscreenCanvas pipeline, File System Access,
   WASI 0.3 build: ~1–2 months, parallel with Phases 2–3.
