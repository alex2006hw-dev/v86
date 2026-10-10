[![Join the chat at https://gitter.im/copy/v86](https://badges.gitter.im/Join%20Chat.svg)](https://gitter.im/copy/v86) or #v86 on [irc.libera.chat](https://libera.chat/)

v86 emulates an x86-compatible CPU and hardware. Machine code is translated to
WebAssembly modules at runtime in order to achieve decent performance. Here's a
list of emulated hardware:

- Forked system images at ```https://github.com/alex-images/v86-images```
- An x86-compatible CPU. The instruction set is around Pentium 4 level,
  including full SSE3 support. Some features are missing, in particular:
  - Task gates, far calls in protected mode
  - Some 16 bit protected mode features
  - Single stepping (trap flag, debug registers)
  - Some exceptions, especially floating point and SSE
  - Multicore
  - 64-bit extensions
- A floating point unit (FPU). Calculations are done using the Berkeley
  SoftFloat library and therefore should be precise (but slow). Trigonometric
  and log functions are emulated using 64-bit floats and may be less precise.
  Not all FPU exceptions are supported.
- A floppy disk controller (8272A).
- An 8042 Keyboard Controller, PS2. With mouse support.
- An 8254 Programmable Interval Timer (PIT).
- An 8259 Programmable Interrupt Controller (PIC).
- Partial APIC support.
- A CMOS Real Time Clock (RTC).
- A generic VGA card with SVGA support and Bochs VBE Extensions.
- A PCI bus. This one is partly incomplete and not used by every device.
- An IDE disk controller.
  - A built-in ISO 9660 CD-ROM generator with Joliet support.
- An NE2000 (RTL8390) PCI network card.
- Various virtio devices: Filesystem, network and balloon.
- A SoundBlaster 16 sound card.
- A hayes-compatible dial-up Modem.

---

## Built-in firmware

v86 ships with its own BIOS and VGA BIOS, so **no ROM image is needed to
boot**. They are MIT-licensed and replace the SeaBIOS/SeaVGABIOS pair the
upstream project fetches separately.

```javascript
new V86({ firmware: "pcjs", fda: { url: "freedos.img" } });
```

Leave `firmware` out and v86 behaves exactly as before, loading
`bios/seabios.bin` and `bios/vgabios.bin`. The two paths can be run against
each other to tell a firmware bug from a guest bug — see
[Firmware examples](#firmware-examples).

### Why it exists

Every ROM in this space is either copyleft or, in one case, unlicensed, and
the one permissive option (PCjs's) covers only its *code*, not the IBM ROM
dumps in its repository. The analysis, with licence text for every candidate,
is in [`docs/firmware-selection.md`](docs/firmware-selection.md). In short:

| Candidate | Verdict |
|---|---|
| SeaBIOS / Bochs VGABIOS | GPL-2.0-or-later / LGPL-3.0 |
| `640-KB/GLaBIOS` | GPL-3.0 |
| `fysnet/i440fx` | No `LICENSE` file; also needs USB and SATA, which v86 lacks |
| PCjs ROM images | PCjs's MIT grant excludes third-party images |
| `b-dmitry1/BIOS` | MIT, but far too minimal for a modern guest |
| OVMF / EDK II | BSD-2-Clause-Patent, but UEFI only |

The firmware is modelled on PCjs's *methods* (MIT) and implemented from
public specifications.

### How it works

The system BIOS is a real 64 KiB ROM at `F000:0000`; the VGA BIOS is a
32 KiB option ROM at `C000:0000`. Both are assembled from Rust source at
build time. They are thin by design — 168 of 65536 bytes are non-fill —
because the services themselves run on the host and the ROM holds only a
reset vector, a POST entry, and a stub per interrupt:

```asm
push 0x0010      ; service id
int  0x66        ; serviced on the host
iret
```

The trap is honoured only when the return address is inside a firmware ROM
window, so **guests are free to use vector `0x66` themselves**.

### Services

| Vector | | Vector | |
|---|---|---|---|
| `08h` | timer tick | `13h` | disk: CHS, **EDD/LBA, El Torito** |
| `09h` | keyboard | `14h` | serial |
| `0Eh` | diskette | `15h` | system: **E820/E801**, A20, config table |
| `70h` | real-time clock | `16h` | keyboard |
| `74h` | PS/2 mouse | `17h` | printer |
| `10h` | video, incl. **VBE 2.0+** | `19h` | bootstrap |
| `11h` | equipment list | `1Ah` | RTC/CMOS |
| `12h` | memory size | `43h` | font request |

IRQ 0, 1, 6, 8 and 12 are wired to the timers, keyboard and diskette. POST
fills the BDA and the drive table, and publishes a conventional-memory size,
an equipment word and a system memory map.

### Disks and CD-ROMs

Hard disks and CD-ROMs are registered with the firmware and are not copied
into it. v86 hands images over as JavaScript buffers that Rust cannot
address, so a 640 MiB ISO would otherwise mean growing the wasm heap by most
of a gigabyte; instead the firmware asks the host for each sector as it needs
it.

```javascript
new V86({ firmware: "pcjs", cdrom: { url: "debian.iso" } });   // El Torito
new V86({ firmware: "pcjs", hda:   { url: "disk.img" } });     // MBR
```

El Torito boot works, including no-emulation images. Two 32-bit images known
to work in v86 are useful test subjects, both from
[v86's Advent calendar](https://copy.sh/v86/advent/2023/):

```console
$ curl -O https://i.copy.sh/FreeNOS-1.0.3.iso        # 10.5 MiB, no-emulation
$ curl -O https://i.copy.sh/HelenOS-0.11.2-ia32.iso  # 24.6 MiB, ia32
$ FW_DEVICE=cdrom node examples/cd-boot.js FreeNOS-1.0.3.iso
```

FreeNOS reaches its `login:` prompt under SeaBIOS but **panics under the
built-in firmware** — a real defect at the boot handoff for protected-mode
guests, not a CD problem (BOOT-2 in [`TechDebt.md`](TechDebt.md)). Debian's
and NetBSD's installers do not complete boot under either BIOS (BOOT-1).

### What is not finished

Worth reading before relying on it:

- **No `INT 13h AX=4B00h/4B02h`**, so a loader cannot boot from CD after POST
  has run; only POST's own CD boot works.
- **No loadable option ROMs**, so a driver cannot be guest-visible code.
- **No USB**, in the emulator or the BIOS.
- **No PCI BIOS, APIC or ACPI tables.**

[`TechDebt.md`](TechDebt.md) is the full register, with file references.

---

## Demos

[9front](https://copy.sh/v86/?profile=9front) —
[Arch Linux](https://copy.sh/v86/?profile=archlinux) —
[Android-x86 1.6-r2](https://copy.sh/v86?profile=android) —
[Android-x86 4.4-r2](https://copy.sh/v86?profile=android4) —
[BasicLinux](https://copy.sh/v86/?profile=basiclinux) —
[Buildroot Linux](https://copy.sh/v86/?profile=buildroot) —
[Damn Small Linux](https://copy.sh/v86/?profile=dsl) —
[ELKS](https://copy.sh/v86/?profile=elks) —
[FreeDOS](https://copy.sh/v86/?profile=freedos) —
[FreeBSD](https://copy.sh/v86/?profile=freebsd) —
[FiwixOS](https://copy.sh/v86/?profile=fiwix) —
[Haiku](https://copy.sh/v86/?profile=haiku) —
[SkiffOS](https://copy.sh/v86/?profile=copy/skiffos) —
[ReactOS](https://copy.sh/v86/?profile=reactos) —
[Windows 2000](https://copy.sh/v86/?profile=windows2000) —
[Windows 98](https://copy.sh/v86/?profile=windows98) —
[Windows 95](https://copy.sh/v86/?profile=windows95) —
[Windows 1.01](https://copy.sh/v86/?profile=windows1) —
[MS-DOS 6.22](https://copy.sh/v86/?profile=msdos) —
[OpenBSD](https://copy.sh/v86/?profile=openbsd) —
[Oberon](https://copy.sh/v86/?profile=oberon) —
[KolibriOS](https://copy.sh/v86/?profile=kolibrios) —
[SkiftOS](https://copy.sh/v86?profile=skift) —
[QNX](https://copy.sh/v86?profile=qnx)

## Documentation

[How it works](docs/how-it-works.md) —
**Firmware selection** ([docs/firmware-selection.md](docs/firmware-selection.md)) —
**Changes** ([Changes.md](Changes.md)) —
**Tech debt** ([TechDebt.md](TechDebt.md)) —
[Networking](docs/networking.md) —
[Dial-up modem networking](docs/modem.md) —
[Alpine Linux guest setup](tools/docker/alpine/) —
[Arch Linux guest setup](docs/archlinux.md) —
[Debian with xfce guest setup](tools/docker/debian/) —
[MS-DOS/FreeDOS guest setup](docs/dos.md) —
[Windows 3.1x guest setup](docs/windows-31x.md) —
[Windows 9x guest setup](docs/windows-9x.md) —
[Windows NT guest setup](docs/windows-nt.md) —
[9p filesystem](docs/filesystem.md) —
[Linux rootfs on 9p](docs/linux-9p-image.md) —
[Profiling](docs/profiling.md)

## Compatibility

Here's an overview of the operating systems supported in v86:

**Note that, since v0.5.0, the recommended way to access v86 in a browser is
via the [WebAssembly build](#how-to-build-run-and-embed).**

MS-DOS 6.22, Windows 1.01, Windows 3.11, Windows 95, Windows 98, Windows
2000, Windows XP, Windows Vista, Windows 7, Windows 8, Windows 10, ReactOS,
FreeDOS, FreeBSD, NetBSD, OpenBSD, KolibriOS, Linux (many distributions),
Arch Linux, Android x86, Haiku, Plan 9, Fiwix, Oberon, Lodepun, HelenOS,
Demand paging, SkiftOS.

You can get some information on the disk images here: https://github.com/copy/images.

## How to build, run and embed?

You need:

- make
- Rust with the wasm32-unknown-unknown target
- A version of clang compatible with Rust
- java (for Closure Compiler, not necessary when using `debug.html`)
- nodejs (a recent version is required, v24.16 is known to be working)
- To run tests: nasm, gdb, qemu-system, gcc, libc-i386 and rustfmt

See [tools/docker/test-image/Dockerfile](tools/docker/test-image/Dockerfile)
for a full setup on Debian or
[WSL](https://docs.microsoft.com/en-us/windows/wsl/install).

- Run `make` to build the debug build (at `debug.html`).
- Run `make all` to build the optimized build (at `index.html`).
- ROM and disk images are loaded via XHR, so if you want to try out `index.html`
  locally, make sure to serve it from a local webserver. You can use `make run`
  to serve the files using Python's http module.
- If you only want to embed v86 in a webpage you can use `libv86.js`. For usage,
  check out the [examples](examples/). You can download it from the [release section](https://github.com/copy/v86/releases).
- For bundler-based setups (Vite/React/Next/Webpack), there is also an official npm package:
  https://www.npmjs.com/package/v86

  This package was originally maintained by [@giulioz](https://github.com/giulioz) (bundler-optimized fork) and was made "official" for this repo by [@basicer](https://github.com/basicer) with the author's permission.
  It is published automatically from this repository via GitHub Actions ([.github/workflows/ci.yml](.github/workflows/ci.yml), Upload release job) on pushes to `master` and uses `npm publish --provenance`.

  Install: `npm install v86`

The built-in firmware adds no build step: `make` and `make all` include it,
and `libv86.js` contains it. Fetching `bios/seabios.bin` is no longer
required unless you leave `firmware` unset.

### Alternatively, to build using Docker

- If you have Docker installed, you can run the whole system inside a container.
- See `tools/docker/exec` to find the Dockerfile required for this.
- You can run `docker build -f tools/docker/exec/Dockerfile -t v86:alpine-3.19 .` from the root directory to generate docker image.
- Then you can simply run `docker run -it -p 8000:8000 v86:alpine-3.19` to start the server.
- Check `localhost:8000` for hosted server.

### Running via Dev Container

- If you are using an IDE that supports Dev Containers, such as GitHub Codespaces, the Visual Studio Code Remote Container extension, or possibly others such as Jetbrains' IntelliJ IDEA, you can setup the development environment in a Dev Container.
- Follow the instructions from your development environment to setup the container.
- Run the Task "Fetch images" in order to download images for testing.

## Testing

The disk images for testing are not included in this repository. You can
download them directly from the website using:

`mkdir -p images && curl --compressed --output-dir images/ --remote-name-all https://i.copy.sh/{linux.iso,linux3.iso,linux4.iso,buildroot-bzimage68.bin,TinyCore-11.0.iso,oberon.img,msdos.img,openbsd-floppy.img,kolibri.img,windows101.img,os8.img,freedos722.img,mobius-fd-release5.img,msdos622.img}`

Run integration tests: `make tests`

Run all tests: `make jshint rustfmt kvm-unit-test nasmtests nasmtests-force-jit expect-tests jitpagingtests qemutests rust-test tests`

See [tests/Readme.md](tests/Readme.md) for more information.

### Firmware tests

The firmware has its own tests, none of which need a disk image:

```console
$ make firmware-unit-test      # 62 host-side tests, no wasm toolchain needed
$ node examples/firmware.js    # end-to-end boot test; exit 0 on success
```

See [Firmware examples](#firmware-examples).

## API examples

- [Basic](examples/basic.html)
- [Programmatically using the serial terminal](examples/serial.html)
- [A Lua interpreter](examples/lua.html)
- [Two instances in one window](examples/two_instances.html)
- [Networking between browser windows/tabs using the Broadcast Channel API](examples/broadcast-network.html)
- [TCP Terminal (fetch-based networking)](examples/tcp_terminal.html)
- [Saving and restoring emulator state](examples/save_restore.html)

Using v86 for your own purposes is as easy as:

```javascript
var emulator = new V86({
    screen_container: document.getElementById("screen_container"),
    bios: { url: "bios/seabios.bin" },
    vga_bios: { url: "bios/vgabios.bin" },
    hda: { url: "freedos.img" },
    autostart: true,
});
```

With the built-in firmware, drop the two ROM lines:

```javascript
var emulator = new V86({
    screen_container: document.getElementById("screen_container"),
    firmware: "pcjs",
    hda: { url: "freedos.img" },
    autostart: true,
});
```

### Firmware options

| Option | Default | Meaning |
|---|---|---|
| `firmware` | unset | `"pcjs"` selects the built-in firmware. Unset keeps loading `bios` / `vga_bios` as an image. |
| `firmware_trace` | `false` | Record the firmware's internal trace log, readable with `cpu.firmware_trace_ptr()` / `_len()`. Costs a little. |

### Firmware examples

Four Node scripts, all runnable in a clean checkout with no disk images.
`FW_ORACLE=seabios` switches the last two to a real BIOS from `bios/`.

| Script | What it does |
|---|---|
| [`firmware.js`](examples/firmware.js) | Boots a floppy on the built-in firmware and asserts the guest printed `RESULT: PASS`. Pass a `.img` path to boot something real: `node examples/firmware.js freedos.img`. |
| [`firmware-debug.js`](examples/firmware-debug.js) | Dumps the state a failure leaves behind: reset vector, both ROM images, the IVT, the BDA, the trap ring, the firmware's trace log and the guest screen. |
| [`firmware-oracle.js`](examples/firmware-oracle.js) | Boots the same boot sector against SeaBIOS or Bochs, so a difference between the two runs is a firmware difference rather than a guest bug. Needs `FW_NO_JIT=1`. Divergence is expected and explained on stderr — see below. |
| [`firmware-service-probe.mjs`](examples/firmware-service-probe.mjs) | Calls one service, records the registers a BIOS returns, and prints them. Settles a disagreement about a specification by measurement. Needs `FW_NO_JIT=1` against a real BIOS. |
| [`cd-boot.js`](examples/cd-boot.js) | Boots a disk or CD image, reporting which structures it found first so a failure says which path was expected. |
| [`build-test-iso.mjs`](examples/build-test-iso.mjs) | Builds a 21-sector El Torito ISO around the self-test boot sector, giving the CD path a controlled subject. |

```console
$ node examples/build-test-iso.mjs test-boot.iso
$ FW_DEVICE=cdrom node examples/cd-boot.js test-boot.iso
WorkAgent
v86 permissive firmware
Memory: 32768 KB
Drives: 1 cd
ERESULT: FAIL
```

`ERESULT: FAIL` is the expected verdict there: the self-test's check **E**
wants a floppy, and this boot has only a CD. Check `D` is likewise stricter
than SeaBIOS, which refuses EDD on floppies entirely.

#### Serving images over HTTP

`tools/httpfs-v86-server.py` serves a directory over plain HTTP, reading the
bytes through an [httpfs](https://github.com/httpfs/httpfs) server. The
httpfs server speaks a JSON RPC that v86 cannot fetch, so the bridge
translates it and adds `Range` support; the bytes themselves come from
httpfs.

```console
$ python -m httpfs.server 8099 /path/to/images/
$ tools/httpfs-v86-server.py --port 8100
$ node examples/cd-boot.js http://127.0.0.1:8100/debian-12.1.0-i386-netinst.iso
```

```console
$ node examples/firmware.js
WorkAgent
v86 permissive firmware
Memory: 32768 KB
Drives: 1 floppy
Booting from floppy...
QRESULT: PASS

$ FW_NO_JIT=1 node examples/firmware.js      # same, without the JIT
$ FW_TRACE=1 node examples/firmware-debug.js  # firmware's own trace log
$ FW_NO_JIT=1 FW_ORACLE=seabios node examples/firmware-service-probe.mjs
int13-edd-install-check   CF=1  AX=0x0100  BX=0x55AA  CX=0x0000  DX=0x0000
```

`FW_NO_JIT=1` is the first thing to try when something misbehaves: it tells
a firmware bug from a translator bug.

The oracle scripts **need** `FW_NO_JIT=1`. SeaBIOS does not boot under the
v86 JIT — it dies with `RuntimeError: table index is out of bounds` before the
guest runs a single instruction, so the crash is in the translator rather than
in anything the BIOS does. The built-in firmware has no such problem. This is
tracked as JIT-1 in [`TechDebt.md`](TechDebt.md).

The self-test boot sector is assembled in
[`firmware-selftest.mjs`](examples/firmware-selftest.mjs) — no nasm, no
disk image, nothing to download. It checks INT 10h, 11h, 12h, 13h, 15h and
1Ah, and prints the letter of the first check that fails, so the screen
says exactly which service misbehaved.

#### Expect divergence from a real BIOS

The built-in firmware passes every check. SeaBIOS and the Bochs BIOS do
not, and that is not a bug in the guest program — the self-test demands two
services a reference BIOS need not provide. `firmware-oracle.js` explains
which, after the output:

| Check | Reference BIOS | Built-in |
|---|---|---|
| `D` — `INT 13h AH=41h` on a **floppy** | Refuses, leaves `BX=55AAh` | Accepts: we really do do EDD reads on floppies |
| `T` — `INT 1Ah AH=02h` | Returns a day of `0` | Correct date |

The second is a v86 gap rather than a firmware one: `src/rtc.js` does not
populate the CMOS time registers the way real hardware would, so a BIOS that
reads CMOS directly gets garbage. Tracked as RTC-1 in
[`TechDebt.md`](TechDebt.md).

The reference BIOSes under `bios/` are GPL-2.0-or-later and LGPL-3.0. They
are used as a behavioural oracle only — queried, never copied into this
tree. See [`bios/README.md`](bios/README.md).

## Generative AI

This project is partially developed with the help of LLMs.

## License

Copyright (c) 2012-24 Fabian Hemmer and contributors

The software in this repository is licensed under the
[MIT License](LICENSE.MIT).

The PCjs-derived firmware methods are MIT; see
[THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).

### Required attribution

If you ship a host that runs the built-in firmware, PCjs's MIT terms require
you to display:

> PCjs © 2012-2026 Jeff Parsons —
> <https://www.pcjs.org> · <https://github.com/jeffpar/pcjs>

on every page that runs it. This is a condition of using the firmware, not
a courtesy; see [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md) §
*Attribution obligations for the host UI*.

Any BIOS images in `bios/` are optional, separately fetched assets under
their own GPL/LGPL terms, and are not part of the MIT-licensed build.

## Credits

v86 is a rewrite of [copy.sh](https://copy.sh/), by
[Fabian Hemmer](https://github.com/copy) and contributors, with
[help](https://github.com/copy/v86/graphs/contributors) from others.

Listed alphabetically: [9292b](https://github.com/9292b),
[CalvinNeo](https://github.com/CalvinNeo),
[cryptonomicon](https://github.com/cryptonomicon),
[dhm](https://github.com/dhm),
[hNEuN2p7z](https://github.com/hNEuN2p7z),
[kcwikizh](https://github.com/kcwikizh),
[mrdoob](https://github.com/mrdoob),
[nenadpalic](https://github.com/nenadpalic),
[pfspari](https://github.com/pfspari),
[pronav](https://github.com/pronav),
[RReverser](https://github.com/RReverser),
[schellingb](https://github.com/schellingb),
[t anders ](https://github.com/tanders),
[winter-2012](https://github.com/winter-2012).

## More questions?

- Chat: [#v86 on irc.libera.chat](https://libera.chat/).
- Q&A: https://github.com/copy/v86/discussions

The original upstream README is preserved at
[`Readme-orig.md`](Readme-orig.md).