# v86 Permissive-Firmware Replacement — Project Plan

**Revised 2026-10-09** — scope narrowed: v86 (BSD-2-Clause) is
the base and is used as-is; the project focus is **replacing
the copyleft BIOS firmware** (SeaBIOS GPL-2.0+, Bochs VGABIOS
LGPL) with permissively-licensed firmware so the distribution
carries no copyleft. The earlier clean-room re-implementation
plan and process artifacts were withdrawn (see §1.4).

## 0. Goal & scope

- Replace/retire GPL/LGPL firmware from the shipped product.
- Keep v86's BSD-2-Clause code (retain `LICENSE` + notices).
- Adopt PCjs-style emulator-internal BIOS services (MIT) as the
  primary firmware, with permissive alternatives for other boot
  paths.
- Out of scope: emulator re-implementation, CPU/device work
  beyond what the new firmware needs.

## 1. Licensing (revised — clean-room withdrawn)

- [x] 1.1 License audit of every component → `THIRD-PARTY-NOTICES.md`
- [ ] 1.2 Release model: **BSD-2-Clause for v86-derived files;
      MIT for new and PCjs-derived firmware files**; retain all
      copyright notices; display the PCjs attribution notice
      ("PCjs © 2012-2026 Jeff Parsons", per its README) in the
      host UI (About panel/footer).
- [x] 1.3 Clean-room artifacts withdrawn: `docs/clean-room-policy.md`,
      `docs/provenance-log.md`, `CLA.md`, and the
      `NOTICE-CLEANROOM.md` quarantine notices in `src/rust/` and
      `gen/` removed — the clean-room process was only required
      to create a *new* MIT-licensed emulator from scratch; with
      v86 as the BSD-2 base it is unnecessary.
- [ ] 1.4 Counsel review (simplified, was §1.6): confirm the
      PCjs README attribution clause treatment and the final
      BSD-2/MIT license mix before public release.
- [ ] 1.5 Note for release notes: v86-derived code remains
      BSD-2-Clause (it cannot be re-licensed MIT-only without
      all contributors' consent); all *new* firmware work is MIT.
      The distribution as a whole is permissive (no copyleft).

## 2. Firmware selection & integration

- [x] 2.1 PCjs analysis complete → `docs/pcjs-bios-analysis.md`
      (architecture, BIOS service inventory, gaps, porting effort)
- [ ] 2.2 **Vendor PCjs components (MIT)** — primary path:
      port `interrupts.js`, `chipset.js`, `video.js`, `disk.js`,
      `fdc.js`, `keyboard.js`, `computer.js` (component model,
      `initBus()` pattern) to the Rust/WASM device model; wire
      disk → WASM block backend, video → canvas, keyboard →
      PS/2 + WASM input. (~2–4 engineer-months; video.js is the
      largest piece.)
- [ ] 2.3 Implement VBE 2.0+ subset from the public VESA VBE
      spec (INT 10h AX=4Fxx mode setting, mode 13h, text
      services) — closes the "no permissive VGA BIOS" gap.
- [ ] 2.4 Vendor b-dmitry1/BIOS (**MIT**, 8 KB NASM) as the
      ROM-image alternative; wire its "disk hypercall" to the
      WASM block backend; pad to 64 KB for ROM-style loaders.
- [ ] 2.5 Implement remaining PC/XT-era services from datasheets:
      CMOS/RTC (MC146818), INT 15h memory map (0x413, E820),
      INT 16h keyboard buffer (0x41E–0x43D), equipment word
      (0x410), A20 control, POST.
- [ ] 2.6 **fw_cfg direct kernel boot** — primary path for
      Linux/FreeBSD (no firmware at all).
- [ ] 2.7 (Phase 2) OVMF/EDK2 (**BSD-2-Clause-Patent**) for
      UEFI guests.
- [ ] 2.8 SeaBIOS + VGABIOS remain **optional, separately
      fetched** GPL/LGPL assets for maximum compatibility
      (never bundled in the permissive distribution).

## 3. Emulator gaps the new firmware needs (v86-side work)

- [ ] 3.1 Block-device backend hook (BIOS disk I/O → existing
      IDE/floppy/virtio backends).
- [ ] 3.2 Framebuffer/canvas binding for the video BIOS layer.
- [ ] 3.3 PS/2 keyboard/mouse event injection for INT 16h.
- [ ] 3.4 CMOS/RTC backing store (NVRAM persistence).
- [ ] 3.5 `V86Options.bios`/`vga_bios` become optional knobs;
      add `firmware: "pcjs" | "rom" | "none"` selector.

## 4. Testing & verification

- [ ] 4.1 Boot suite via new firmware: MS-DOS 3.3/4.0/6.22,
      Windows 1.0/2.0/3.1/95, Minix 1.x/2.x.
- [ ] 4.2 Linux/FreeBSD via fw_cfg direct boot.
- [ ] 4.3 DOS game suite (mode-X / palette timing edge cases).
- [ ] 4.4 Fallback: SeaBIOS + VGABIOS separate-fetch still boots.
- [ ] 4.5 QEMU as behavioral oracle only (its test code is GPL —
      never incorporated).

## 5. Release

- [ ] 5.1 `THIRD-PARTY-NOTICES.md` final; PCjs attribution in UI.
- [ ] 5.2 Docs: firmware architecture, boot-path matrix, migration
      guide (v86 → permissive firmware).
- [ ] 5.3 npm + GitHub releases (wasm); tag `v0.2.0`
      (firmware-only change from v86 0.5.x lineage).
- [ ] 5.4 Optional: publish the permissive firmware as a
      standalone package (`firmware-pcjs.wasm`).

## 7. Modern firmware upgrades (2026)

Full plan: `docs/pcjs-modern-upgrade.md`.

- [ ] 7.0 Baseline audit of PCjs's existing ATAPI CD-ROM
      (added 2019, incomplete for Win95-era), mouse, and
      VGA support — harden before extending.
- [ ] 7.1 **P0 boot-path completeness** (removes the
      SeaBIOS dependency): INT 13h Extensions/EDD (LBA,
      Disk Address Packet, AH=41h–48h), El Torito ISO
      boot (no-emulation, boot catalog, CD as INT 13h
      drive 0x00–0x0F), VBE 2.0+/3.0 (AX=4Fxx, linear
      framebuffer), CMOS/RTC MC146818 + NVRAM
      persistence, E820/E801 memory map, A20 gate.
- [ ] 7.2 **P1 modern guests**: PCI BIOS (INT 1Ah B1xx),
      APIC (IOAPIC + LAPIC), ACPI tables (RSDP/RSDT/
      FACP/DSDT), ATA/ATAPI hardening (LBA48), PS/2
      mouse (aux-port commands, 3-wheel Explorer).
- [ ] 7.3 **P2 peripheral boot**: USB UHCI + mass-storage
      boot (BOT protocol), OVMF/UEFI image.
- [ ] 7.4 **Web platform 2026**: Wasm SIMD/Relaxed SIMD
      (VGA blit, scalers, softfloat, ISO9660 CRC),
      threads + SharedArrayBuffer + OffscreenCanvas
      workers (COOP/COEP), Wasm exception handling
      (exnref) with JS-exception fallback bundles,
      Memory64 (>4 GB guest RAM, 16 GB browser cap) +
      multiple memories, feature-detected multi-bundle
      shipping via `wasm-feature-detect`.
- [ ] 7.5 **Web APIs**: File System Access (persistent
      image handles, save-state snapshots), WebUSB
      passthrough, WebSerial console, WebAudio
      AudioWorklet, service-worker offline cache,
      WASI 0.3 (Feb 2026) server/CLI build for headless
      CI boot tests.
- [ ] 7.6 Test matrix: El Torito ISO boot, LBA (GRUB/
      Limine via AH=42h/43h), VBE (VBEtest), CMOS/RTC
      persistence, PS/2 mouse, PCI enumeration,
      cross-bundle differential testing (all wasm
      bundles), QEMU as behavioral oracle.

## 6. Order & rough sizing

1. §1 licensing sign-off (counsel) — 1–2 weeks, gating release.
2. §2.2 PCjs port — 2–4 months (start with interrupts/chipset/
   keyboard/disk; video last).
3. §7.1 P0 firmware upgrades — 2–3 months, parallel with §2.2
   (closes the LBA/El Torito/VBE gaps that §2.x left open).
4. §2.3–2.6 VBE, services, fw_cfg — 1 month, parallel.
5. §3 emulator glue — 2–4 weeks, parallel with §2.2.
6. §7.2–7.5 P1/P2 + web platform — 2–3 months, parallel.
7. §4 + §7.6 test suite — continuous; gates §5.

**Risks:** PCjs video.js complexity (CRT timing, EGA/VGA
planes); PCjs README attribution clause interpretation
(counsel); DOS-game mode-X timing quirks (mitigated by
§4.3 + VGABIOS fallback); SIMD/threads bundle behavior
parity (mitigated by §7.6 cross-bundle differential
testing); SharedArrayBuffer COOP/COEP header requirements
for hosting.
