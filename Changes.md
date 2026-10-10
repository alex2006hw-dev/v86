# Changes

Session log for the permissive-firmware work: what changed, why, and how
each claim was checked. Companion documents: `README.md` (what the
firmware is and how to use it), `TechDebt.md` (what it deliberately does
not do), `todo.md` (the plan), and
`docs/firmware-selection.md` (why we build ROMs rather than import one).
`architecture.md`, covering how it all fits together, has not been
written yet.

Base commit: `8a9f739d` (`pcjs-v1`).

---

## 1. Review first

Before writing anything, the existing implementation at HEAD was read and
run. It did not compile:

```
45 errors, all from the firmware integration
```

and the design did not justify saving it. `init_firmware()` had no callers,
so the whole feature was dead. What existed was not a BIOS: a software-
interrupt-only trap in real mode, with no ROM image, no reset vector, no
POST and no interrupt handling. There was also a VBE bug and a test that
pointed at a developer's local path:

```
tests/firmware/run.mjs:21-33  import CPUx86 from "/Volumes/HOME/Users/dev/work/pcjs/..."
```

Upstream `94faf3d6` was also checked and compiles clean, so the breakage
was local to this work. The review is recorded in
`docs/firmware-selection.md`.

## 2. Why no third-party ROM was adopted

Searched in 2026, including the licence text of every candidate rather than
just the repository description. **Nothing usable exists**:

| Candidate | Why not |
|---|---|
| PCjs ROM images | PCjs's own `LICENSE.txt` limits the MIT grant to "the PCjs repository and website, **excluding all programs, images and documentation produced by other parties**". They are IBM ROM dumps. |
| `fysnet/i440fx` | Technically the best fit — but **no `LICENSE` file**, and it ships a prebuilt `.bin`. Also needs USB and SATA, which v86 does not emulate. |
| `640-KB/GLaBIOS` | GPL-3.0 |
| SeaBIOS / Bochs VGABIOS | GPL-2.0-or-later / LGPL-3.0 |
| `b-dmitry1/BIOS` | MIT, but minimal: "Int 13h supports only reset/read/write". No EDD, no El Torito, no VBE. |
| OVMF / EDK II | BSD-2-Clause-Patent, but UEFI only — no legacy INT 10h/13h. |

**Decision: build the ROMs, copy only PCjs *methods* (MIT code).** This
corrects a premise in `todo.md` §2.2, which assumed PCjs being MIT made
PCjs's BIOS available. It does not.

## 3. New firmware

A separate crate, `src/rust/firmware`, MIT, unit-testable on its own and
independent of the emulator.

### New modules

| File | What it is |
|---|---|
| `asm.rs` | 16-bit assembler that emits both ROM images. Needed reg/rm and rel-displacement fixes. |
| `font.rs` | Generated 8x16 character set for the video ROM. |
| `rom.rs` | Builds the 64 KiB system ROM at `F000:0000` and the 32 KiB video option ROM at `C000:0000`. Defines `TRAP_VECTOR` and `is_firmware_rom`. |
| `irq.rs` | IRQ 0, 1, 6, 8 and 12. |
| `debug.rs` | Trace ring, readable from the host. |

### Rewritten

`vbe.rs` (spec-correct `AX=4F00`–`4F0Bh` behind a `VideoHost` trait),
`post.rs`, `bda.rs`, `ivt.rs`, `int10.rs`, `dispatch.rs`, `machine.rs`.

### The trap design

Leaf services run on the host and are reached from ROM stubs:

```asm
push 0x0010      ; service id
int  0x66        ; honoured only from inside a firmware ROM window
iret
```

Vector `0x66` is left free for guests: the emulator consults
`is_firmware_rom(*instruction_pointer)` before honouring the trap, so a
guest `INT 66h` is an ordinary interrupt.

v86's `reset_cpu` already lands at `F000:FFF0`, so the firmware's reset
vector is used with **no CPU change**.

### `standalone` feature

`ffi.rs` was leaking `env::v86_*` imports into the v86 wasm build. Gated
behind a new `standalone` Cargo feature, off by default.

## 4. Emulator integration

| File | Change |
|---|---|
| `src/rust/fw_adapter.rs` | Rewritten against the real CPU API. Host ABI: `v86_firmware_init`, `_add_floppy`, `_drive_ptr`, `_drive_len`, `_set_trace`, `_set_rtc`, `_trap_ring*`. |
| `src/rust/cpu/cpu.rs` | Trap in `call_interrupt_vector`. `set_eflags` fixed — condition codes are **lazily evaluated**, so writing `*flags` without clearing `flags_changed` let the next `getcf()` overwrite it with a stale result. |
| `src/cpu.js` | `load_firmware()`, the firmware wasm imports, and `firmware: "pcjs"`. |
| `src/browser/starter.js` | Passes `firmware` and `firmware_trace` through — the option table copies by hand, so an unrecognised key would be silently dropped. |

`src/cpu.js` also snapshots `firmware_boot_drives` **before** `new IO(this)`
runs, because device setup detaches the disk buffers.

## 5. Bugs found by testing, and fixed

The acceptance test is a hand-assembled boot sector that exercises the
services a real loader depends on and prints `RESULT: PASS` / `RESULT: FAIL`.
It found five bugs that no amount of reading had.

### 5.1 `patch_saved_flags` corrupted the return address

**The root cause of everything.** The trap fires *before* `int 0x66`
pushes anything — `instruction_pointer` already holds the return address —
so after the service id is popped, the stack is exactly the frame the
guest's own `INT n` pushed:

```
SS:SP+0   IP        <- what was being written
SS:SP+2   CS
SS:SP+4   FLAGS     <- where the carry belongs
```

The carry bit was written at `SS:SP`, into the low byte of the **saved
return address**, redirecting the caller into the middle of its next
instruction.

| Symptom | Explanation |
|---|---|
| Guest hung under the interpreter | The corrupted IP landed on the `0xF4` (`hlt`) byte inside a `jmp`, and halted. |
| `RuntimeError: table index is out of bounds` under the JIT | The same corruption, seen by the translator. |

**These were one bug, not two.** It also meant every status-returning
service silently reported success, because the carry never reached the
caller — the reason `INT 13h AH=41h` failed check D while looking fine.

Fixed with `SAVED_FLAGS_OFFSET` named so it cannot drift again, plus three
regression tests, one of which asserts the saved return address is
byte-for-byte unchanged.

SeaBIOS relies on the same layout: its `struct bregs` overlays the frame
and `regs->flags` is the word at that offset.

### 5.2 A third-party BIOS's `int 0x66` was swallowed

**A regression introduced by this work, caught while using the oracle.**

The trap was guarded on `is_firmware_rom` alone. But SeaBIOS and the Bochs
BIOS occupy exactly the address windows this firmware claims — `F000:0000`
and `C000:0000` — so an `int 0x66` executed by a *third-party* ROM satisfied
the ROM test. `firmware_trap()` found no firmware installed, returned, and
`call_interrupt_vector` returned too — **without ever dispatching through the
IVT**. The interrupt simply vanished.

SeaBIOS does execute an `int 0x66` (one occurrence, at file offset
`0x1E386`).

Fixed by requiring both conditions:

```rust
if crate::fw_adapter::firmware_is_installed()
    && crate::fw_adapter::is_firmware_rom(*instruction_pointer as u32)
```

Leaving `firmware` unset must keep working exactly as before, and that
invariants check was not obvious enough to get right by inspection.

### 5.3 EDD installation check returned a malformed answer

`src/rust/firmware/src/int13.rs`:

| Field | Was | Now |
|---|---|---|
| `AH` | `0` (the classic INT 13h success code) | `03h` (Extensions major version) |
| `DH` | `0x20` | `00h` (minor version) |
| `DL` | the drive the caller asked about | the number of drives |

A caller parsing the version got "version 0". A caller counting drives got
1 regardless of how many existed. Not caught by the self-test — found by
comparing against a real BIOS (see §7).

### 5.4 Four bugs in the test itself

| Bug | Consequence |
|---|---|
| Disk address packet fields written two bytes late; buffer segment derived from the offset | The EDD read landed at the wrong address. |
| Boot signature compared against the wrong byte of a word load | A correct read reported failure. |
| `INT 10h AH=0Fh` column count read from `AL`; it lives in `AH` | Tested the video mode instead. |
| RTC century read from `AH`; it lives in `CH` | **Passed for the wrong reason** — `AH` still held the `0x02` from the `mov ah` that made the call. |

The last one is why the oracle exists: our firmware and the test shared an
assumption, and only a different BIOS could expose it.

## 6. Verification

| Check | Result |
|---|---|
| `cargo check` (host, and `--target wasm32-unknown-unknown`) | 0 errors, 0 warnings |
| `cargo test -D warnings` (firmware crate) | **62 pass** (was 58) |
| `node examples/firmware.js` (JIT) | **exit 0, `RESULT: PASS`** |
| `node examples/firmware.js` (`FW_NO_JIT=1`) | **exit 0, `RESULT: PASS`** |
| `node examples/firmware-oracle.js` (`FW_NO_JIT=1`) | SeaBIOS boots the same sector |
| `npx eslint src examples tools gen lib` | 0 issues |

### SeaBIOS cannot boot under the v86 JIT

`FW_ORACLE=seabios node examples/firmware-oracle.js` **without** `FW_NO_JIT`
dies with `RuntimeError: table index is out of bounds`. The built-in firmware
boots under the JIT; SeaBIOS does not.

This is **not** the firmware work, and it was verified rather than assumed:

- It reproduces with `firmware` unset, so no firmware is installed
  (`firmware_present() == 0`).
- It reproduces with a boot sector that is only `cli; hlt` — the guest does
  nothing, so the crash is inside SeaBIOS's own POST.
- After the §5.2 fix the trap cannot fire at all in that configuration.
- `set_eflags` is called from exactly one place, inside the firmware; with no
  firmware installed it is never reached.

- Removing `set_jit_block_boundary` (see below) did not change it.
- It is deterministic: 5 runs in a row, same failure.
- The site is `wasm::call_indirect1` in `src/rust/cpu/cpu.rs:3246`, called
  with `wasm_table_index + WASM_TABLE_OFFSET`, where the index comes from the
  JIT's TLB entry. An index out of range there means a stale or corrupt cache
  entry, which is JIT bookkeeping — code this work does not touch.

It could **not** be bisected against `8a9f739d`, because HEAD does not
compile — the 48 errors in §1. So "pre-existing" is inferred from the above,
not proven. Either way it is a v86 JIT bug, and the fix belongs there
rather than in the firmware.

**Practical consequence:** the oracle scripts need `FW_NO_JIT=1`. The README
documents this, and it is the first thing to reach for whenever something
misbehaves.

### Dead code removed

`set_jit_block_boundary()` was added speculatively while chasing the §5.1
crash, on the theory that redirecting CS:IP from outside the CPU core needed
telling. It has no callers, the crash was elsewhere, and the firmware boots
under the JIT without it — so it was removed rather than left as unused
surface.

## 7. Using `bios/` as a behavioural oracle

Rather than keep arguing with the specification, the *same* boot sector was
booted against SeaBIOS and the Bochs BIOS from `bios/`, and a service probe
was added to record what a BIOS actually returns:

```console
$ node examples/firmware-service-probe.mjs          # built-in firmware
$ FW_ORACLE=seabios node examples/firmware-service-probe.mjs
```

This settled three questions that reading had not:

- **EDD on a floppy is not a given.** SeaBIOS and Bochs both refuse
  `AH=41h` on drive 0 and leave `BX=55AAh` untouched. We accept it,
  because we do implement EDD reads on floppies — but a caller must not
  assume either behaviour.
- **`INT 1Ah AH=02h` has no dependable answer here.** Both reference BIOSes
  return a day of `0`, because v86's CMOS RTC is not populated the way real
  hardware would be. Ours reads the emulated clock directly and is
  unaffected. Recorded as RTC-1 in `TechDebt.md`.
- **`INT 12h` legitimately differs.** We report configured memory
  (`AX=8000h`); SeaBIOS and Bochs report conventional memory (`AX=027Fh`).

The images are GPL-2.0-or-later / LGPL-3.0 and are **only ever queried**.
Nothing is copied or disassembled into. This is the pattern `todo.md` §4.5
already sets for QEMU: an oracle to consult, never code to copy.

## 8. Known-red, and not from this work

`npx eslint tests/firmware` reports 59 errors. Verified present on a
pristine `git worktree` at HEAD (`8a9f739d`) in files untouched since, so
`make eslint` was already red before the ROM firmware landed. Those files
are the stale harness described in `TechDebt.md` BUILD-2 — recorded rather
than silently expanded into.

## 9. Build notes

The documented toolchain could not run in the development container (no
`clang`, no Java, no root). The wasm was produced with a downloaded
**Zig 0.14.1** using `zig cc --target=wasm32-wasi` for `build/softfloat.o`
and `build/zstddeclib.o`, and `build/libv86.mjs` / `build/libv86.js` are
hand-written one-line shims re-exporting `src/main.js`.

**Closure Compiler has therefore never been run against this firmware**,
so the shipped bundle size is unmeasured. Recorded as BUILD-1 in
`TechDebt.md`.

## 10. Files

**Modified (20):** `Cargo.toml`, `src/browser/starter.js`, `src/cpu.js`,
`src/rust/cpu/cpu.rs`, `src/rust/firmware/Cargo.toml`, and 16 sources under
`src/rust/firmware/src/` and `src/rust/fw_adapter.rs`.

**Added (13):**

| File | Purpose |
|---|---|
| `src/rust/firmware/src/{asm,debug,font,irq,rom}.rs` | The new firmware modules |
| `examples/firmware.js` | End-to-end boot test |
| `examples/firmware-selftest.mjs` | The hand-assembled boot sector and its 16-bit assembler |
| `examples/firmware-debug.js` | ROM/IVT/BDA/trace-ring/screen diagnostic |
| `examples/firmware-oracle.js` | Runs the same sector against a real BIOS |
| `examples/firmware-service-probe.mjs` | Records what a BIOS returns for a service |
| `TechDebt.md`, `docs/firmware-selection.md` | Analysis and debt register |

## 11. Not done

Listed so nothing here reads as more finished than it is:

- `architecture.md` is referenced by `TechDebt.md` but has not been written.
- **No CD-ROM reaches the firmware.** `v86_firmware_add_floppy` is the only
  drive-registration entry point, so `boot_from_cd()` is unreachable dead
  code. CD-1/CD-2 in `TechDebt.md`.
- **No USB**, in the emulator or the BIOS, and no WebUSB mapping. USB-1
  through USB-3.
- **No loadable option-ROM infrastructure**, which both of the above need.
  ROM-1.
- **Nothing runs in CI.** TEST-1/TEST-3.
- **SeaBIOS does not boot under the v86 JIT** (§6). It needs
  `FW_NO_JIT=1`. This is a v86 bug, not a firmware one, and it could not be
  bisected against HEAD because HEAD does not compile.