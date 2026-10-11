# Changes

Session log for the permissive-firmware work: what changed, why, and how
each claim was checked. Companion documents: `README.md` (what the
firmware is and how to use it), `TechDebt.md` (what it deliberately does
not do), `todo.md` (the plan),
`docs/firmware-selection.md` (why we build ROMs rather than import one),
and `docs/architecture.md` (how it all fits together).

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
| `cargo test -D warnings` (firmware crate) | **86 pass** (was 58) |
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
| `examples/cd-boot.js` | Boots a disk or CD image |
| `examples/build-test-iso.mjs` | Builds a minimal El Torito ISO for the CD path |
| `tools/httpfs-v86-server.py` | Serves httpfs-backed files over plain HTTP |
| `TechDebt.md`, `docs/firmware-selection.md` | Analysis and debt register |

## 11. Hard disk and CD-ROM support

### 11.1 The blocker

`v86_firmware_add_floppy` was the only drive-registration entry point, so
`DriveKind::HardDisk` and `DriveKind::CdRom` were never populated and
`boot_from_cd()` was unreachable dead code.

The obvious fix — copy the image into the firmware — does not survive contact
with a real ISO. The Debian netinst image is 640 MiB; v86 hands images over as
JavaScript buffers that Rust cannot address, so copying one means growing the
wasm heap by most of a gigabyte and holding it for the machine's lifetime.

So drives are registered **by reference**. `HostImage` keeps the geometry and
pulls each sector across the host boundary as it is needed:

```
guest INT 13h → firmware → Machine::read_host_image → js::read_host_image
             → CPU.prototype.firmware_read_image → JS buffer → wasm memory
```

`dest` is a Rust slice pointer — an address in the wasm linear memory — so the
copy is a direct `Uint8Array.set`. One host call per sector read, invisible
next to the emulated CPU work around it. Floppies keep the copy-in path;
1.44 MiB is not worth a callback.

| Added | |
|---|---|
| `backend.rs` | `HostImage`, a `BlockBackend` that reads through the host |
| `machine.rs` | `Machine::read_host_image`, defaulting to "unsupported" |
| `cpu.rs` | the `read_host_image` wasm import |
| `fw_adapter.rs` | `v86_firmware_add_drive(image, kind, sectors, sector_size)` |
| `cpu.js` | `register_firmware_image`, `firmware_read_image` |
| `starter.js` | the import |
| `lib.js` | Node can now fetch `http:` URLs, which it previously could not |

Drives are numbered the way hardware is: floppies `0x00`-, hard disks
`0x80`-, CD-ROMs `0xE0`-, with 2048-byte sectors for optical media.

### 11.2 Three real bugs in El Torito, each fatal on its own

None of this had ever executed. El Torito boot was dead code that looked
alive, and the only test for it asserted that a *blank* image was rejected.

```rust
&sector[1..7] == b"CD001"      // six bytes compared against five -- never true
```

`"CD001"` is five bytes at offsets 1..6. `parse_boot_info` could therefore
only ever return `NotIso`.

```rust
boot_system_id[21..]           // overlaps the signature
```

`"EL TORITO SPECIFICATION"` is 23 characters, so the blank remainder of the
32-byte identifier field starts at 23. Starting at 22 rejected every
otherwise-valid boot record.

```rust
// the catalogue sector was assumed to open with a signature
```

It does not. It opens with the **validation entry**: header id `0x01`,
platform, identifier string, key bytes `55 AA` at offsets 30-31, and a
sixteen-word sum that must be zero. Debian's isohybrid boot info block at
LBA 1119 is laid out exactly this way. An implementation that expects
`EL TORITO SPECIFICATION` at offset 0 will not read any real image.

Each is now pinned by a test that builds a bootable image in memory and
asserts it parses — the case that was missing.

### 11.3 What the ISOs in `~/iphone/cdrom` actually are

| | Debian netinst | NetBSD 11.0 |
|---|---|---|
| MBR / hybrid | yes, isohybrid | no |
| El Torito boot record at LBA 17 | yes | yes |
| Boot catalogue | LBA 1119, valid; default entry is no-emulation, 4 sectors from LBA 3344 | none usable |

Debian is **hybrid**: it boots from its isohybrid MBR or from El Torito.
`examples/cd-boot.js` reports which structures it found before trying, so a
failure says which path was expected.

I first concluded that neither image had an El Torito catalogue. That was
wrong: I searched for the `EL TORITO SPECIFICATION` string, which real
catalogues do not contain, and had the default entry's sector-count field one
byte early. Both were the same mistake the firmware had made — finding it in
the firmware is what sent me back into the images.

### 11.4 Serving the images over HTTP, through httpfs

v86 loads images with `fetch()` in the browser but only `fs` under Node, so
nothing served over HTTP was testable from a script. `src/lib.js` now
fetches when handed an `http:`/`https:`/`file:` URL under Node.

`tools/httpfs-v86-server.py` bridges httpfs to plain HTTP. The httpfs server
speaks a JSON RPC for POSIX operations and rejects any request without an
`HttpFsClient` user agent, so it cannot be fetched directly. The bridge opens
each file with `OP_OPEN`, reads ranges with `OP_READ`, and serves
`GET`/`HEAD` with `Range`. **The bytes genuinely come from httpfs**; only the
protocol in front is translated.

```console
$ python -m httpfs.server 8099 /localdisk/home/dev/work/iphone/cdrom/
$ tools/httpfs-v86-server.py --port 8100
$ node examples/cd-boot.js http://127.0.0.1:8100/debian-12.1.0-i386-netinst.iso
```

Verified byte-identical to the local files at six offsets, including the MBR
signature and the El Torito boot record.

### 11.5 What boots

| Image | Device | Result |
|---|---|---|
| `test-hd.img` (self-test as MBR) | `hda` | **boots**, self-test runs |
| `hd-512.img`, 512 MiB | `hda` | **boots** — size is not the limit |
| `test-boot.iso` | `cdrom` | **boots via El Torito**, no-emulation |
| `test-boot.iso` over httpfs | `cdrom` | **boots**, interpreter and JIT |
| `debian-12.1.0-i386-netinst.iso` | `cdrom` | firmware parses its catalogue and loads its boot image; Debian's loader then stalls |
| `debian-12.1.0-i386-netinst.iso` | `hda` | same |
| `NetBSD-11.0-i386.iso` | `cdrom` | no usable boot catalogue |

The Debian stall is **not** a firmware regression, and that was checked rather
than assumed:

- SeaBIOS fails on the same image as `hda` from a local file, ending at the
  same `cs:eip` with a garbage signature at `0000:7C00`.
- The firmware's part demonstrably works: the trace shows it reading LBA 16
  (primary volume descriptor), LBA 17 (boot record) and LBA 1119
  (catalogue), loading the no-emulation image and jumping to it. The guest
  then performs 16 further INT 13h reads of its own before stopping.
- SeaBIOS could not load the 640 MiB CD through the bridge within 150 s,
  because that path copies the image. The by-reference design streams it in
  ~20 s — an asymmetry that is a side benefit, not the point.

Debian's early boot loader reaches a state v86 does not carry it past, under
any BIOS. Filed as **BOOT-1** in `TechDebt.md`.

### 11.6 Testing against real 32-bit images

The self-test ISO proves the CD path works, but it is a boot sector that
prints and halts. To test against something real, two images were taken from
v86's own Advent calendar, which publishes only images that "are 32-bit x86
and work in v86":

| Image | Size | El Torito |
|---|---|---|
| [`FreeNOS-1.0.3.iso`](https://i.copy.sh/FreeNOS-1.0.3.iso) | 10.5 MiB | no-emulation, 4 sectors from LBA 3800, catalogue at LBA 46 |
| [`HelenOS-0.11.2-ia32.iso`](https://i.copy.sh/HelenOS-0.11.2-ia32.iso) | 24.6 MiB | no-emulation, 56 sectors from LBA 64, catalogue at LBA 63 |

Both have the catalogue LBA at offset `0x47`, which is what the firmware
reads, and both parse correctly.

| | SeaBIOS | built-in firmware |
|---|---|---|
| FreeNOS | **reaches `login:`** | panics: `Unimplemented: #GP handler` on `POP ES` |
| HelenOS | boots | panics, identically |

So the firmware's CD path is sound, and there is a **separate, real defect at
the boot handoff** for protected-mode guests — recorded as **BOOT-2** in
`TechDebt.md`, with what has been ruled out (the CD path, host I/O, and A20)
and what has not.

That defect turned out to be real hardware handoff state: the bootstrap runs
through a software `int 0x19`, which clears IF in the CPU core, and nothing
re-enabled it, so the loader started with interrupts off. Enabling them at
the handoff is correct and matches what SeaBIOS does, but it did not resolve
BOOT-2.

### 11.7 New examples and tooling

| | |
|---|---|
| `examples/cd-boot.js` | boots a disk or CD image, reporting which structures it found |
| `examples/build-test-iso.mjs` | builds a 21-sector El Torito ISO around the self-test boot sector, giving the CD path a controlled subject with no ISO tooling installed |
| `tools/httpfs-v86-server.py` | httpfs → plain HTTP, with `Range` |

## 13. Recent work

### 13.1 E820 terminator fixed

The E820 memory map could never end: resetting `EBX` to 0 made the
exhaustion guard unreachable. Now `CF` is set, `AH=04h`, and `EBX` is
untouched when the map is walked to its terminator. A test walks the
map like a loader would.

### 13.2 CD EDD reads verified byte-exact

`examples/cd-read-check.mjs` reads 8192 bytes through the firmware's
EDD path and compares them byte-for-byte against the same bytes read
directly from the image. All 8192 match.

### 13.3 Protected-mode reproducer

A synthetic boot sector in `examples/firmware-selftest.mjs` enters
protected mode, installs a GDT, clears PE, and returns to real mode.
Seven bugs were in the test itself (GDT access byte offset, pseudo-descriptor
overlap, LGDT/SGDT ModRM encoding, `and eax,imm8` encoding, far jump
offsets, 0x66 prefix, CS:IP restoration). All fixed — the self-test passes
end-to-end under both interpreter and JIT.

### 13.4 LGDT probe

`examples/lgdt-probe.mjs` — a 20-line boot sector proving `LGDT`/`SGDT`
round-trip correctly. This refuted the earlier claim that v86 does not
store the GDT; the bug was in the probe's own ModRM encoding.

### 13.5 FreeNOS fault

`examples/freenos-fault.mjs` boots FreeNOS under the firmware and catches
the fault: `switch_seg → trigger_gp → call_interrupt_vector → panic`.
Fault at `CS=8:EIP=0x5fe5` (byte `07` = `POP ES`). The thunk ping-pongs
between `CS=8` (0x82ef) and `CS=0x18` (0x8369) — the protected-mode
BIOS-call thunk. No IDT installed at that point → panic. This is BOOT-2.

### 13.6 VM snapshot work

- The vm front end now uses `firmware: "pcjs"` and `vga_memory_size: 8 MiB`.
- `vm-boot-check.mjs` boots the warm-boot snapshot headless, watches serial.
- Snapshot restore is sound: `CR0=0x80050033` (protected mode, paging on),
  segment registers intact. Guest does not resume because the snapshot was
  saved with CPU halted (`state[17] in_hlt = 1`). Filed as **VM-1**.
- The 8 MiB VGA surface was a real bug: the snapshot saved with 8 MiB, the
  page asked for 2 MiB → `offset is out of bounds`. Filed as **VM-2**.
- `vm/` converted to a proper submodule (`git@github.com:alex-iphone/vm.git`).
- `bios-seabios` branch created at `8f611b3` (SeaBIOS/VGABIOS); `main`
  at `99c6a41` (new firmware). Both pushed to origin.

### 13.6 Browser test case

`examples/firmware.html` is the browser mirror of the SeaBIOS-era
examples (`basic.html` and friends boot v86 with
`bios: { url: "../bios/seabios.bin" }`). It boots the self-test boot
sector, assembled in the page, on the built-in firmware — no ROM image
fetched at all — and renders the emulated VGA screen.

A dropdown also boots the same sector against SeaBIOS and the Bochs BIOS
from `bios/`, so a difference between the rows is a firmware difference
rather than a guest bug. The reference BIOSes run with `disable_jit: true`,
carrying JIT-1 over from the Node side; the built-in firmware runs with
the JIT on.

`examples/firmware-browser-check.mjs` validates the page headlessly: that
the ids the script reads match the ids the page defines (a typo throws
only at boot time, in the browser), that the three selections are wired
including the JIT switch, and that the page's configuration actually
boots the self-test to `RESULT: PASS`. It cannot check the DOM and
canvas half — the ScreenAdapter needs a 2d context — which is noted in
its output rather than claimed as covered.

### 13.7 Option ROM support (ROM-1 closed)

This was the blocker for every path that boots a kernel, and it was the
reason the vm guest could not cold boot at all.

v86 boots a `bzimage` by building a 512-byte stub and pushing it into
`cpu.option_roms`. `kernel.js` describes it plainly:

> This rom will be executed by seabios after its initialisation

The firmware built two images and ran neither of them from anywhere but
their fixed places, so the stub was data in a buffer and the kernel never
started. A cold boot of the vm guest ended at `cs:eip = f000:237` —
inside the BIOS — with INT 19h the last service dispatched and 0 bytes on
the serial line.

`option_rom.rs` now runs every ROM the host registers:

* validates `55 AA`, the length in 512-byte blocks, and the byte sum
  over the declared length;
* copies the image into `0xD0000`–`0xDFFFF`, back to back on its own
  length, which is where a guest reading them back expects to find them;
* enters it with a **far call**, so a ROM that returns resumes the
  firmware instead of running off the end of its image. The return
  address is a five-byte stub in the system ROM that far-jumps to the
  INT 19h stub — everything POST has left to do is the bootstrap.

The far call needs a `push_u16` on the `Machine` trait, which did not
exist: the one place the firmware builds a frame itself is entering a
ROM. `push_u16` is the counterpart to `pop_stack_u16` and writes back
through the CPU's stack pointer, so a 32-bit stack is handled.

Registered from `src/cpu.js` as a separate step, `register_firmware_option_roms()`,
because it has to run *after* every source of ROMs: `load_kernel` pushes
its stub during construction, and `restore_state` rebuilds them too.
Registering inside `load_firmware` saw an empty list and ran nothing —
the first bug, and the reason the first test after fixing it still
failed.

With that in place a cold boot of the vm guest reaches:
`Decompressing Linux... Booting the kernel`, `cs=0x60` with a 32-bit
`eip`, and 17 firmware traps with INT 10h the last service. That is
further than a cold boot of this guest has ever gone, and it stops at
exactly where `vm/state/v86state.bin.zst` was taken — see VM-3.

### 13.8 A v86 restore bug found while testing

`examples/vm-state-check.mjs` saves a running machine and restores it
into a fresh one. Restoring died with

```
TypeError: Cannot read properties of undefined (reading 'original_bar')
    at PCI.set_state
```

`pci_bars` is not densely indexed: `ide.js` leaves `undefined` in the
slots of a channel that is not present, and `virtio.js` assigns
`pci_bars[cap.bar]` for whatever BAR number the capability names.
`set_state` walked `pci_bars.length` and indexed without checking, while
the two other call sites in the same file already guarded with
`if(!bar) continue;`. Fixed the same way. Filed as **V86-1**.

### 13.9 The kernel blocker: E820 stopped at 64 MiB on a 128 MiB machine

Written up in full under VM-3 in `TechDebt.md`. The short version, since
it is the emulator bug the cold boot was actually hitting:

* `Config::memory_kib` is the 16-bit BDA word at 0x413 — conventional
  memory, which cannot describe more than 64 MiB. The E820 map was being
  built from it.
* A guest sizing its page tables from E820 therefore placed them where
  the emulator had no memory, which is exactly what the boot showed:
  writes to physical 0x35045FBC, ~880 MiB past the end of RAM.
* `Config::total_memory_kib` is now separate and unclamped, and
  `INT 15h AH=88h` / `AX=E801h` use it. The E801 code also had a unit
  bug — `mem_kib - 640 * 1024` — that treated a KiB count as bytes.

**Measured, not assumed.** With this in place the built-in firmware
reaches the *identical* state SeaBIOS does: the same `cr3`, `cr4` and a
page fault at the same `cr2`. The differential is the honest way to tell
a firmware difference from an emulator one, and here it showed the
firmware had caught up and the remainder is v86's.

### 13.10 `vm-kernel-fault.mjs`: where the kernel stops

The diagnostic that answered the above, rather than inferring it:

* translates the guest's EIP through *its own* page tables, because a
  kernel at `0xc0000000` with paging on has a linear EIP that is not an
  address in the emulated RAM, and reading `mem8[eip]` gives nothing;
* walks the CR registers and prints them by their hardware numbers,
  because v86 stores them by slot (`cr[3]` is CR3, `cr[4]` is CR4) and
  the obvious numbering is off by one;
* logs the control registers as they change, because "CR3 was never
  loaded" and "CR3 was loaded and then lost" look the same in a final
  state dump;
* dumps the IDT entry thunk the guest is stuck in, which is what turned
  "it hangs after Booting the kernel" into "it is in an interrupt storm
  in `irq_entries`".

Three of those four were bugs in earlier versions of this script, not in
the emulator, and each one changed the conclusion. Worth remembering
before trusting a state dump.

## 14. Not done

Listed so nothing here reads as more finished than it is:

- **No USB**, in the emulator or the BIOS, and no WebUSB mapping. USB-1
  through USB-3.
- **No USB**, in the emulator or the BIOS, and no WebUSB mapping. USB-1
  through USB-3.
- **No loadable option-ROM infrastructure**, which USB-2 needs. ROM-1.
- **`INT 13h AX=4B00h/4B02h` are still absent.** The firmware boots from a CD
  at POST, but a loader that wants to boot from CD later has nothing to call.
  CD-2 in `TechDebt.md`.
- **Debian's and NetBSD's installers do not complete boot**, though the
  firmware parses and loads their El Torito images correctly. BOOT-1 in
  `TechDebt.md`.
- **Nothing runs in CI.** TEST-1/TEST-3.
- **SeaBIOS does not boot under the v86 JIT** (§6). It needs
  `FW_NO_JIT=1`. This is a v86 bug, not a firmware one, and it could not be
  bisected against HEAD because HEAD does not compile.