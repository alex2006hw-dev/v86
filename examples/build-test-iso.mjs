// Build a minimal bootable El Torito CD image.
//
//   node examples/build-test-iso.mjs <out.iso>
//
// The ISOs in circulation are awkward test subjects: a hybrid image boots
// through its MBR rather than El Torito, an El Torito image has a catalogue
// its boot loader may or may not respect, and a 640 MiB one takes twenty
// seconds to fetch before anything is proven. This builds the smallest
// image that exercises exactly one thing -- the El Torito no-emulation boot
// path -- so a failure has one possible cause.
//
// The image carries the same self-test boot sector as
// `firmware-selftest.mjs`, so it prints `RESULT: PASS` when the firmware
// does the right thing and names the failing check when it does not.
//
// Written by hand rather than with xorriso so that `node examples/firmware.js`
// works in a clean checkout with no ISO tooling installed. Only the
// structures El Torito and ISO 9660 require for booting are present; there is
// no filesystem, because nothing reads one.

import fs from "node:fs";
import { build_self_test_floppy } from "./firmware-selftest.mjs";

const SECTOR = 2048;

// ISO 9660 volume descriptor types.
const VD_BOOT_RECORD = 0;
const VD_PRIMARY = 1;
const VD_TERMINATOR = 0xFF;

const CATALOG_LBA = 19;
const IMAGE_LBA = 20;

/** A volume descriptor: type, then the shared `CD001` header. */
function descriptor(type, identifier)
{
    const d = Buffer.alloc(SECTOR, 0);
    d[0] = type;
    d.write("CD001", 1, "latin1");
    d[6] = 1; // version
    identifier.copy(d, 8, 0, Math.min(identifier.length, 32));
    return d;
}

/** Pad the identifier field with spaces, as the spec wants. */
function pad(text, length)
{
    return Buffer.from(text.padEnd(length, " "), "latin1");
}

/**
 * The El Torito boot catalogue: one validation entry and one initial/default
 * entry, both in a sector that begins with the "EL TORITO SPECIFICATION"
 * signature.
 */
function build_catalog()
{
    const c = Buffer.alloc(SECTOR, 0);

    // The catalogue sector opens with the validation entry directly -- no
    // signature in front of it. Debian's isohybrid boot info block is laid out
    // this way, and so is this firmware's expectation. Everything not set
    // below stays zero, identifier string included: a stray character there
    // changes the checksum and the entry is refused.
    c[0] = 0x01;                       // header id
    c[1] = 0x00;                       // platform: 80x86
    c[30] = 0x55;                      // key bytes
    c[31] = 0xAA;

    // The sixteen little-endian words must sum to zero. Word 0 is the header
    // id (1) and word 15 is 0xAA55 (43605), so word 7 supplies
    // 65536 - 43605 - 1 = 21930, which is 0x55AA.
    c[14] = 0xAA;
    c[15] = 0x55;

    const e = c.subarray(32, 64);
    e[0] = 0x88;                       // boot indicator
    e[1] = 0x00;                       // media: no emulation
    e.writeUInt16LE(0x07C0, 2);        // load segment
    e.writeUInt16LE(1, 6);             // virtual sector count
    e.writeUInt32LE(IMAGE_LBA, 8);     // load RBA

    return c;
}

/** The boot record volume descriptor, which points at the catalogue. */
function build_boot_record()
{
    const d = descriptor(VD_BOOT_RECORD, pad("", 32));
    d.write("EL TORITO SPECIFICATION", 7, "latin1");
    // Two layouts, because the sources disagree and a test image has to
    // satisfy whichever one the BIOS under test believes.
    //
    // 0x47: the boot catalogue LBA. This is where Debian's and NetBSD's
    //       images put it -- both have a plausible LBA there -- and it is
    //       where this firmware looks.
    //
    // 95:   the offsets the ISO 9660 / El Torito prose quotes, laid out as
    //       the `struct boot_sector` SeaBIOS uses.
    //
    // Writing both costs 40 bytes of an otherwise empty sector.
    d.writeUInt32LE(CATALOG_LBA, 0x47);
    d[0x4B] = 0x00;                    // media type: no emulation

    d[95] = 0x88;                      // boot indicator: bootable
    d[96] = 0x00;                      // boot media type: no emulation
    d.writeUInt16LE(0x07C0, 97);       // load segment
    d[99] = 0x00;                      // system type
    d.writeUInt16LE(1, 101);           // sector count
    d.writeUInt32LE(IMAGE_LBA, 103);   // load RBA
    d.writeUInt32LE(CATALOG_LBA, 107); // boot catalogue LBA
    return d;
}

/** A primary volume descriptor, for tools that insist on one existing. */
function build_primary()
{
    const d = descriptor(VD_PRIMARY, pad("V86TEST", 32));
    return d;
}

export function build_test_iso()
{
    const image = Buffer.alloc((IMAGE_LBA + 1) * SECTOR, 0);

    build_primary().copy(image, 16 * SECTOR);
    build_boot_record().copy(image, 17 * SECTOR);
    descriptor(VD_TERMINATOR, pad("", 32)).copy(image, 18 * SECTOR);
    build_catalog().copy(image, CATALOG_LBA * SECTOR);

    // No-emulation mode hands the loader whole 2048-byte CD sectors and lets
    // it read 512-byte "virtual" sectors from them, so the first 512 bytes of
    // the image sector are the boot sector.
    const boot_sector = new Uint8Array(build_self_test_floppy()).subarray(0, SECTOR);
    Buffer.from(boot_sector.buffer).copy(image, IMAGE_LBA * SECTOR);

    return image;
}

if(process.argv[1] && process.argv[1].endsWith("build-test-iso.mjs"))
{
    const out = process.argv[2];

    if(!out)
    {
        console.error("usage: node examples/build-test-iso.mjs <out.iso>");
        process.exit(2);
    }

    const image = build_test_iso();
    fs.writeFileSync(out, image);

    console.log("wrote " + out);
    console.log("  sectors        : " + image.length / SECTOR);
    console.log("  catalog at LBA : " + CATALOG_LBA);
    console.log("  boot image LBA : " + IMAGE_LBA + " (no emulation)");
    console.log("  boot signature : " +
        image.readUInt16LE(IMAGE_LBA * SECTOR + 510).toString(16));
}
