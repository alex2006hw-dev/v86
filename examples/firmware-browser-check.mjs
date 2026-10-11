// Validate examples/firmware.html without a browser.
//
//   node examples/firmware-browser-check.mjs
//
// The page itself needs a real canvas and a web server, so this checks
// the two things that are checkable headlessly, both of which are real
// failure modes that would otherwise only surface in a browser:
//
//   1. the ids the script reads match the ids the page defines -- a typo
//      throws only at boot time, in the browser, with no stack pointing
//      anywhere useful;
//   2. the boot configuration the page builds actually boots the
//      self-test sector and reports PASS, which is the same run
//      `examples/firmware.js` performs, minus the screen container.
//
// What this cannot check is the DOM and canvas half: the ScreenAdapter
// needs a 2d context, so the page is opened in a browser to see the
// verdict rendered. The Node scripts cover the boot itself.
//
// Exit 0 if every check passes, 1 otherwise.

import fs from "node:fs";
import path from "node:path";
import url from "node:url";
import { V86 } from "../build/libv86.mjs";
import { build_self_test_floppy } from "./firmware-selftest.mjs";

const ROOT = path.resolve(path.dirname(url.fileURLToPath(import.meta.url)), "..");
const PAGE = path.join(ROOT, "examples/firmware.html");
const SCRIPT = path.join(ROOT, "examples/firmware-browser.mjs");

let failed = false;

function check(name, ok, detail)
{
    console.log((ok ? "ok   " : "FAIL ") + name + (ok || !detail ? "" : " -- " + detail));
    if(!ok)
    {
        failed = true;
    }
}

// ---- 1. the ids line up ---------------------------------------------------

const html = fs.readFileSync(PAGE, "utf8");
const source = fs.readFileSync(SCRIPT, "utf8");

// Every `id="x"` in the page, and every `getElementById("x")` in the
// script. An id in the page the script never reads is fine -- the
// `verdict` and `details` panels are written to, not read from -- but a
// missing one is a null reference at boot.
const html_ids = new Set([...html.matchAll(/id="([^"]+)"/g)].map(m => m[1]));
const wanted_ids = new Set([...source.matchAll(/getElementById\("([^"]+)"\)/g)].map(m => m[1]));

const missing = [...wanted_ids].filter(id => !html_ids.has(id));
check("firmware.html defines every id the script reads", missing.length === 0,
    missing.length ? "missing: " + missing.join(", ") : "");

console.log("     page ids  : " + [...html_ids].join(", "));
console.log("     script ids: " + [...wanted_ids].join(", "));

// ---- 2. the boot configuration --------------------------------------------
// The three firmware selections, and the fact that only the reference
// ones need the JIT off. The distinction is easy to lose in a later
// edit and it is the first thing to reach for when the page misbehaves.
check("page boots the built-in firmware with firmware: pcjs",
    /settings\.firmware = "pcjs"/.test(source) &&
    /value="pcjs"/.test(html));

const selects_rom = /settings\.bios = \{ url: "\.\.\/bios\/(seabios|bochs-bios)\.bin" \}/.test(source) &&
    /settings\.vga_bios = \{ url: "\.\.\/bios\/(vgabios|bochs-vgabios)\.bin" \}/.test(source) &&
    /value="seabios"/.test(html) && /value="bochs"/.test(html);
check("page can also boot SeaBIOS and Bochs from bios/", selects_rom);

check("the reference BIOSes run with the JIT disabled (JIT-1)",
    /settings\.disable_jit = true/.test(source));

// The boot sector the page passes in. If this threw there would be no
// boot at all, and the page reports it as a timeout rather than a
// failure.
let floppy = null;
try
{
    floppy = build_self_test_floppy();
}
catch(e)
{
    check("the self-test boot sector assembles", false, String(e.message).split("\n")[0]);
}
if(floppy)
{
    const bytes = new Uint8Array(floppy);
    check("the self-test boot sector assembles", bytes.length === 2880 * 512);
    check("the boot sector carries the 0xAA55 signature",
        bytes[510] === 0x55 && bytes[511] === 0xAA);
}

// ---- 3. that configuration boots -----------------------------------------
// The page's settings, minus `screen_container`: this run has no canvas.
const emulator = new V86({
    wasm_path: path.join(ROOT, "build/v86.wasm"),
    memory_size: 32 * 1024 * 1024,
    vga_memory_size: 2 * 1024 * 1024,
    firmware: "pcjs",
    bios: undefined,
    vga_bios: undefined,
    fda: floppy ? { buffer: floppy } : undefined,
    autostart: true,
});

function screen_text()
{
    const vga = emulator.v86.cpu.devices.vga;
    const mem = vga.vga_memory;
    const space = [0xA0000, 0xA0000, 0xB0000, 0xB8000][(vga.miscellaneous_graphics_register >> 2) & 3];
    const base = (0xB8000 - space) + ((vga.start_address || 0) << 1);
    const cols = vga.max_cols || 80;
    const rows = [];

    for(let row = 0; row < 25; row++)
    {
        let line = "";

        for(let col = 0; col < cols; col++)
        {
            line += String.fromCharCode(mem[base + (row * cols + col) * 2]);
        }

        rows.push(line.replace(/\s+$/, ""));
    }

    return rows.filter(line => line.length).join("\n");
}

const deadline = Date.now() + 30000;
const timer = setInterval(function()
{
    let text = "";

    try
    {
        text = screen_text();
    }
    catch(e)
    {
        // The VGA device may not exist yet while start-up is in progress.
    }

    if(/RESULT:/.test(text))
    {
        clearInterval(timer);
        const verdict = (text.match(/RESULT: \w+/) || [null])[0];
        console.log("");
        console.log("--- guest screen ---");
        console.log(text);
        console.log("--------------------");
        check("the page's configuration boots the self-test to " + verdict, verdict === "RESULT: PASS");
        emulator.destroy();
        console.log(failed ? "\nRESULT: FAIL -- the page would not boot in a browser"
                           : "\nRESULT: PASS -- the page is wired correctly");
        process.exit(failed ? 1 : 0);
    }

    if(Date.now() > deadline)
    {
        clearInterval(timer);
        console.error("timed out after 30s; the guest never printed a verdict");
        emulator.destroy();
        process.exit(1);
    }
}, 200);
