# Third-Party Notices

License audit for this distribution. **Revised 2026-10-09:** the
clean-room re-implementation plan was withdrawn; v86 (BSD-2-Clause)
is the base and is used as-is; the firmware was replaced with
permissive alternatives. The distribution carries **no copyleft**.

## License summary

| Component | Location | License | Notes |
|---|---|---|---|
| Emulator core, devices, browser glue | `src/`, `src/browser/` | **BSD-2-Clause** (`LICENSE`) | v86 base, used as-is; retain notice |
| Rust→WASM CPU/JIT port | `src/rust/`, `gen/` | **BSD-2-Clause** (derived from `src/*.js`) | v86 base, used as-is; retain notice |
| Floppy controller (Intel 82078) | `src/floppy.js` | **MIT** (`LICENSE.MIT`, QEMU port, Jocelyn Mayer / Hervé Poussineau) | Retain notice |
| **PCjs PCx86 firmware (BIOS services)** | `github.com/jeffpar/pcjs` | **MIT** (Jeff Parsons, © 2012–2026) | **Adopted firmware** (todo §2.2). Vendor with attribution: retain MIT notice **and** display "PCjs © 2012-2026 Jeff Parsons" (with pcjs.org and GitHub links) in the host UI, per the PCjs README attribution requirement |
| **b-dmitry1/BIOS** (8 KB x86 BIOS, optional ROM path) | `github.com/b-dmitry1/BIOS` | **MIT** | ROM-image firmware alternative (todo §2.4); retain notice |
| SoftFloat IEEE-754 package | `lib/softfloat/softfloat.c` | **BSD-3-Clause** (UC Regents / John R. Hauser, SoftFloat 3e) | Retain notice |
| zstd decompressor | `lib/zstd/zstddeclib.c` | **Dual: BSD-2-Clause (with patent grant) OR GPLv2** (Yann Collet, Facebook, Inc.) | Used under the **BSD option**; retain notice and upstream patent grant |
| VBE 2.0+ firmware (new) | this project | **MIT** (new work) | Written from the public VESA VBE specification |
| OVMF/EDK2 (optional UEFI, Phase 2) | `github.com/tianocore/edk2` | **BSD-2-Clause-Patent** | Optional separately-fetched UEFI firmware |
| SeaBIOS | `bios/seabios*.bin` | **GPL-2.0-or-later** | **Not bundled.** Optional, separately fetched asset for compatibility only |
| Bochs VGA BIOS | `bios/vgabios*.bin` | **LGPL** (`bios/COPYING.LESSER`) | **Not bundled.** Optional, separately fetched asset for compatibility only |
| QEMU x86 CPU test | `tests/qemu/` | **GPL-2.0** (Fabrice Bellard) | Test oracle only — never incorporated |

## Attribution obligations for the host UI

1. v86: retain `LICENSE` (BSD-2-Clause, "The v86 contributors").
2. PCjs: display "PCjs © 2012-2026 Jeff Parsons" with links to
   <https://www.pcjs.org> and <https://github.com/jeffpar/pcjs>
   on every page/host that runs the firmware (PCjs README
   requirement; standard MIT notice retention is also satisfied).
3. b-dmitry1/BIOS: retain its MIT notice when the ROM path is used.

## License texts

- `LICENSE` — BSD-2-Clause (v86 contributors)
- `LICENSE.MIT` — MIT (QEMU floppy emulator portions)
- `bios/COPYING.LESSER` — LGPL (Bochs VGA BIOS)
- PCjs — standard MIT License, Copyright (c) 2012-2026 Jeff Parsons
  (see `github.com/jeffpar/pcjs/blob/master/LICENSE.txt`)
- SoftFloat 3e BSD-3-Clause and zstd (BSD option + patent grant):
  retain upstream texts as shipped in `lib/`.

## Copyleft boundary

- No GPL/LGPL code is bundled in the permissive distribution.
- SeaBIOS and VGABIOS remain available as **optional, separately
  fetched** assets under their original licenses, with build
  scripts and source attribution intact (`bios/fetch-and-build-seabios.sh`,
  `bios/README.md`).
- The emulator must support **fw_cfg direct kernel boot** (Linux
  boot protocol, `-kernel`/`-initrd`), which requires no firmware.
