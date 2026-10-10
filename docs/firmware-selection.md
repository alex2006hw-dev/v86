# Firmware selection for v86 (2026 review)

Question: which ROM / BIOS / VGA BIOS should v86 ship, and what can
legally be copied from elsewhere?

Short answer: **no third-party firmware is both permissively licensed and
feature-complete.** The only things that can be reused are PCjs's *code*
(MIT) and b-dmitry1/BIOS (MIT, but minimal). The ROM images everyone
points at are either copyleft or unlicensed, so the modern feature set
has to be built — which is what `src/rust/firmware` now does.

## Options considered

| Option | Licence | Feature coverage for v86 | Verdict |
|---|---|---|---|
| **PCjs ROM images** (IBM 5150/5160/5170, COMPAQ, Olivetti M24) | **Not MIT.** PCjs's own `LICENSE.txt` limits the MIT grant to "the PCjs repository and website, **excluding all programs, images and documentation produced by other parties**". These are IBM ROM dumps. | Real but 1981–1984 BIOSes: no EDD/LBA, no El Torito, no VBE, no PCI/ACPI | **Unusable** — the licence excludes them, and they are too old to close the P0 gaps |
| **PCjs source** (`interrupts.js`, `video.js`, `chipset.js`, `fdc.js`, …) | **MIT** + attribution clause | The service model, VGA/VBE device handling, disk services | **Usable — this is what to copy.** Not a single line of ROM binary is involved |
| **fysnet/i440fx** | **No `LICENSE` file** (GitHub reports `null`), and it ships a prebuilt `i440fx.bin` in-tree | Technically the best fit by far: ACPI, APIC, HPET, PCI PnP, USB (UHCI/OHCI/EHCI/xHCI boot), SATA, El Torito, CMOS; boots FreeDOS through Windows 11 | **Unusable** — all rights reserved by default. Also needs USB + SATA, which v86 does not emulate at all |
| **b-dmitry1/BIOS** | **MIT** (2025, actively maintained) | Deliberately minimal: INT 10h–1Ah, a tiny SVGA BIOS, "Int 13h supports only reset/read/write". No EDD/LBA, no El Torito, no PCI, no VBE, no ACPI | **Usable but insufficient.** A legitimate *optional* minimal ROM for tiny guests, not the modern path |
| **640-KB/GLaBIOS** | **GPL-3.0** | Modern, scratch-built | Excluded — copyleft |
| **SeaBIOS / SeaVGABIOS** | **GPL-2.0-or-later / LGPL-3.0** | Full legacy + EDD, VBE 2.0 | Excluded from the permissive build; already correctly documented as separately-fetched assets in `bios/README.md` |
| **Bochs VGABIOS** | **LGPL-3.0** | VBE 2.0, mode-X compatible | Excluded, same as above |
| **Coreboot** | GPL-2.0 | — | Excluded |
| **OVMF / EDK II** | **BSD-2-Clause-Patent** | UEFI: ACPI, modern boot, no legacy INT 10h/13h | **Usable** for the UEFI path (todo §2.7), but it does not replace a legacy BIOS |

## What this means for the plan

`todo.md` §2.2 reads "Vendor PCjs components (MIT) — port `interrupts.js`,
`chipset.js`, `video.js`, …". That is still right, with one correction
that the analysis above forces:

* ~~PCjs is MIT, therefore PCjs's BIOS is available.~~ **False.** PCjs
  is MIT; its *ROM images* are third-party dumps that the MIT grant
  explicitly excludes. Vendoring PCjs's ROMs would reintroduce exactly
  the copyleft §1 is trying to remove.
* The reusable part is the **method**: the component/`initBus()` service
  model, and the fact that a BIOS service layer can live on the host and
  be reached from real ROM stubs.

That is the architecture `src/rust/firmware` now implements:

* a real 64 KiB system BIOS at `F0000` and a real 32 KiB video option ROM
  at `C0000`, assembled from source by `rom.rs` + `asm.rs`, so the guest
  sees a reset vector, hardware-interrupt stubs and vectored INT handlers
  it can inspect and replace;
* the leaf work (INT 10h/13h/15h/16h/1Ah, VBE 4Fxx, IRQ 0/1/6/8/12) on
  the host, reached through a trap that is only honoured for code
  executing inside the firmware's own ROM windows.

So: **no ROM or VGABIOS is copied.** Everything vendored is either
original MIT work here, or (when the port proceeds) PCjs's MIT *code*.
The PCjs attribution notice is still required in the UI, because the
service model and register layouts are taken from it.

## Consequences for the roadmap

1. The P0 list (todo §7.1) has to be implemented, not vendored: EDD/LBA,
   El Torito, VBE 2.0/3.0, CMOS/RTC, E820, A20. Much of this now exists.
2. The P2 USB/UHCI boot work (§7.3) cannot use i440fx, and v86 has no
   USB controller, so USB boot is blocked on emulator-side device work
   regardless of firmware licensing.
3. OVMF (BSD-2-Clause-Patent) stays the realistic UEFI answer, and needs
   no firmware work from us beyond the ACPI tables v86 already has.

## Licences that must not be broken

* `bios/*.bin` are tracked in git and are GPL/LGPL. They stay optional,
  separately fetched assets; the permissive default is
  `firmware: "pcjs"`.
* Never translate, link or extract SeaBIOS/VGABIOS into the permissive
  tree (todo §7 "never copy SeaBIOS/VGABIOS").
* QEMU remains a behavioural oracle only — its test code is GPL.
