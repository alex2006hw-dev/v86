# Legal Feasibility Memo — Clean-Room Rust→WASM Re-implementation

> **SUPERSEDED 2026-10-09.** The clean-room re-implementation
> plan was withdrawn: the project pivoted to using v86 (already
> BSD-2-Clause) as the base and replacing only the copyleft BIOS
> firmware (SeaBIOS GPL-2.0+, VGABIOS LGPL) with permissive
> alternatives (PCjs MIT, b-dmitry1/BIOS MIT, from-spec VBE,
> OVMF BSD-2-Patent). The clean-room process was only needed to
> create a *new* MIT emulator from scratch; it is unnecessary
> here. This memo is retained as the historical record of the
> pivot. Current plan: `todo.md` and `THIRD-PARTY-NOTICES.md`.

**Privileged & confidential — draft for internal review**
**Prepared:** 2026-10-09 · **Status:** AI-assisted research memo, **not legal advice**
**Action required:** external IP counsel sign-off before implementation (todo §1.6)

> Disclaimer: this memo was drafted by an AI agent from web research and
> repository analysis. It is not legal advice and creates no attorney-client
> relationship. Notably, **this AI session has read the v86 source**, so it
> must not itself write any clean-room implementation code (see §4.3).

## 1. Question presented

Is it legally feasible to produce a clean-room re-implementation of the v86
x86-32 emulator (currently BSD-2-Clause, with GPL/LGPL BIOS assets) in
Rust→WASM, and release the new code under the MIT License, without
infringing the existing project's rights?

## 2. Short answer

**Feasible, with conditions.** The plan is sound under US copyright doctrine
and the project's actual licensing posture is *more* favorable than assumed
(v86 is BSD-2-Clause today, not GPL-2.0). The clean-room process is not
strictly required by law — independent creation is a complete defense — but
it is the correct risk strategy, and the two-team firewall, provenance log,
and BIOS separate-fetch design in `docs/clean-room-policy.md` match the
standards courts and practitioners expect. Residual risks are: (a) AI-era
scrutiny of the clean-room process itself; (b) patent exposure, which clean
room does **not** address; (c) the GPL/LGPL firmware binaries, which must
stay out of the MIT distribution.

## 3. Facts

- v86 source (`src/`, `src/browser/`, `lib/`) is **BSD-2-Clause**
  (`LICENSE`, `package.json`, `Readme.md` §License). Historically GPL-2.0;
  relicensed by the copyright holder. BSD-2 already permits derivative use
  with attribution — but a derivative work would remain BSD-2, not MIT.
- `src/rust/` is **machine-generated from the JS** by `gen/` → derivative
  BSD-2 work; quarantined (`NOTICE-CLEANROOM.md`).
- `src/floppy.js`: MIT (QEMU 82078 port, `LICENSE.MIT`).
- `lib/softfloat`: BSD-3-Clause (UC Regents). `lib/zstd`: dual BSD-2
  (with patent grant) **or** GPLv2 — BSD option is MIT-compatible.
- `bios/seabios.bin`: **GPL-2.0-or-later**; `bios/vgabios.bin`: **LGPL**
  (Bochs VGABIOS project; `bios/COPYING.LESSER`). Confirmed as prebuilt,
  unmodified upstream blobs (SeaBIOS rel-1.16.x) — standard practice is
  separate distribution, as v86 itself does.
- Public design documentation exists (`docs/how-it-works.md`), so no trade
  secret exposure from reading published material; but its *expression*
  (structure, code, comments, opcode-table organization) is protected.

## 4. Analysis

### 4.1 What copyright protects here

Copyright protects **expression**, not ideas, procedures, processes, systems,
or methods of operation (17 U.S.C. §102(b)). An x86 emulator's *functionality*
— the ISA, register semantics, device behavior defined by the Intel SDM and
hardware datasheets — is unprotectable. What is protected in v86 is its
literal code and, under the **abstraction-filtration-comparison** test
(*Computer Associates v. Altai*, 2d Cir. 1992), its non-literal elements:
structure, sequence, organization, comments, naming, and the specific
expression of design decisions (e.g., the particular TLB layout, the
"stackifier" JIT pipeline, the opcode-table organization in `gen/x86_table.js`).

**Consequence:** a from-specification re-implementation that independently
chooses its own architecture, naming, module boundaries, and code structure
does not infringe — provided no protected expression is copied. The clean-room
firewall exists to *prove* that.

### 4.2 Clean-room doctrine

- Clean room is **best practice, not a legal requirement** (LII/Wex;
  practitioner consensus). Independent creation is a complete defense; the
  two-team Chinese-wall is the standard evidentiary method.
- *NEC v. Intel* (microcode): a later, clean-room-derived microcode that was
  "sufficiently different" was held free of infringement — the classic
  clean-room precedent.
- *Sega v. Accolade* (9th Cir. 1992) and *Sony v. Connectix* (9th Cir.
  2000): functional elements of software — **including BIOS functionality** —
  are not copyrightable (§102(b)); reverse engineering to access them can be
  fair use. Connectix is directly on point: building an emulator around a
  console BIOS's functional interface was lawful.
- *Oracle v. Google* (2021): reimplementing an interface for interoperability
  is transformative fair use; interface elements bound up with uncopyrightable
  ideas receive thin protection. (We go further: the clean-room project writes
  its own host API rather than copying `v86.d.ts` expression — only the
  *interface behavior* is matched, which §102(b) does not protect.)

### 4.3 AI-era risk (2025–2026)

- Law firms (Reed Smith, 2026) note AI-assisted clean rooms receive
  **additional scrutiny**; contemporaneous documentation proving independent
  creation is the key defense. Our provenance log + signed certifications
  directly address this.
- **Process risk specific to this engagement:** an AI (or human) that has
  read the v86 source cannot be an Implementation Team member. This session
  has read it; it may therefore produce only legal/process artifacts, never
  implementation code. Implementation must be done by clean humans or
  zero-exposure AI sessions. Model-training-data contamination is an
  unquantifiable residual risk; mitigate with the AFC-style code review
  (§7 of the policy) and provenance logging.

### 4.4 Licensing compatibility

- New clean-room code is a **new work with new copyright** → MIT release is
  unencumbered. (Relicensing the *existing* BSD-2 code as MIT would require
  all contributors' consent; the clean-room path avoids that question
  entirely.)
- MIT repo must **not** contain: SeaBIOS (GPL-2.0+), Bochs VGABIOS (LGPL),
  or any translated/derived BSD-2 v86 code. Separate-fetch of firmware
  (decision §1.5) matches how v86 itself distributes them and avoids
  combined-work arguments.
- Vendored permissive code (SoftFloat BSD-3, zstd BSD option) may be used
  with notice retention; a from-IEEE-754 softfloat avoids even that.

### 4.5 Non-copyright exposure (outside clean room's scope)

- **Patents:** core x86 ISA patents are largely expired, but emulation/JIT
  patents exist (e.g., WO2018231598A1, binary-translation patents held by
  Intel/VMware/Transmeta successors). Clean room does **not** defend
  patents. Risk for a non-commercial open-source emulator is low but
  non-zero; a freedom-to-operate opinion is a separate, optional exercise.
- **Trade secrets:** low — v86's design is publicly documented; nothing in
  the plan involves improper acquisition.
- **Trademark:** do not brand the new project as "v86" or use its logos;
  pick a distinct name.
- **Contracts/TOS:** none identified that restrict re-implementation from
  public specifications.

## 5. Conditions for feasibility (all must hold)

1. Two-team firewall with 30-day cooldowns and role separation (policy §2, §6).
2. Implementation Team zero-exposure to v86 source, `gen/` tables, golden
   tests, internal design docs, issue trackers (policy §3).
3. All behavior derived only from the whitelisted public specifications
   (policy §4); per-file provenance log with signed certifications (policy §5).
4. No firmware in the MIT distribution; SeaBIOS/VGABIOS as separately
   fetched, license-marked assets; fw_cfg direct boot supported (policy §8).
5. CLA or DCO from every contributor (todo §1.3).
6. AFC-style code review for structural similarity; incident-response
   procedure for suspected contamination (policy §7).
7. External IP counsel reviews policy, CLA, provenance process, and a
   sampled set of implemented files, and issues a clean-room opinion
   **before** first public release (todo §1.6).

## 6. Verdict

**Feasible.** Confidence: high on copyright (given the process is followed
and documented), moderate-high overall after patent/trademark caveats. The
hard gates are organizational (discipline, documentation) and legal-review
turnaround — not technical or doctrinal blockers. Estimated legal-review
cycle: 2–6 weeks with counsel; implementation may proceed in parallel only
after counsel approves the process (not the code).

## 7. Recommended next steps

1. Engage IP counsel; hand over `docs/clean-room-policy.md`, `CLA.md`,
   `THIRD-PARTY-NOTICES.md`, and this memo for review (todo §1.6).
2. Have counsel confirm: (a) the policy's firewall meets the *NEC*/*Sega*
   standard; (b) CLA text; (c) separate-fetch firmware distribution model;
   (d) optional FTO scoping for JIT/binary-translation patents.
3. Only then: freeze the feature matrix (todo §2) and begin Specification
   Team work.
