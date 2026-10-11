//! Option ROM support.
//!
//! An option ROM is a small piece of code the firmware is expected to
//! run during POST: it initialises a device, and in the boot case it
//! takes the machine over entirely. v86 needs this for its two ways of
//! booting a kernel -- `bzimage` and `multiboot` both build a 512-byte
//! stub that `kernel.js::make_linux_boot_rom` describes as "executed by
//! seabios after its initialisation". Without it, that stub is ordinary
//! data sitting in a buffer and the kernel never starts.
//!
//! It is not kernel-specific: a guest that installs its own option ROM
//! and reboots expects the same treatment, and a boot manager that
//! wants to run before the bootstrap needs somewhere to live.
//!
//! ## Where the ROMs live
//!
//! The option-ROM run is `0xD0000`-`0xDFFFF`: above the video option
//! ROM at `0xC0000` (32 KiB) and above the VBE structures at `0xC8000`,
//! and clear of the `0xE0000` region a real chipset reserves. ROMs are
//! placed back to back on their own length in 512-byte blocks, which is
//! what byte 2 of the header counts, so a guest reading them back finds
//! them where a scan would.
//!
//! ## The handover
//!
//! A ROM's entry point is at offset 3 and is reached with a **far call**,
//! so a ROM that returns resumes the firmware rather than running off
//! the end of its image. The return address points into the system ROM,
//! which jumps to the INT 19h stub -- so a returning ROM falls through
//! to the ordinary bootstrap, and a non-returning one (the Linux stub
//! ends in a far jump) simply never uses it.

use crate::machine::{Machine, SegReg};
use crate::Firmware;

/// Physical base of the option-ROM run. Above the video ROM and the VBE
/// structures, below the `0xE0000` region.
pub const OPTION_ROM_BASE: u32 = 0xD_0000;
/// One past the last byte a registered ROM may occupy.
pub const OPTION_ROM_END: u32 = 0xE_0000;

/// Header signature: byte 0 `0x55`, byte 1 `0xAA`.
pub const SIGNATURE: [u8; 2] = [0x55, 0xAA];

/// Offset of the far entry point relative to the start of the image.
pub const ENTRY_OFFSET: usize = 3;

/// Length of the option-ROM header.
pub const HEADER_LEN: usize = 3;

/// Why a registered image was not run.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Rejection {
    /// Shorter than a header.
    TooShort,
    /// Missing the `55 AA` signature.
    BadSignature,
    /// The declared length is zero, or beyond the image.
    BadLength,
    /// The bytes do not sum to zero, so the image is truncated or corrupt.
    BadChecksum,
}

impl Rejection {
    /// One line for the trace log.
    pub fn as_str(self) -> &'static str {
        match self {
            Rejection::TooShort => "too short",
            Rejection::BadSignature => "no 55AA signature",
            Rejection::BadLength => "bad length",
            Rejection::BadChecksum => "bad checksum",
        }
    }
}

/// Length in bytes an image declares for itself, from header byte 2.
///
/// Byte 2 counts 512-byte blocks. The kernel stub sets it to 1 for a
/// single 512-byte block; a 32 KiB VGA ROM would set it to 64.
pub fn declared_size(data: &[u8]) -> Option<usize> {
    if data.len() < HEADER_LEN {
        return None;
    }

    let blocks = usize::from(data[2]);
    if blocks == 0 {
        return None;
    }

    Some(blocks * 512)
}

/// Whether an image is a well-formed option ROM, and how long it is.
///
/// The checksum is the sum of every byte in the declared length, taken
/// modulo 256, which must be zero. It is what makes a truncated or
/// mistyped image detectable, so it is checked rather than assumed --
/// the firmware is the only thing that will ever look at these bytes.
pub fn validate(data: &[u8]) -> Result<usize, Rejection> {
    if data.len() < HEADER_LEN {
        return Err(Rejection::TooShort);
    }

    if data[0] != SIGNATURE[0] || data[1] != SIGNATURE[1] {
        return Err(Rejection::BadSignature);
    }

    let size = declared_size(data).ok_or(Rejection::BadLength)?;
    if size > data.len() {
        return Err(Rejection::BadLength);
    }

    // Sum over the declared length, not the buffer: an image padded to
    // a block boundary has to sum to zero over what it claims, and
    // nothing outside that belongs to it.
    let sum: u32 = data[..size].iter().map(|&b| u32::from(b)).sum();
    if sum & 0xFF != 0 {
        return Err(Rejection::BadChecksum);
    }

    Ok(size)
}

/// Round a length up to the 512-byte block the next ROM starts on.
///
/// Placement has to stay 16-byte aligned, because the entry point is
/// addressed as `segment:3`, so the block size must be a multiple of
/// 16 -- which 512 is.
fn round_up(length: usize) -> usize {
    ((length + 511) / 512) * 512
}

/// Run every registered option ROM.
///
/// Each is copied into guest memory, checked, and entered with a far
/// call whose return address lands on the bootstrap path in the system
/// ROM. ROMs run in registration order, which is the order the host
/// supplied them in.
///
/// A ROM that never returns -- the Linux stub ends in a far jump --
/// means the code after the loop does not run, and nothing does: the
/// machine is now the guest's.
pub fn run_all<M: Machine>(fw: &mut Firmware<M>) {
    if fw.option_roms.is_empty() {
        return;
    }

    // The return target comes from the ROM layout, which `install_roms`
    // fills in before this runs.
    let return_offset = match fw.roms.as_ref() {
        Some(layout) => layout.option_rom_return,
        None => {
            fw.trace(crate::debug::tag::POST, "option ROMs: no ROM layout");
            return;
        }
    };

    // Taken rather than borrowed: the far call below hands the machine
    // over, so a borrow held across it would outlive its owner.
    let roms = std::mem::take(&mut fw.option_roms);
    let mut address = OPTION_ROM_BASE;

    for data in &roms {
        if address >= OPTION_ROM_END {
            fw.trace(crate::debug::tag::POST,
                &format!("option ROM run is full at 0x{:05X}", address));
            break;
        }

        match validate(data) {
            Ok(size) => {
                if address + size as u32 > OPTION_ROM_END {
                    fw.trace(crate::debug::tag::POST,
                        &format!("option ROM of {} bytes does not fit above 0x{:05X}", size, address));
                    break;
                }

                for (i, b) in data.iter().take(size).enumerate() {
                    fw.machine.write_u8(address + i as u32, *b);
                }

                fw.trace(crate::debug::tag::POST,
                    &format!("running option ROM of {} bytes at 0x{:05X}", size, address));
                run_one(fw, address, return_offset);
                address += round_up(size) as u32;
            }
            Err(reason) => {
                // Skip past what it claimed, so one bad image cannot
                // stall the ones behind it.
                fw.trace(crate::debug::tag::POST,
                    &format!("option ROM rejected ({}) at 0x{:05X}", reason.as_str(), address));
                address += round_up(data.len().max(HEADER_LEN)) as u32;
            }
        }
    }

    // Put them back so a save/restore still has them.
    fw.option_roms = roms;
}

/// Enter one ROM with a far call.
fn run_one<M: Machine>(fw: &mut Firmware<M>, address: u32, return_offset: u16) {
    // The entry point is at offset 3, addressed as `segment:3`:
    // placement is 16-byte aligned, so the image starts on a segment
    // boundary and the offset really is 3.
    let segment = (address >> 4) as u16;

    // A far call pushes the offset first and the segment second, so the
    // frame the ROM's `retf` pops reads CS at [SP] and IP at [SP+2].
    fw.machine.push_u16(return_offset);
    fw.machine.push_u16(crate::rom::SYSTEM_ROM_SEG);

    // CS before IP: the instruction pointer is relative to the code
    // segment, so writing IP while CS still names the old base would
    // compute the wrong linear address.
    fw.machine.write_seg(SegReg::Cs, segment);
    fw.machine.write_ip(u32::from(ENTRY_OFFSET as u16));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal well-formed ROM: 55 AA, one block, an entry point, and
    /// a checksum byte that makes the image sum to zero.
    fn rom(entry: &[u8]) -> Vec<u8> {
        let mut data = vec![0u8; 512];
        data[0] = 0x55;
        data[1] = 0xAA;
        data[2] = 1;
        for (i, b) in entry.iter().enumerate() {
            data[ENTRY_OFFSET + i] = *b;
        }
        let sum: u32 = data.iter().map(|&b| u32::from(b)).sum();
        data[511] = (-(sum as i32) & 0xFF) as u8;
        data
    }

    #[test]
    fn declares_its_own_length_in_blocks() {
        assert_eq!(declared_size(&[0x55, 0xAA, 1]), Some(512));
        assert_eq!(declared_size(&[0x55, 0xAA, 64]), Some(32768));
        assert_eq!(declared_size(&[0x55, 0xAA, 0]), None, "zero blocks is meaningless");
        assert_eq!(declared_size(&[0x55]), None);
    }

    #[test]
    fn a_well_formed_rom_validates() {
        assert_eq!(validate(&rom(&[0xCB])), Ok(512));
    }

    #[test]
    fn the_signature_is_enforced() {
        let mut data = rom(&[0xCB]);
        data[1] = 0xAB;
        // Repair the checksum so the signature is the only thing wrong.
        let sum: u32 = data.iter().map(|&b| u32::from(b)).sum();
        data[511] = (-(sum as i32) & 0xFF) as u8;
        assert_eq!(validate(&data), Err(Rejection::BadSignature));
    }

    #[test]
    fn a_broken_checksum_is_rejected() {
        // The case that matters: a truncated image, or one that was
        // never a ROM at all, must not be jumped into. Flipping any one
        // byte always changes the sum, because XOR with 0xFF moves a
        // byte by an odd amount.
        let mut data = rom(&[0xCB]);
        data[0x40] ^= 0xFF;
        assert_eq!(validate(&data), Err(Rejection::BadChecksum));
    }

    #[test]
    fn a_declared_length_beyond_the_image_is_rejected() {
        let mut data = rom(&[0xCB]);
        data[2] = 4; // claims 2 KiB, the buffer is 512 bytes
        assert_eq!(validate(&data), Err(Rejection::BadLength));
    }

    #[test]
    fn a_short_image_is_rejected() {
        assert_eq!(validate(&[]), Err(Rejection::TooShort));
        assert_eq!(validate(&[0x55, 0xAA]), Err(Rejection::TooShort));
    }

    #[test]
    fn the_entry_point_is_at_offset_three() {
        // The Linux boot stub puts `cli; mov ax, ...` at offset 3.
        let data = rom(&[0xFA, 0xB8, 0x00, 0x80]);
        assert_eq!(validate(&data), Ok(512));
        assert_eq!(data[ENTRY_OFFSET], 0xFA);
    }

    #[test]
    fn roms_are_placed_on_block_boundaries() {
        assert_eq!(round_up(1), 512);
        assert_eq!(round_up(512), 512);
        assert_eq!(round_up(513), 1024);
        for length in [512usize, 600, 1024, 2048] {
            assert_eq!(round_up(length) % 16, 0, "{} is not segment-aligned", length);
        }
    }

    #[test]
    fn the_area_is_clear_of_the_roms_the_firmware_owns() {
        assert!(OPTION_ROM_BASE > crate::rom::VGA_ROM_BASE + crate::rom::VGA_ROM_SIZE as u32);
        assert!(OPTION_ROM_BASE >= crate::dispatch::VBE_ROM_BASE);
        assert!(OPTION_ROM_END > OPTION_ROM_BASE);
    }

    #[test]
    fn the_area_holds_several_kernel_stubs() {
        // The bzimage stub is exactly one block, and v86 also builds a
        // multiboot ROM this size. There should be room for both.
        assert!(OPTION_ROM_END - OPTION_ROM_BASE >= 1024);
        assert!(OPTION_ROM_END - OPTION_ROM_BASE >= 64 * 1024);
    }
}
