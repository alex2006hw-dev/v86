// Browser smoke test for the built-in firmware.
//
// Open examples/firmware.html from a web server -- the examples load the
// emulator over HTTP, so a file:// URL will not do:
//
//     $ python -m http.server 8000
//     $ open http://localhost:8000/examples/firmware.html
//
// This is the browser-side mirror of `node examples/firmware.js`: it boots
// the same self-test boot sector and watches for `RESULT: PASS`. Nothing
// has to be downloaded and no ROM image is fetched -- the firmware is
// inside v86.wasm, and the boot sector is assembled in the page.
//
// The point of the page is the dropdown. The same sector can be booted
// against SeaBIOS and the Bochs BIOS, exactly as the SeaBIOS-era
// examples (basic.html and friends) do, so a difference between the rows
// is a firmware difference rather than a guest bug. That is the same
// differential the Node scripts run against, brought to the browser.
//
// One constraint carries over from the Node side: the reference BIOSes
// do not survive the v86 JIT (TechDebt JIT-1 -- they die with
// `table index is out of bounds` before the guest runs a single
// instruction), so those rows run with the JIT switched off. The
// built-in firmware has no such problem and runs with it on.
//
// eslint does not look inside HTML, so the logic lives here where it is
// checked.

import { V86 } from "../build/libv86.js";
import { build_self_test_floppy } from "./firmware-selftest.mjs";

// The self-test halts in its own spin loop once it has printed, so a
// bounded poll is enough and needs no keyboard.
const SECONDS = 30;
const POLL_MS = 200;

const screen_container = document.getElementById("screen_container");
const verdict = document.getElementById("verdict");
const details = document.getElementById("details");
const choice = document.getElementById("firmware_choice");

let emulator = null;
let poll_timer = null;
let started_at = 0;

/**
 * A fresh screen container per boot.
 *
 * The ScreenAdapter appends its own nodes, and destroying an emulator
 * leaves them behind; a stale canvas would show the previous run's
 * contents and make a reboot look like it worked.
 *
 * @return {Node}
 */
function build_screen()
{
    screen_container.textContent = "";

    const row = document.createElement("div");
    row.style.whiteSpace = "pre";
    row.style.font = "14px monospace";
    row.style.lineHeight = "14px";

    const canvas = document.createElement("canvas");
    canvas.style.display = "none";

    screen_container.appendChild(row);
    screen_container.appendChild(canvas);

    return screen_container;
}

/**
 * What the dropdown picked, turned into v86 settings.
 *
 * @param {string} selected
 * @return {object}
 */
function settings_for(selected)
{
    const settings = {
        wasm_path: "../build/v86.wasm",
        memory_size: 32 * 1024 * 1024,
        vga_memory_size: 2 * 1024 * 1024,
        screen_container: build_screen(),
        // The self-test floppy is built in the browser, so this page
        // runs in a clean checkout with no image to download.
        fda: { buffer: build_self_test_floppy() },
        autostart: true,
    };

    if(selected === "pcjs")
    {
        // The built-in firmware brings its own ROMs, so no image is
        // fetched and `bios`/`vga_bios` are cleared rather than left
        // undefined by accident.
        settings.firmware = "pcjs";
        settings.bios = undefined;
        settings.vga_bios = undefined;
    }
    else if(selected === "seabios")
    {
        settings.disable_jit = true;         // JIT-1
        settings.bios = { url: "../bios/seabios.bin" };
        settings.vga_bios = { url: "../bios/vgabios.bin" };
    }
    else
    {
        settings.disable_jit = true;
        settings.bios = { url: "../bios/bochs-bios.bin" };
        settings.vga_bios = { url: "../bios/bochs-vgabios.bin" };
    }

    return settings;
}

/**
 * Tear the previous machine down, if there is one.
 */
function stop()
{
    if(poll_timer)
    {
        clearInterval(poll_timer);
        poll_timer = null;
    }

    if(emulator)
    {
        emulator.destroy();
        emulator = null;
    }
}

function hex(value, digits)
{
    return (value >>> 0).toString(16).toUpperCase().padStart(digits, "0");
}

/**
 * What the run can report once the guest has been given long enough.
 *
 * Everything here is read through the public CPU surface the Node
 * scripts use, so a browser failure looks the same as a headless one.
 * Reads are guarded: `emulator.v86` appears only after the wasm module
 * has been instantiated, and a timeout can fire before that.
 *
 * @param {string} screen
 * @return {string}
 */
function describe(screen)
{
    const lines = [];

    try
    {
        const cpu = emulator.v86.cpu;
        const mem = cpu.mem8;

        lines.push("halted at            cs:eip = " + hex(cpu.sreg[1]) + ":" + hex(cpu.get_real_eip()));
        lines.push("boot sector at 7C00  " + hex(mem[0x7C00] | (mem[0x7C01] << 8) | (mem[0x7C02] << 16) | (mem[0x7C03] << 24), 8) +
            "   signature " + hex(mem[0x7DFE], 2) + hex(mem[0x7DFF], 2));

        // Only meaningful with the built-in firmware: it writes its own
        // ROMs into guest memory, so the reset vector is readable from
        // `mem8`. The reference BIOSes are mapped rather than copied.
        lines.push("reset vector FFFF0   " + hex(mem[0xFFFF0], 2) + " " + hex(mem[0xFFFF1], 2) +
            " " + hex(mem[0xFFFF2], 2) + " " + hex(mem[0xFFFF3], 2) + " " + hex(mem[0xFFFF4], 2));

        const present = cpu.firmware_present();

        lines.push("built-in firmware    " + (present ? "installed" : "not installed") +
            "   traps " + cpu.firmware_trap_count() +
            "   last service " + (cpu.firmware_last_service() === 0xFFFF
                ? "none" : "0x" + hex(cpu.firmware_last_service())));
    }
    catch(e)
    {
        lines.push("(state not readable: " + String(e.message).split("\n")[0] + ")");
    }

    lines.push("");
    lines.push("--- guest screen ---");
    lines.push(screen);

    return lines.join("\n");
}

/**
 * Boot the sector and watch the screen for the verdict.
 */
function boot()
{
    stop();

    const selected = choice.value;
    const settings = settings_for(selected);

    verdict.textContent = "booting " + selected + " ...";
    verdict.className = "pending";
    details.textContent = "";
    started_at = Date.now();

    emulator = new V86(settings);

    poll_timer = setInterval(function()
    {
        let screen = "";

        try
        {
            screen = emulator.screen_adapter.get_text_screen()
                .map(row => row.replace(/\s+$/, ""))
                .filter(row => row.length)
                .join("\n");
        }
        catch(e)
        {
            // The adapter may not exist yet while start-up is in progress.
        }

        if(/RESULT:/.test(screen))
        {
            const match = (screen.match(/RESULT: \w+/) || [null])[0];

            if(match === "RESULT: PASS")
            {
                verdict.textContent = "PASS -- " + selected + " served every BIOS service";
                verdict.className = "pass";
            }
            else
            {
                verdict.textContent = match + " -- see the screen below";
                verdict.className = "fail";
            }

            stop();
            details.textContent = describe(screen);
            return;
        }

        if(Date.now() - started_at > SECONDS * 1000)
        {
            verdict.textContent = "TIMEOUT after " + SECONDS + "s -- the guest never printed a verdict";
            verdict.className = "fail";
            stop();
            details.textContent = describe(screen);
        }
    }, POLL_MS);
}

choice.addEventListener("change", boot);
document.getElementById("boot_button").addEventListener("click", boot);

// Boot once on load so the page is useful without touching anything.
boot();
