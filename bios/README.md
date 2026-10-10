# BIOS firmware images — licensing

This directory contains **third-party firmware binaries**. They are **not**
part of the emulator's source code and are **not** MIT-licensed.

| File | Origin | License |
|---|---|---|
| `seabios.bin`, `seabios-debug.bin` (+ `.config`) | [SeaBIOS](https://www.seabios.org/) | **GPL-2.0-or-later** |
| `vgabios.bin`, `vgabios-debug.bin`, `bochs-vgabios.bin`, `bochs-bios.bin` | Bochs | **LGPL-3.0** (`COPYING.LESSER`) |

`fetch-and-build-seabios.sh` downloads and builds SeaBIOS from its official
source tree.

## Policy for the clean-room MIT project (todo §1.5, decision: separate fetch)

- The MIT-licensed distribution **ships no firmware**.
- These images are **optional, separately fetched assets**, distributed only
  under their original GPL/LGPL terms, with their build scripts and source
  attribution intact.
- The emulator must also support **direct kernel boot via fw_cfg** (Linux
  boot protocol with `-kernel`/`-initrd`), which requires no BIOS image at
  all.
- Never import, link, or translate these binaries or their build outputs
  into the MIT-licensed source tree.
