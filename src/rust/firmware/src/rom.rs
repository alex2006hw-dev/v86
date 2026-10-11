//! The firmware's ROM images.
//!
//! The guest-visible half of a BIOS is real code: the CPU resets at
//! `F000:FFF0`, runs POST, takes hardware interrupts through a real
//! IDT, and reaches every INT service through a vector a guest is free
//! to inspect, chain and replace. Only the leaf *work* lives on the
//! host. That split is what makes the firmware behave like firmware:
//! a program that hooks INT 9h gets the hook, a program that rebases
//! INT 13h keeps its handler, and IRQ 0 still ticks whether or not
//! anyone is looking.
//!
//! Two images are produced:
//!
//! * the 64 KiB system BIOS at `0xF0000` (segment `F000`), holding
//!   POST, the hardware-interrupt stubs, the INT-vector stubs and the
//!   reset vector;
//! * the 32 KiB video option ROM at `0xC0000` (segment `C000`),
//!   holding the video INT 10h/43h/1Ah stubs, the VBE information
//!   structures and the character generator.
//!
//! ## Reaching the host
//!
//! A stub does not call into the host directly. It pushes a service id
//! and executes [`TRAP_VECTOR`]:
//!
//! ```text
//!   int10:  push 0x0010
//!           int  0x66
//!           iret
//! ```
//!
//! The CPU notices the trap only when the return address it just
//! pushed lies inside one of the firmware's own ROM windows, so
//! guests are free to use vector 0x66 for themselves — the trap is
//! unreachable from anywhere else. See `crate::fw_adapter` on the v86
//! side for the check that enforces this.
//!
//! ## Unserviceable vectors
//!
//! INT 19h is the bootstrap: it loads a boot sector and sets CS:IP, so
//! it never returns. Its stub therefore ends in a spin rather than an
//! `iret`, because a return would pop the bootstrap's own frame and
//! resume at the power-on stack.

use crate::asm::{Asm, Reg, Seg};
use crate::font::FONT_8X16;

/// Physical base of the 64 KiB system BIOS, and its real-mode segment.
pub const SYSTEM_ROM_BASE: u32 = 0xF_0000;
pub const SYSTEM_ROM_SEG: u16 = 0xF000;
pub const SYSTEM_ROM_SIZE: usize = 0x1_0000;

/// Physical base of the 32 KiB video option ROM, and its segment.
pub const VGA_ROM_BASE: u32 = 0xC_0000;
pub const VGA_ROM_SEG: u16 = 0xC000;
pub const VGA_ROM_SIZE: usize = 0x8000;

/// The interrupt vector the ROM stubs use to reach the host services.
///
/// Chosen because it is unassigned in the IBM/PC-AT vector map and is
/// not used by DOS, DPMI or the Windows debug interfaces. The ROM
/// window check is what actually protects it, so the exact number
/// matters less than staying out of everyone's way.
pub const TRAP_VECTOR: u8 = 0x66;

/// Service id for "run POST". Not a real interrupt vector.
pub const SERVICE_POST: u16 = 0x00F0;

/// The vectors the system BIOS installs a stub for, in stub order.
///
/// Hardware vectors first, because a stub that is missing there
/// produces a hang rather than a wrong answer.
const SYSTEM_VECTORS: &[u8] = &[
    0x08, // timer tick (IRQ0)
    0x09, // keyboard (IRQ1)
    0x0A, // cascade (IRQ2)
    0x0B, // COM2 (IRQ3)
    0x0C, // COM1 (IRQ4)
    0x0D, // secondary controller
    0x0E, // diskette (IRQ6)
    0x0F, // FDC extension
    0x70, // real-time clock (IRQ8)
    0x74, // PS/2 mouse (IRQ12)
    0x10, // video
    0x11, // equipment list
    0x12, // conventional memory size
    0x13, // disk
    0x14, // serial
    0x15, // system services
    0x16, // keyboard
    0x17, // printer
    0x18, // BASIC
    0x19, // bootstrap
    0x1A, // timer
    0x1C, // user timer tick
];

/// Where things ended up in the system ROM, so the installer can point
/// the real IVT at them.
#[derive(Clone, Debug)]
pub struct SystemRomLayout {
    /// Offset of the POST entry point.
    pub post: u16,
    /// Offset of the `push id; int TRAP_VECTOR; iret` stub for a vector,
    /// or `NONE` for a vector with no stub.
    pub stubs: [u16; 256],
    /// Offset of the continuation an option ROM returns into, which
    /// jumps to the bootstrap.
    pub option_rom_return: u16,
}

/// Marks a vector that has no stub.
pub const NO_STUB: u16 = 0xFFFF;

impl SystemRomLayout {
    pub fn stub(&self, vector: u8) -> Option<u16> {
        let o = self.stubs[vector as usize];
        if o == NO_STUB {
            None
        } else {
            Some(o)
        }
    }
}

/// Fixed offsets of the VBE data inside the video option ROM.
///
/// The option ROM layout is a constant so that a host service can read
/// the structures straight out of guest memory without being handed a
/// layout from `build_vga_rom`.
pub mod vbe_rom {
    pub const INFO: u16 = 0x0100;
    pub const MODES: u16 = 0x0300;
    pub const STRINGS: u16 = 0x0400;
    pub const FONT: u16 = 0x0800;
}

impl VgaRomLayout {
    /// Offset of the VBE controller information block, as a constant.
    pub fn vbe_info_offset() -> u32 {
        u32::from(vbe_rom::INFO)
    }
}

/// Where things ended up in the video ROM.
#[derive(Copy, Clone, Debug)]
pub struct VgaRomLayout {
    /// Offset of the option ROM header's initialisation entry.
    pub init: u16,
    /// Offset of the INT 10h stub.
    pub int10: u16,
    /// Offset of the INT 43h (font) stub.
    pub int43: u16,
    /// Offset of the INT 1Ah stub.
    pub int1a: u16,
    /// Offset of the character generator.
    pub font: u16,
    /// Offset of the VBE 2.0/3.0 controller information block.
    pub vbe_info: u16,
    /// Offset of the VBE mode list (including its reserved word).
    pub vbe_modes: u16,
    /// Offset of the VBE OEM string block.
    pub vbe_strings: u16,
}

/// A built ROM image plus the map needed to wire it up.
pub struct SystemRom {
    pub image: Vec<u8>,
    pub layout: SystemRomLayout,
}

pub struct VgaRom {
    pub image: Vec<u8>,
    pub layout: VgaRomLayout,
}

/// Bytes reserved per stub. The longest stub is INT 19h's nine-byte
/// "trap then spin", so a power-of-two stride keeps every entry aligned.
const STUB_STRIDE: usize = 16;

/// Emit the three-instruction service stub `push id; int 0x66; iret`.
fn service_stub(a: &mut Asm, at: usize, service: u16, vector: u8) {
    a.at(at);
    a.label(&format!("stub_{:02x}", vector));
    a.push_imm(service);
    a.int(TRAP_VECTOR);
    a.iret();
}

/// Build the 64 KiB system BIOS image.
pub fn build_system_rom() -> SystemRom {
    let mut a = Asm::new(SYSTEM_ROM_SIZE, 0xFF);

    // ---- POST -------------------------------------------------------
    //
    // Real-mode power-on. The CPU has already zeroed the segment
    // registers in practice on some paths and not others, so establish
    // a known flat state before touching anything.
    a.at(0x0000);
    a.label("post");
    a.cli();
    a.xor_ax_ax();
    a.mov_seg_r(Seg::Ds, Reg::Ax);
    a.mov_seg_r(Seg::Es, Reg::Ax);
    a.mov_seg_r(Seg::Ss, Reg::Ax);
    // A stack clear of the low 640 KiB: high enough to leave the DOS
    // interrupt vector table and BDA alone, low enough that a
    // conventional-memory-size test still covers it.
    a.mov_r_imm(Reg::Sp, 0x7000);
    a.cld();
    // Ask the host to build the BDA, the drive table and the E820 map,
    // now that the segments are trustworthy.
    a.push_imm(SERVICE_POST);
    a.int(TRAP_VECTOR);
    a.sti();
    // INT 19h: the bootstrap loads a boot sector and sets CS:IP, so
    // control never comes back here.
    a.int(0x19);
    a.label("post_hang");
    a.cli();
    a.hlt();
    a.jmp_rel8("post_hang");

    // ---- service stubs ---------------------------------------------
    let mut stubs = [NO_STUB; 256];
    let mut cursor = 0x0100;
    for &vector in SYSTEM_VECTORS {
        // INT 19h never returns, so it must not IRET.
        if vector == 0x19 {
            a.at(cursor);
            a.label("stub_19");
            a.push_imm(0x0019);
            a.int(TRAP_VECTOR);
            // Unreachable: the bootstrap sets CS:IP to the loaded image.
            // The spin is a backstop for a host that declines to boot.
            a.label("boot_hang");
            a.cli();
            a.hlt();
            a.jmp_rel8("boot_hang");
        } else {
            service_stub(&mut a, cursor, vector as u16, vector);
        }
        stubs[vector as usize] = cursor as u16;
        cursor += STUB_STRIDE;
    }

    // ---- reset vector ----------------------------------------------
    //
    // A real BIOS puts a jump to POST here. The five bytes at F000:FFF5
    // are a second entry some warm-boot paths use, so they get the same
    // far jump into the middle of POST.
    a.at(0xFFF0);
    a.jmp_rel16("post");
    a.at(0xFFF5);
    a.emit(&[0xEA, 0x00, 0x00]);
    a.dw(SYSTEM_ROM_SEG);

    // ---- the return an option ROM lands on ---------------------------
    //
    // A ROM is entered with a far call, so a ROM that returns resumes
    // here. Everything POST has left to do is the bootstrap, and INT 19h
    // never comes back either, so a far jump to its stub is the whole
    // continuation. It has to be far, not near: a ROM may have left CS
    // naming anything.
    let option_rom_return = cursor as u16;
    a.at(cursor);
    a.emit(&[0xEA]); // jmp far segment:offset
    a.dw(stubs[0x19]);
    a.dw(SYSTEM_ROM_SEG);

    let layout = SystemRomLayout { post: 0x0000, stubs, option_rom_return };
    SystemRom {
        image: a.finish(),
        layout,
    }
}

/// Build the 32 KiB video option ROM image.
pub fn build_vga_rom() -> VgaRom {
    let mut a = Asm::new(VGA_ROM_SIZE, 0xFF);

    // ---- option ROM header ----------------------------------------
    //
    // Byte 0/1 are the 55 AA signature, bytes 2/3 the image length in
    // 512-byte blocks, and bytes 3/4 a far jump to the init entry that
    // a BIOS loader calls with a far return address.
    a.at(0x0000);
    a.label("header");
    a.db(0x55);
    a.db(0xAA);
    a.dw((VGA_ROM_SIZE / 512) as u16);
    a.jmp_rel16("init");

    // ---- init entry ------------------------------------------------
    a.at(0x0008);
    a.label("init");
    a.retf();

    // ---- service stubs ---------------------------------------------
    let (int10, int43, int1a) = (0x0010usize, 0x0020usize, 0x0030usize);
    service_stub(&mut a, int10, 0x0010, 0x10);
    service_stub(&mut a, int43, 0x0043, 0x43);
    service_stub(&mut a, int1a, 0x001A, 0x1A);

    // ---- VBE information block ------------------------------------
    let vbe_info = vbe_rom::INFO as usize;
    let vbe_modes = vbe_rom::MODES as usize;
    let vbe_strings = vbe_rom::STRINGS as usize;
    let font = vbe_rom::FONT as usize;

    crate::vbe::build_vbe_data(&mut a, vbe_info, vbe_modes, vbe_strings);

    // ---- character generator ---------------------------------------
    a.at(font);
    let flat: Vec<u8> = FONT_8X16.iter().flatten().copied().collect();
    assert_eq!(flat.len(), 4096);
    a.emit(&flat);

    let layout = VgaRomLayout {
        init: 0x0008,
        int10: int10 as u16,
        int43: int43 as u16,
        int1a: int1a as u16,
        font: font as u16,
        vbe_info: vbe_info as u16,
        vbe_modes: vbe_modes as u16,
        vbe_strings: vbe_strings as u16,
    };

    VgaRom {
        image: a.finish(),
        layout,
    }
}

/// True when `physical` lies inside a ROM window owned by the firmware.
///
/// The emulator consults this before honouring [`TRAP_VECTOR`], which
/// is what stops a guest from calling the host services directly.
pub fn is_firmware_rom(physical: u32) -> bool {
    let in_system = physical >= SYSTEM_ROM_BASE && physical < SYSTEM_ROM_BASE + SYSTEM_ROM_SIZE as u32;
    let in_vga = physical >= VGA_ROM_BASE && physical < VGA_ROM_BASE + VGA_ROM_SIZE as u32;
    in_system || in_vga
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Slice of the image at a segment:offset, where the segment is the
    /// base the ROM is loaded at.
    fn at_seg(rom: &[u8], base: u32, seg: u16, off: u16) -> &[u8] {
        let physical = (u32::from(seg) << 4) + u32::from(off);
        assert!(physical >= base, "{:X} is before the ROM base", physical);
        &rom[(physical - base) as usize..]
    }

    #[test]
    fn system_rom_reset_vector_jumps_to_post() {
        let rom = build_system_rom();
        let reset = at_seg(&rom.image, SYSTEM_ROM_BASE, SYSTEM_ROM_SEG, 0xFFF0);
        // E9 rel16
        assert_eq!(reset[0], 0xE9);
        let rel = i16::from_le_bytes([reset[1], reset[2]]);
        // The displacement is measured from the end of the jmp, which is
        // F000:FFF3; the target is POST at F000:0000.
        assert_eq!((0xFFF3i32 + i32::from(rel)) as u16, 0x0000);
    }

    #[test]
    fn system_rom_post_establishes_a_stack_then_bootstraps() {
        let rom = build_system_rom();
        let post = at_seg(&rom.image, SYSTEM_ROM_BASE, SYSTEM_ROM_SEG, 0);
        let hex: Vec<String> = post[..32].iter().map(|b| format!("{:02X}", b)).collect();
        // cli; xor ax,ax; mov ds,ax; mov es,ax; mov ss,ax; mov sp,7000; cld;
        assert_eq!(
            &hex[..16],
            &[
                "FA", "31", "C0", "8E", "D8", "8E", "C0", "8E", "D0", "BC", "00", "70", "FC", "68",
                "F0", "00"
            ]
        );
        // ...then int 0x66 (the POST service) and sti / int 0x19.
        assert_eq!(post[13], 0x68); // push imm16
        assert_eq!(post[14], 0xF0); // service id low byte
        assert_eq!(post[15], 0x00); // service id high byte
        assert_eq!(post[16], 0xCD);
        assert_eq!(post[17], TRAP_VECTOR);
        assert_eq!(post[18], 0xFB); // sti
        assert_eq!(post[19], 0xCD); // int 19h
        assert_eq!(post[20], 0x19);
        // And the backstop spin the ROM falls into if POST ever returns.
        assert_eq!(post[21], 0xFA); // cli
        assert_eq!(post[22], 0xF4); // hlt
    }

    #[test]
    fn every_service_vector_has_a_stub_that_traps_and_returns() {
        let rom = build_system_rom();
        for &vector in SYSTEM_VECTORS {
            let off = rom.layout.stub(vector).expect("stub installed");
            let code = at_seg(&rom.image, SYSTEM_ROM_BASE, SYSTEM_ROM_SEG, off);
            assert_eq!(code[0], 0x68, "vector {:02X} must push its service id", vector);
            assert_eq!(
                u16::from_le_bytes([code[1], code[2]]),
                u16::from(vector),
                "vector {:02X} service id",
                vector
            );
            assert_eq!(code[3], 0xCD);
            assert_eq!(code[4], TRAP_VECTOR);
            if vector == 0x19 {
                // The bootstrap must not return.
                assert_ne!(code[5], 0xCF, "INT 19h must not IRET");
            } else {
                assert_eq!(code[5], 0xCF, "vector {:02X} must IRET", vector);
            }
        }
    }

    #[test]
    fn vector_19_stub_spins_instead_of_returning() {
        let rom = build_system_rom();
        let off = rom.layout.stub(0x19).unwrap();
        let code = at_seg(&rom.image, SYSTEM_ROM_BASE, SYSTEM_ROM_SEG, off);
        // push 19; int 66; cli; hlt; jmp back to the cli
        assert_eq!(&code[..5], &[0x68, 0x19, 0x00, 0xCD, TRAP_VECTOR]);
        assert_eq!(code[5], 0xFA); // cli
        assert_eq!(code[6], 0xF4); // hlt
        assert_eq!(code[7], 0xEB); // jmp rel8
        // The jump returns to the cli two bytes before its own end.
        let rel = code[8] as i8;
        assert_eq!((off as i32 + 9 + i32::from(rel)) as u16, off + 5);
    }

    #[test]
    fn system_rom_is_exactly_64k() {
        assert_eq!(build_system_rom().image.len(), 0x1_0000);
    }

    #[test]
    fn vga_rom_has_a_valid_option_rom_header() {
        let rom = build_vga_rom();
        assert_eq!(&rom.image[0..2], &[0x55, 0xAA]);
        let blocks = u16::from_le_bytes([rom.image[2], rom.image[3]]);
        assert_eq!(blocks as usize * 512, VGA_ROM_SIZE);
        // The far jump the loader uses to reach init.
        assert_eq!(rom.image[4], 0xE9);
        let rel = i16::from_le_bytes([rom.image[5], rom.image[6]]);
        assert_eq!(0x0007i32 + i32::from(rel), i32::from(rom.layout.init));
        // init must return far.
        assert_eq!(rom.image[rom.layout.init as usize], 0xCB);
    }

    #[test]
    fn vga_rom_publishes_the_font_and_the_vbe_block() {
        let rom = build_vga_rom();
        // Glyph 0x41 ('A') sits at font + 0x41 * 16.
        let a = rom.layout.font as usize + 0x41 * crate::font::GLYPH_SIZE;
        assert_eq!(&rom.image[a..a + 16], &FONT_8X16[0x41]);
        assert_eq!(&rom.image[rom.layout.vbe_info as usize..][..4], b"VBE2");
    }

    #[test]
    fn rom_window_check_covers_both_images() {
        assert!(is_firmware_rom(SYSTEM_ROM_BASE));
        assert!(is_firmware_rom(0xFFFF0));
        assert!(is_firmware_rom(VGA_ROM_BASE));
        assert!(is_firmware_rom(VGA_ROM_BASE + VGA_ROM_SIZE as u32 - 1));
        // Just outside each window.
        assert!(!is_firmware_rom(SYSTEM_ROM_BASE - 1));
        assert!(!is_firmware_rom(VGA_ROM_BASE + VGA_ROM_SIZE as u32));
        // Ordinary guest memory and VGA video RAM.
        assert!(!is_firmware_rom(0x7C00));
        assert!(!is_firmware_rom(0xB8000));
        assert!(!is_firmware_rom(0xA0000));
    }
}
