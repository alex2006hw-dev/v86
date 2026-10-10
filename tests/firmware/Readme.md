# v86 PCjs firmware tests

Boots the PCjs-derived BIOS firmware inside the **real PCjs x86 CPU** under
Node.js and runs a hand-assembled BIOS self-test boot sector against it.

```
make firmware-test          # build the firmware wasm + run the suite
make firmware-unit-test     # host-side unit tests (no wasm toolchain needed)
```

Set `FW_TRACE_INT=1` to dump register state after every BIOS interrupt.

## What runs where

| Piece | What it is |
|---|---|
| CPU | `CPUx86` from the PCjs tree (MIT) — the only x86 core involved |
| RAM | PCjs `Memoryx86` blocks covering the physical address space |
| BIOS | `src/rust/firmware` compiled to `wasm32-wasip1` |
| Glue | `run.mjs`, via the firmware's imported `env::v86_*` callbacks |
| Guest | `boot-sector.mjs`, a hand-assembled 512-byte boot sector |

No copyleft is involved anywhere: PCjs is MIT and the firmware is new MIT work.

## How interrupts reach the firmware

The harness does not emulate a ROM. Instead it hooks PCjs's `addIntNotify()`,
which fires on the `INT n` opcode:

* returning `true` lets PCjs perform the real INT prologue — push FLAGS/CS/IP,
  load `CS:IP` from the IVT. POST points those vectors at `F000:0000`, where the
  harness places a single `IRET`, so every serviced interrupt returns cleanly to
  its caller;
* returning `false` suppresses the ROM trap entirely. INT 19h needs this: the
  bootstrap never returns, having already loaded a boot sector and set `CS:IP`.

This mirrors the hook the firmware uses inside the main v86 crate
(`call_interrupt_vector` in `src/rust/cpu/cpu.rs`).

The run therefore follows a real PC's cold boot path: reset vector
`F000:FFF0` → `int 19h` → firmware loads LBA 0 to `0000:7C00` → boot sector runs.

## What the boot sector checks

Twelve checks; a failure prints a single character naming the check, so the
screen says exactly which service misbehaved.

| ID | Check |
|---|---|
| M | INT 12h conventional memory ≥ 512 KiB |
| E | INT 11h equipment word reports a floppy |
| C | INT 10h AH=0Fh reports 80 columns |
| R | INT 13h AH=00h reset, CF clear |
| D | INT 13h AH=41h EDD install check → BX=AA55h |
| G | INT 13h AH=08h geometry: CH/CL/DH/DL |
| B | INT 13h AH=42h EDD read of LBA 0 via a DAP |
| S | the sector read back is this boot sector (0xAA55) |
| 8 | INT 15h AX=E820h succeeds |
| 0 | first SMAP entry describes memory at base 0 |
| Q/A/X | INT 15h AX=2402h/2401h/2400h A20 query/enable/disable |
| T | INT 1Ah AH=02h RTC date century byte non-zero |

The verdict line reads `RESULT: PASS` or `RESULT: FAIL`.

## Files

* `boot-sector.mjs` — the assembler and the guest program.
* `run.mjs` — the harness and the assertions.
* `trace.mjs` — instruction-level tracer, for debugging the firmware.
  `TRACE_N=<n>` bounds the instruction count, `STEP_CYCLES=0` single-steps.

## Notes for anyone extending this

The PCjs CPU modules have two import-order traps that a standalone harness must
avoid (the PCjs machine definition hides both):

1. `x86ops.js` builds `X86.aOps` / `X86.aOpGrp*` out of the `X86.fn*`
   primitives **at module evaluation time**, so `x86func.js` and `x86help.js`
   must be imported first or the tables capture `undefined`.
2. `initProcessor()` only installs the ModR/M decode function pointers
   (`updateAddrSize()` / `updateDataSize()`) when the operand sizes disagree,
   which never happens on a freshly reset 16-bit CPU. Call them explicitly.

Also note PCjs models condition codes as `setXXX()` / `clearXXX()` pairs that
take **no argument** — there is no `setCF(bool)` — and `getXXX()` recomputes a
pending arithmetic result, so the harness routes flag writes through the pair.