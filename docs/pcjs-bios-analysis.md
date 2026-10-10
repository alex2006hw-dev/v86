# PCjs BIOS Analysis (todo §8.0a)

**Deliverable for:** clean-room Rust→WASM v86 re-implementation,
firmware workstream · **Date:** 2026-10-09 · **Status:** analysis
complete; vendor-vs-reimplement decision pending (§6)

## 1. Project summary

- **Project:** PCjs Machines — PCx86, "the original IBM PC
  emulator that runs in your browser"
  (`github.com/jeffpar/pcjs`, 1.2k stars, MIT)
- **Author/copyright:** Jeff Parsons, © 2012–2026
- **License:** standard **MIT License** (`LICENSE.txt`).
  Additionally, the README requests (states as required) that
  the notice *"PCjs © 2012-2026 Jeff Parsons"* with links to
  pcjs.org and the GitHub repo be included in every copy and
  **displayed on every web page or computer that it runs on**.
  → Any vendored use must honor this in the host UI (About
  panel / footer). Standard MIT notice retention is legally
  sufficient; the README attribution is a strong courtesy
  obligation — honor it (see counsel, todo §1.6).
- **Guest coverage:** IBM PC, PC XT, PC AT, Compaq DeskPro 386;
  runs DOS, Windows 3.x, OS/2 in the browser.

## 2. Architecture inventory (the key finding)

**PCx86 has no `bios.js`.** The "BIOS" is distributed across the
emulator itself — the emulator-internal BIOS services model:

| Module (`machines/pcx86/modules/v2/`) | Role |
|---|---|
| `interrupts.js` (3,409 lines) | BIOS/DOS vector definitions, **BIOS Data Area layout (0x400–0x4A8)**, per-INT function reference tables |
| `chipset.js` | machine/chipset glue (POST, boot sequencing) |
| `rom.js` / `ram.js` | ROM and RAM components |
| `video.js` | video card — **implements the INT 10h service handlers** (and the VGA ROM entry-point convention, INT 0x6D) |
| `disk.js` / `fdc.js` / `hdc.js` | disk devices — **implement the INT 13h service handlers** (CHS) |
| `keyboard.js` | **INT 16h** handlers, keyboard buffer at 0x41E |
| `serial.js` / `parallel.js` / `mouse.js` | INT 14h / INT 17h / mouse services |
| `computer.js` | supervises all device components; `initBus()` notification pattern |
| `x86.js` + `x86ops.js` + `fpux86.js` + `segx86.js` | PCjs's own 8088–386 CPU interpreter + FPU (separate from our CPU work) |

Design properties that map well onto Rust/WASM:
- **Component model:** every device extends a base `Component`
  class; devices receive an `initBus()` callback after the bus
  exists — a clean dependency-inversion pattern for a device bus.
- **No ROM blobs for boot services:** services live in emulator
  code; ROM images are optional machine artifacts (`rom.js`).
- **Debugger-first instrumentation:** `BackTrack` data-flow
  tracking (per-byte source indexes) — useful reference for our
  diagnostic tooling, not required.

## 3. BIOS service inventory (from `interrupts.js`)

**Vectors:** 0x08–0x1F ROM BIOS vectors (VECTOR_TABLE at
F000:FEF3), including INT 10h VIDEO, 11h EQUIPMENT, 12h
MEM_SIZE, 13h DISK, 14h SERIAL, 15h CASSETTE, 16h KEYBOARD,
17h PARALLEL, 18h BASIC, 19h BOOTSTRAP, 1Ah TIMER, 1B/1C
break/timer hooks, 1D/1E/1F parameter tables and graphics-font
extension vector.

**BIOS Data Area (0x400–0x4A8):** full documented layout —
RS232/PRINTER bases, EQUIP_FLAG, MEMORY_SIZE (0x413), keyboard
flags and 32-byte buffer (0x417–0x41E), diskette motor/seek
status, CRT_MODE (0x449), CRT_COLS (0x44A), REGEN buffer info,
cursor positions for 8 pages (0x450), 6845 base (0x463),
palette, timer low/high (0x46C), reset flag (0x472), hard-disk
status, rows/points-per-char, EGA save pointer, etc.

**INT 10h functions 00h–0Eh:** set mode; set/read cursor type
and position; read light pen; set display page; scroll up/down;
read character; write char/attr; write char; set palette;
write/read dot; write tty.

**INT 13h functions 00h–18h:** reset; get status; read sectors
(CHS: DL=@drive, CH:@cyl, DH:@head, CL:@sector, AL=@count →
ES:BX); write sectors; verify; format; read drive parameters;
get/set DASD type; media change line. (No LBA/INT 13h
extensions — PC/XT-era only.)

**INT 15h functions 80h–91h:** cassette-era API plus
get-extended-memory-size (0x88), wait-event, SYSREQ,
processor-virtual-mode, device-busy loop.

**INT 21h (DOS) and Windows D386 interfaces** are also
defined (WINDBG vector 0x41, WINDBGRM vector 0x68) — PCx86
supports Windows 3.x protected-mode debugging, confirming
386-era coverage.

**VGA ROM convention:** `VIDEO_VGA: 0x6D` — "the default VGA
INT 10h handler invokes this interrupt and IRETs", i.e. PCx86
emulates the VGA-BIOS-as-INT-10h-handler contract without a
VGA ROM image.

## 4. Gap analysis (vs. v86 requirements)

| v86 need | PCx86 coverage | Action |
|---|---|---|
| DOS 3.3–6.22, Win 3.1/95-era boot | ✅ (PC/XT/AT) | none |
| CHS floppy + hard disk (INT 13h) | ✅ | none |
| Text/graphics video (INT 10h, incl. VGA-era modes) | ✅ (video.js) | verify mode-X timing cases |
| Keyboard (INT 16h), serial (14h), parallel (17h), mouse | ✅ | map to PS/2 + WASM input |
| CD-ROM / El Torito ISO boot | ❌ no ATAPI component | fw_cfg direct boot; or add ATAPI from spec |
| LBA / INT 13h extensions | ❌ | add from spec if needed for LBA disks |
| CMOS/RTC (INT 1Ah, MC146818) | partial (timer vectors; RTC as device) | implement CMOS from MC146818 datasheet |
| NE2000, SB16, virtio, PCI, ACPI, APIC | ❌ (PC/XT-era ISA machine) | out of scope for the BIOS layer; our existing device plan (todo §5) |
| UEFI | ❌ | OVMF Phase 2 (todo §8.0e) |
| v86 API (screen/keyboard/mouse adapters, bus connector) | different model | adapter layer per todo §6 |

## 5. Porting assessment

- **Size:** PCx86 modules total roughly 30–40k lines of JS; the
  BIOS-relevant surface (`interrupts.js`, `chipset.js`, `video.js`,
  `disk.js`/`fdc.js`, `keyboard.js`, `computer.js`) is the large
  majority of that.
- **Effort:** vendor-port ≈ 2–4 engineer-months to Rust/WASM
  (including video, the biggest piece); from-spec reimplementation
  ≈ 3–6 engineer-months with PCjs as behavioral oracle.
- **Mapping to our architecture:**
  - `Component`/`initBus()` → Rust device trait + bus registration
  - BIOS Data Area → fixed 1 KB struct at 0x400 in guest RAM
  - INT vectors → interrupt-dispatch table in the CPU's exception path
  - `video.js` framebuffer → WASM canvas backend
  - `disk.js`/`fdc.js` → WASM block-device backend
- **Risk:** video.js is the largest and most subtle component
  (CRT timing, palettes, EGA/VGA planes); budget accordingly and
  test against DOS game suite (todo §7.4).

## 6. Recommendation — vendor PCjs (MIT) directly

**Recommended: vendor PCjs's firmware components under MIT**
(the clean-room process was withdrawn — with v86 as the BSD-2
base, a from-scratch reimplementation is unnecessary):

1. **Vendor** PCjs's BIOS-relevant components (`interrupts.js`,
   `chipset.js`, `video.js`, `disk.js`, `fdc.js`, `keyboard.js`,
   `computer.js`) and port them to the Rust/WASM device model,
   retaining the MIT notice and the README attribution in
   THIRD-PARTY-NOTICES and in the host UI.
2. **Fill gaps from public specs:** VESA VBE 2.0+ (INT 10h
   AX=4Fxx), MC146818 CMOS/RTC, INT 15h memory map, A20 — the
   vector numbers, function numbers, register layouts, and the
   BIOS Data Area layout are functional facts, not expression.
3. **Counsel check (todo §1.4):** confirm treatment of the PCjs
   README attribution clause ("display on every web page or
   computer that it runs on") before release.

## 7. Handoff

No clean-room restriction applies — PCjs is MIT and v86 is
BSD-2-Clause, so implementation may proceed by any developer or
AI session. Implementers should still avoid copying SeaBIOS or
VGABIOS (GPL/LGPL) code or structure; the firmware must be
derived from PCjs (MIT), the public specs, and the hardware
datasheets only.
