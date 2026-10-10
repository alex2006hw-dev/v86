# BIOS Engineering Report — License-Free Firmware Alternatives

**Role:** BIOS/firmware engineer · **Date:** 2026-10-09
**Goal:** replace SeaBIOS (GPL-2.0+) and Bochs VGABIOS (LGPL) so the
clean-room MIT project can ship with permissively-licensed or
specification-derived firmware.

## 1. Executive summary

**No drop-in permissive replacement exists for the full SeaBIOS feature
set, and no permissive standalone VGA BIOS exists at all** — the entire
x86 firmware ecosystem (SeaBIOS, OpenBIOS, coreboot, SeaVGABIOS, Bochs
VGABIOS, Cirrus VGABIOS) is copyleft. However, there are viable
permissive paths, and the clean-room project does not actually need a
full SeaBIOS clone:

| Tier | Path | License | Coverage |
|---|---|---|---|
| 0 | **fw_cfg direct kernel boot** (no firmware at all) | n/a | Linux, FreeBSD, other kernel-protocol guests |
| 1 | **b-dmitry1/BIOS** (8 KB, NASM) — vendor or extend | **MIT** | DOS 3.3–6.22, Windows 1.0–3.1/95, Minix, Linux ≤1.3.89 (CHS disks) |
| 2 | **From-scratch minimal VGA/VBE BIOS** from the VESA VBE spec (public standard) | MIT (ours) | Text modes, mode 13h, VBE mode setting |
| 3 | **TianoCore EDK2 / OVMF** (or Microsoft Mu) | **BSD-2-Clause-Patent** | Full UEFI: modern Linux, Windows 8+, any UEFI-capable guest |
| Fallback | SeaBIOS + VGABIOS as **separately-fetched** GPL/LGPL assets | GPL/LGPL | Maximum compatibility (today's v86 behavior) |

Recommendation: implement **Tier 0 + Tier 1 + Tier 2** in the MIT tree,
offer **Tier 3** as an optional permissive firmware download, and keep
the **fallback** as a clearly-marked separate fetch. This preserves every
v86 guest class without any copyleft code in the MIT distribution.

## 2. Candidate survey

### Permissive — legacy BIOS

- **[b-dmitry1/BIOS](https://github.com/b-dmitry1/BIOS)** — **MIT**,
  <8 KB ROM, NASM assembly, built for FPGA/emulators/386EX. Tested on
  real 8086–486 CPUs and with MS-DOS 3.3/4.0/6.22, Windows
  1.0/2.0/3.1/95, Linux 0.01/1.3.89, Minix 1.x/2.x. Implements INT
  10h–1Ah, minimal SVGA (enough for Heroes of Might & Magic, Transport
  Tycoon), INT 13h reset/read/write, add-on ROM support, A20 control,
  and a **"BIOS disk hypercall for emulators"** (ideal hook for a
  browser emulator's block backend). QEMU-compatible (pad to 64 KB).
  Caveats: no LBA, no CD-ROM/El Torito, no hardware detection/setup,
  no extended memory test, no text printing in graphics modes, video
  adapter init incomplete (the emulator must own the actual VGA
  hardware — which our emulator does).
- **[sergev/tiltti](https://github.com/sergev/tiltti)** — **MIT** i86
  PC-DOS emulator with its own BIOS layer (INT 10h video services etc.).
  Useful as a behavioral reference for BIOS service semantics.
- **[PCjs (jeffpar/pcjs)](https://github.com/jeffpar/pcjs)** — **MIT**
  browser IBM-PC emulator. Architectural reference for the
  **emulator-internal BIOS model**: PCjs implements BIOS services in the
  host emulator itself rather than as a ROM blob. This is the cleanest
  long-term design for a WASM emulator (no ROM images at all).
- **[b-dmitry1/e86r](https://github.com/b-dmitry1/e86r)** — same
  author's 80486 IBM-PC emulator (Windows/STM32); companion project to
  the BIOS above.

### Permissive — UEFI (replaces both SeaBIOS and VGABIOS for UEFI guests)

- **[TianoCore EDK2 / OVMF](https://github.com/tianocore/edk2)** —
  **BSD-2-Clause-Patent**. Full UEFI implementation; OVMF is the
  QEMU-oriented build. Heavy (megabytes), needs a UEFI-capable guest and
  a machine model with ACPI/PCI/runtime services — which our emulator
  already provides. There is an IA-32 build for 32-bit guests.
- **[Microsoft Project Mu](https://github.com/microsoft/mu_plus)** —
  **BSD-2-Clause-Patent**. Modular UEFI core; more modern codebase,
  ARM/x64 focused, includes Rust UEFI support.
- **EDK2 DuetPkg** — BSD-2-Patent, but it is a UEFI environment that
  runs *on top of* a legacy BIOS (a payload/bootloader, not a BIOS
  replacement; does not boot DOS). **Not suitable.**

### Permissive — bootloaders (not BIOS, but an alternative boot path)

- **[Limine](https://github.com/limine-bootloader/limine)** — BSD-2-Clause,
  multiprotocol (BIOS/UEFI/PXE) bootloader + boot protocol.
- **[rEFInd](https://sourceforge.net/projects/refind/)** — BSD-3-Clause
  UEFI boot manager (requires UEFI firmware first).

### Excluded (copyleft or not open)

- SeaBIOS (GPL-2.0+), OpenBIOS (GPL), coreboot/libreboot (GPL),
  SeaVGABIOS (GPL), Bochs VGABIOS (LGPL), Cirrus VGABIOS (LGPL),
  86Box (GPL-2+), DOSBox (GPL), js-dos (DOS-specific model),
  **JSLinux (not open source — Fabrice Bellard's IP)**, SLOF (BSD-3
  but PowerPC-only), retrotick (CC0 but app-level API reimplementation).

## 3. VGA BIOS gap analysis

Every standalone VGA BIOS in the wild is copyleft (Bochs VGABIOS LGPL,
SeaVGABIOS GPL, Cirrus LGPL). Three license-free strategies:

1. **Write a minimal VGA/VBE BIOS from the VESA BIOS Extensions
   specification** (public standard, v3 current). Scope: INT 10h text
   services, mode setting (incl. mode 13h), palette, VBE 2.0+ mode
   information and set/get mode via INT 10h AX=4Fxx. The *interface*
   is functional and uncopyrightable (*Sony v. Connectix*); the
   implementation is ours.
2. **Emulator-internal VBE services** (PCjs/DOSBox model): implement
   VBE INT 10h calls in the emulator host, ship no VGA ROM at all.
   Best fit for WASM: no ROM loading, no 64 KB padding, direct access
   to the emulated framebuffer.
3. **b-dmitry1/BIOS's minimal Video BIOS** (MIT) as a vendored
   component for text + mode 13h, extended with our own VBE code.

For guests that hard-require the real VGABIOS (some DOS games with
mode-X timing quirks), keep the LGDB VGABIOS as a separate fetch.

## 4. Guest compatibility matrix

| Guest | Tier 0 (fw_cfg) | Tier 1 (MIT 8 KB BIOS) | Tier 2 (our VGA BIOS) | Tier 3 (OVMF/UEFI) | Fallback (SeaBIOS+VGABIOS) |
|---|---|---|---|---|---|
| Linux (modern) | ✅ primary | — | — | ✅ | ✅ |
| FreeBSD | ✅ | — | — | ✅ | ✅ |
| MS-DOS 3.3–6.22 | — | ✅ | ✅ (video) | — | ✅ |
| Windows 3.1/95 | — | ✅ | ✅ | — | ✅ |
| Windows NT 4/2000 | partial | partial | partial | ✅ | ✅ |
| DOS games (mode X) | — | ✅ | ⚠ timing quirks | — | ✅ |
| Boot from ISO (El Torito) | ✅ (direct kernel) | ❌ no CD-ROM | ❌ | ✅ | ✅ |

## 5. Engineering notes

- b-dmitry1/BIOS builds with NASM in minutes; pad to 64 KB for
  QEMU-style loaders; the "BIOS disk hypercall" maps cleanly onto a
  WASM import that reads from the v86 block-device backend.
- The emulator-internal BIOS model (PCjs) removes the ROM-loading API
  surface entirely; if adopted, `V86Options.bios`/`vga_bios` become
  optional compatibility knobs.
- OVMF IA-32 build requires the emulator to implement UEFI runtime
  services, ACPI tables (our `acpi.js` already drafts some), and a
  PCI root bus (we have `pci.js`) — feasible but a Phase-2 effort.
- CMOS/RTC defaults (`fill_cmos` semantics, cf. tiny386) matter for
  Win9x compatibility; our BIOS must present a sane memory map and
  equipment word.

## 6. Legal notes

- MIT and BSD-2-Patent code may be vendored in the MIT tree with
  attribution (clean-room policy §9 permits permissive vendoring).
- If pure clean-room provenance is desired for the firmware itself,
  use b-dmitry1/BIOS only as a **behavioral oracle** (run it, observe
  INT semantics) and write our own implementation from the IBM PC
  BIOS interface + VESA VBE specs; the interface is functional and
  uncopyrightable (*Sony v. Connectix*, *Sega v. Accolade*).
- Never import SeaBIOS/VGABIOS binaries or translations into the MIT
  tree; separate-fetch only (policy §8).

## 7. Decision — adopt PCjs (MIT) as the BIOS approach

**Chosen path (revised 2026-10-09):** the PCjs model — BIOS
services implemented in the emulator host, no ROM blobs — is the
primary firmware, **vendored directly from PCjs under its MIT
license** (the clean-room process was withdrawn; see `todo.md`
§1.3 — with v86 as the BSD-2 base, reimplementation-from-scratch
is unnecessary and MIT vendoring is explicitly permitted by
PCjs's license):

- **Why PCjs:** MIT-licensed; its architecture (BIOS as
  emulator-internal JS components, `Component`/`initBus()`
  pattern) maps directly onto a Rust/WASM device model and
  removes ROM loading, 64 KB padding, and VGA-ROM quirks from
  the emulator API; it is a proven browser-IBM-PC implementation
  (DOS, Windows 3.x, OS/2 on PC/XT/AT and Compaq DeskPro 386).
- **Obligations:** retain the MIT notice and the README
  attribution ("PCjs © 2012-2026 Jeff Parsons" with pcjs.org
  links) in THIRD-PARTY-NOTICES and display it in the host UI.
- **Gaps to close:** PCjs targets PC/XT/AT-era guests — no
  LBA, no CD-ROM/El Torito, no UEFI; fill with the from-spec
  VBE 2.0+ implementation (§8.0d in todo), the b-dmitry1/BIOS
  ROM path (MIT), and fw_cfg direct kernel boot for OS guests.
- **Boot paths after integration:** fw_cfg direct kernel boot
  (Tier 0, Linux/BSD), PCjs BIOS services (Tier 1, DOS-era),
  ROM images as compatibility fallback (b-dmitry1/BIOS MIT, or
  SeaBIOS/VGABIOS separate-fetch), UEFI via OVMF (Phase 2).

## 8. Action items (added to todo.md)

- [ ] Decide emulator-internal BIOS vs. ROM-image model (architectural).
- [ ] Vendor or reimplement the 8 KB MIT BIOS; add disk hypercall →
      WASM block backend.
- [ ] Implement VBE 2.0+ subset from the VESA spec (INT 10h AX=4Fxx).
- [ ] Build OVMF IA-32 and integrate UEFI boot (Phase 2).
- [ ] Keep SeaBIOS/VGABIOS separate-fetch as compatibility fallback.
