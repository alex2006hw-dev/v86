// Does the firmware serve the right bytes from a CD-ROM?
//
//   node examples/cd-read-check.mjs <image.iso> <lba> <sectors>
//
// Boots a boot sector that reads a CD-ROM with `INT 13h AH=42h` through a
// disk address packet, then stops. The host then compares guest memory
// against the same bytes in the image file.
//
// This is the check BOOT-2 needs. Every read trace so far proved only that a
// read returned *success* -- "-> 1" is not "-> the right bytes" -- and the
// guest that fails is a boot loader that reads itself through AH=42h before
// entering protected mode. If our CD path serves the wrong data while
// reporting success, that is the whole bug.
//
// It is also the cheapest possible form of the question: no boot loader, no
// protected mode, just one read and a comparison against the file on disk.

import fs from "node:fs";
import url from "node:url";
import { V86 } from "../build/libv86.mjs";
import { Asm } from "./firmware-selftest.mjs";

const SECTOR = 512;
const DATA_SEG = 0x2000;
const DAP = 0x0000;
const BUFFER_SEG = 0x0000;
const BUFFER_OFF = 0x0800;         // linear 0x800, well clear of the sector

// Which INT 13h drive number holds the CD. With a floppy attached the
// firmware numbers the floppy 0x00 and the CD-ROM takes the next free
// number in its 0x00-0x0F range, so it is 0x01.
const drive = Number(process.env.FW_DRIVE || 0x01);

const image_path = process.argv[2];
const lba = Number(process.argv[3] || 3800);
const sectors = Number(process.argv[4] || 4);
const DRIVE_DEFAULT = drive;

if(!image_path)
{
    console.error("usage: node examples/cd-read-check.mjs <image.iso> [lba] [sectors]");
    process.exit(2);
}

/**
 * A boot sector that performs exactly one EDD read and stops.
 *
 * The disk address packet is built in the scratch segment, the same way the
 * self-test does it, so the firmware sees a packet in guest memory rather
 * than something it built itself.
 */
function build_reader()
{
    const a = new Asm(SECTOR);

    a.label("start");
    a.cli();
    a.xor_ax_ax();
    a.mov_seg_ax(3);              // DS
    a.mov_seg_ax(0);              // ES
    a.mov_seg_ax(2);              // SS
    a.mov_r16(4, 0x7000);         // SP
    a.sti();

    a.mov_r16(0, DATA_SEG);
    a.mov_seg_ax(3);              // DS = scratch segment

    a.mov_moffs_imm16(DAP + 0x00, 16);           // packet size
    a.mov_moffs_imm16(DAP + 0x02, sectors);      // sector count
    a.mov_moffs_imm16(DAP + 0x04, BUFFER_OFF);    // buffer offset
    a.mov_moffs_imm16(DAP + 0x06, BUFFER_SEG);    // buffer segment
    a.mov_moffs_imm16(DAP + 0x08, lba & 0xFFFF);  // LBA, low word
    a.mov_moffs_imm16(DAP + 0x0A, (lba >> 16) & 0xFFFF);
    a.mov_moffs_imm16(DAP + 0x0C, 0);             // LBA, bits 32..63
    a.mov_moffs_imm16(DAP + 0x0E, 0);

    a.mov_ah(0x42);
    a.mov_dl(DRIVE_DEFAULT);
    a.mov_r16(6, DAP);           // SI = packet
    a.int(0x13);

    // Record the outcome so a failure is distinguishable from a hang.
    a.pushf();
    a.pop_ax();
    a.mov_moffs_reg(0x0C00, 0);  // flags, including carry

    a.label("done");
    a.cli();
    a.hlt();
    a.b(0xEB, 0xFD);

    const code = a.link();
    const image = new Uint8Array(2880 * SECTOR);
    image.set(code, 0);
    image[SECTOR - 2] = 0x55;
    image[SECTOR - 1] = 0xAA;
    return image.buffer;
}

const emulator = new V86({
    wasm_path: url.fileURLToPath(new URL("../build/v86.wasm", import.meta.url)),
    memory_size: 32 * 1024 * 1024,
    vga_memory_size: 2 * 1024 * 1024,
    disable_jit: !!process.env.FW_NO_JIT,
    firmware: "pcjs",
    bios: undefined,
    vga_bios: undefined,
    fda: { buffer: build_reader() },
    cdrom: { buffer: fs.readFileSync(image_path).buffer },
    autostart: true,
});

setTimeout(() =>
{
    const mem = emulator.v86.cpu.mem8;
    const flags = mem[0x0C00] | (mem[0x0C01] << 8);
    const carry = flags & 1;

    const want_len = sectors * 2048;
    const fd = fs.openSync(image_path, "r");
    const want = Buffer.alloc(want_len);
    fs.readSync(fd, want, 0, want_len, lba * 2048);
    fs.closeSync(fd);

    const got = Buffer.from(mem.subarray(BUFFER_OFF, BUFFER_OFF + want_len));

    let first_diff = -1;
    for(let i = 0; i < want_len; i++)
    {
        if(want[i] !== got[i])
        {
            first_diff = i;
            break;
        }
    }

    console.log("=== CD read check: " + image_path.split("/").pop() + " ===");
    console.log("  INT 13h AH=42h, DL=" + drive.toString(16).padStart(2, "0") +
        "h, LBA=" + lba + ", sectors=" + sectors);
    console.log("  carry on return : " + carry + (carry ? "  (read FAILED)" : "  (read reported success)"));
    console.log("  bytes compared : " + want_len);
    console.log("  first 16 in image : " + want.subarray(0, 16).toString("hex"));
    console.log("  first 16 in guest : " + got.subarray(0, 16).toString("hex"));

    if(carry)
    {
        console.log("  RESULT: FAIL -- the read reported an error");
    }
    else if(first_diff === -1)
    {
        console.log("  RESULT: PASS -- every byte matches the image");
    }
    else
    {
        console.log("  RESULT: FAIL -- first difference at byte +" + first_diff +
            " (want " + want[first_diff].toString(16) +
            ", got " + got[first_diff].toString(16) + ")");
        const nz = got.reduce((a, b) => a + (b ? 1 : 0), 0);
        console.log("           guest buffer has " + nz + "/" + got.length + " non-zero bytes");
    }

    emulator.destroy();
    process.exit(carry || first_diff !== -1 ? 1 : 0);
}, 4000);
