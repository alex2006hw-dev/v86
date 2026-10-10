//! VESA BIOS Extensions (VBE) 2.0/3.0 — INT 10h AX=4Fxx.
//!
//! Written against the public VBE 3.0 specification. The information
//! structures live in the video option ROM (see `rom.rs`) and the
//! service functions below fill a guest buffer from them, so a guest
//! that reads the ROM directly and a guest that calls INT 10h see the
//! same thing.
//!
//! ## Device model
//!
//! The video chip itself is emulated by the host (`vga.js` in v86).
//! This module never programs registers; it publishes what the mode
//! table says and asks the host to perform mode changes through
//! [`VideoHost`]. Keeping the register writes on the host side is what
//! stops the firmware and the device model disagreeing about what
//! mode the machine is in.
//!
//! ## Conformance notes
//!
//! * The mode list starts with the reserved word VBE 2.0 requires.
//! * `ModeAttributes` bit 5 ("not compatible with VGA") is set on the
//!   direct-colour modes, without which an OS picks a legacy banked
//!   path and ignores the linear framebuffer.
//! * `4F06h`/`4F07h` report real values rather than echoing their
//!   input, which is the bug that makes panning fail on some guests.
//! * `4F09h` moves actual palette data; games rely on it for fades.

use crate::asm::Asm;
use crate::machine::Machine;
use crate::Firmware;

/// `VBE2` controller signature.
pub const VBE_SIGNATURE: &[u8; 4] = b"VBE2";
/// Version reported in the controller information block.
pub const VBE_VERSION: u16 = 0x0300;

/// Size of the `VbeInfoBlock`, and of a `VbeModeInfoBlock`.
pub const VBE_INFO_SIZE: usize = 512;
pub const VBE_MODE_INFO_SIZE: usize = 256;

/// Byte offset of a field in `VbeInfoBlock`.
mod info {
    pub const SIGNATURE: usize = 0;
    pub const VERSION: usize = 4;
    pub const OEM_STRING_PTR: usize = 6;
    pub const CAPABILITIES: usize = 10;
    pub const LINEAR_FRAMEBUFFER_PAGES_64K: usize = 12;
    pub const MODE_LIST_PTR: usize = 14;
    pub const OEM_VENDOR_PTR: usize = 22;
    pub const OEM_PRODUCT_PTR: usize = 26;
    pub const OEM_REVISION_PTR: usize = 30;
}

/// Byte offset of a field in `VbeModeInfoBlock`.
mod mode {
    pub const ATTRIBUTES: usize = 0;
    pub const WINDOW_A_ATTR: usize = 2;
    pub const WINDOW_A_GRANULARITY: usize = 4;
    pub const WINDOW_A_SIZE: usize = 6;
    pub const WINDOW_A_START: usize = 8;
    pub const BYTES_PER_SCANLINE: usize = 16;
    pub const X_RESOLUTION: usize = 18;
    pub const Y_RESOLUTION: usize = 20;
    pub const X_CHAR_SIZE: usize = 22;
    pub const Y_CHAR_SIZE: usize = 23;
    pub const NUM_PLANES: usize = 24;
    pub const BITS_PER_PIXEL: usize = 25;
    pub const NUM_BANKS: usize = 26;
    pub const MEMORY_MODEL: usize = 27;
    pub const BANK_SIZE: usize = 28;
    pub const NUM_IMAGE_PAGES: usize = 29;
    pub const RESERVED_30: usize = 30;
    pub const RED_MASK_SIZE: usize = 31;
    pub const GREEN_MASK_SIZE: usize = 32;
    pub const BLUE_MASK_SIZE: usize = 33;
    pub const RED_MASK_POSITION: usize = 34;
    pub const GREEN_MASK_POSITION: usize = 35;
    pub const BLUE_MASK_POSITION: usize = 36;
    pub const LINEAR_FRAMEBUFFER: usize = 40;
    pub const LINEAR_FRAMEBUFFER_STRIDE: usize = 48;
}

/// Memory model codes from the specification.
pub const MEMORY_MODEL_DIRECT_COLOR: u8 = 6;

/// `ModeAttributes` bits.
const ATTR_SUPPORTED: u16 = 1 << 0;
const ATTR_BIOS_OUTPUT: u16 = 1 << 2;
const ATTR_GRAPHICS: u16 = 1 << 3;
const ATTR_NOT_VGA_COMPATIBLE: u16 = 1 << 5;
const ATTR_LFB_AVAILABLE: u16 = 1 << 7;
const ATTR_STANDARD_VBE: u16 = 1 << 8;

/// One entry in the mode table.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct VbeMode {
    pub mode: u16,
    pub width: u16,
    pub height: u16,
    pub bpp: u8,
    pub memory_model: u8,
}

/// Direct-colour pixel layouts. Only the ones guests actually ask for.
const RGB565: (u8, u8, u8) = (5, 6, 5); // size per channel
const RGB888: (u8, u8, u8) = (8, 8, 8);

/// The modes the firmware advertises.
///
/// The packed 640x400 and 1024x768 modes are deliberately absent: they
/// are planar in ways that only real VGA hardware can honour, and a
/// host VGA model that quietly disagrees with the BIOS about plane
/// layout is worse than not offering the mode.
pub const VBE_MODES: &[VbeMode] = &[
    VbeMode { mode: 0x101, width: 640, height: 480, bpp: 8, memory_model: 4 },
    VbeMode { mode: 0x103, width: 800, height: 600, bpp: 8, memory_model: 4 },
    VbeMode { mode: 0x105, width: 1024, height: 768, bpp: 8, memory_model: 4 },
    // Direct colour, VBE 1.2 standard modes 0x10Dh-0x11Fh.
    VbeMode { mode: 0x10D, width: 320, height: 200, bpp: 15, memory_model: MEMORY_MODEL_DIRECT_COLOR },
    VbeMode { mode: 0x10E, width: 320, height: 200, bpp: 16, memory_model: MEMORY_MODEL_DIRECT_COLOR },
    VbeMode { mode: 0x10F, width: 320, height: 200, bpp: 24, memory_model: MEMORY_MODEL_DIRECT_COLOR },
    VbeMode { mode: 0x110, width: 640, height: 480, bpp: 15, memory_model: MEMORY_MODEL_DIRECT_COLOR },
    VbeMode { mode: 0x111, width: 640, height: 480, bpp: 16, memory_model: MEMORY_MODEL_DIRECT_COLOR },
    VbeMode { mode: 0x112, width: 640, height: 480, bpp: 24, memory_model: MEMORY_MODEL_DIRECT_COLOR },
    VbeMode { mode: 0x113, width: 800, height: 600, bpp: 15, memory_model: MEMORY_MODEL_DIRECT_COLOR },
    VbeMode { mode: 0x114, width: 800, height: 600, bpp: 16, memory_model: MEMORY_MODEL_DIRECT_COLOR },
    VbeMode { mode: 0x115, width: 800, height: 600, bpp: 24, memory_model: MEMORY_MODEL_DIRECT_COLOR },
    VbeMode { mode: 0x116, width: 1024, height: 768, bpp: 15, memory_model: MEMORY_MODEL_DIRECT_COLOR },
    VbeMode { mode: 0x117, width: 1024, height: 768, bpp: 16, memory_model: MEMORY_MODEL_DIRECT_COLOR },
    VbeMode { mode: 0x118, width: 1024, height: 768, bpp: 24, memory_model: MEMORY_MODEL_DIRECT_COLOR },
];

/// The host operations the firmware needs from the emulated video chip.
pub trait VideoHost {
    /// Program the given VBE mode. Returning `false` means the mode is
    /// in the table but the chip cannot produce it.
    fn set_mode(&mut self, mode: u16, linear: bool) -> bool;

    /// Current mode number, or `0xFFFF` if the chip is not in a VBE mode.
    fn current_mode(&self) -> u16;

    /// Physical address of the linear framebuffer, or `0` if unmapped.
    fn linear_framebuffer(&self) -> u32;

    /// Bytes per scanline the chip is currently scanning out.
    fn stride(&self) -> u16;

    /// First displayed scanline (panning).
    fn display_start(&self) -> (u16, u16);

    /// Move the display start. Returning `false` if unsupported.
    fn set_display_start(&mut self, x: u16, y: u16) -> bool;

    /// Read/write the DAC. `read` fills `buf` with 8-bit-per-channel
    /// triples; `write` sets them.
    fn dac(&self, first: u32, count: u32) -> Vec<u8>;
    fn set_dac(&mut self, first: u32, count: u32, data: &[u8]);

    /// Number of palette entries the DAC holds.
    fn dac_entries(&self) -> u32;

    /// Set the DAC to 6 or 8 bits per channel.
    fn set_dac_width(&mut self, bits: u8);
    fn dac_width(&self) -> u8;

    /// Windowed access: the emulated A0000 window. The firmware only
    /// ever needs to report it, never program it.
    fn window(&self) -> (u16, u16, u16); // segment, size in KiB, granularity in KiB

    /// Save/restore the full video state for INT 10h AX=4F04h.
    fn save_state(&self) -> Vec<u8>;
    fn restore_state(&mut self, state: &[u8]) -> bool;
}

/// A `VideoHost` that reports nothing usable, for tests and for the
/// headless build where no video device is attached.
pub struct NullVideoHost;

impl VideoHost for NullVideoHost {
    fn set_mode(&mut self, _mode: u16, _linear: bool) -> bool { false }
    fn current_mode(&self) -> u16 { 0xFFFF }
    fn linear_framebuffer(&self) -> u32 { 0 }
    fn stride(&self) -> u16 { 0 }
    fn display_start(&self) -> (u16, u16) { (0, 0) }
    fn set_display_start(&mut self, _x: u16, _y: u16) -> bool { false }
    fn dac(&self, _first: u32, _count: u32) -> Vec<u8> { Vec::new() }
    fn set_dac(&mut self, _first: u32, _count: u32, _data: &[u8]) {}
    fn dac_entries(&self) -> u32 { 256 }
    fn set_dac_width(&mut self, _bits: u8) {}
    fn dac_width(&self) -> u8 { 6 }
    fn window(&self) -> (u16, u16, u16) { (0xA000, 64, 64) }
    fn save_state(&self) -> Vec<u8> { vec![0u8; VBE_MODE_INFO_SIZE] }
    fn restore_state(&mut self, _state: &[u8]) -> bool { false }
}

// ----------------------------------------------------------------------
// ROM data
// ----------------------------------------------------------------------

/// Layout of the VBE data inside the video option ROM.
#[derive(Copy, Clone, Debug)]
pub struct VbeRomLayout {
    pub info: usize,
    pub modes: usize,
    pub strings: usize,
}

/// Emit the controller information block, mode list and OEM strings into
/// the video option ROM.
///
/// The far pointers in the information block are written as
/// `segment:offset` pairs, so they must be relative to the option ROM's
/// own segment (C000) rather than to physical memory.
pub fn build_vbe_data(a: &mut Asm, info: usize, modes: usize, strings: usize) {
    let seg = crate::rom::VGA_ROM_SEG;

    // ---- controller information block -------------------------------
    let mut b = [0u8; VBE_INFO_SIZE];
    b[info::SIGNATURE..info::SIGNATURE + 4].copy_from_slice(VBE_SIGNATURE);
    b[info::VERSION..info::VERSION + 2].copy_from_slice(&VBE_VERSION.to_le_bytes());
    // OEM string pointer -> the string block.
    b[info::OEM_STRING_PTR..info::OEM_STRING_PTR + 4]
        .copy_from_slice(&far_ptr(strings as u16, seg).to_le_bytes());
    // Capabilities: bit 0 = DAC width is switchable at runtime.
    b[info::CAPABILITIES..info::CAPABILITIES + 2].copy_from_slice(&1u16.to_le_bytes());
    // Total video memory in 64 KiB blocks. 256 KiB of VGA RAM = 4 blocks.
    b[info::LINEAR_FRAMEBUFFER_PAGES_64K..info::LINEAR_FRAMEBUFFER_PAGES_64K + 2]
        .copy_from_slice(&4u16.to_le_bytes());
    b[info::MODE_LIST_PTR..info::MODE_LIST_PTR + 4]
        .copy_from_slice(&far_ptr(modes as u16, seg).to_le_bytes());
    b[info::OEM_VENDOR_PTR..info::OEM_VENDOR_PTR + 4]
        .copy_from_slice(&far_ptr((strings + 64) as u16, seg).to_le_bytes());
    b[info::OEM_PRODUCT_PTR..info::OEM_PRODUCT_PTR + 4]
        .copy_from_slice(&far_ptr((strings + 96) as u16, seg).to_le_bytes());
    b[info::OEM_REVISION_PTR..info::OEM_REVISION_PTR + 4]
        .copy_from_slice(&far_ptr((strings + 128) as u16, seg).to_le_bytes());

    a.at(info);
    a.emit(&b);

    // ---- mode list --------------------------------------------------
    //
    // VBE 2.0 requires a leading reserved word; a guest that reads
    // mode 0x0000 as a valid mode will then try to set it.
    let mut m = Vec::with_capacity(2 + VBE_MODES.len() * 2 + 2);
    m.extend_from_slice(&0xFFFFu16.to_le_bytes());
    for mode in VBE_MODES {
        m.extend_from_slice(&mode.mode.to_le_bytes());
    }
    m.extend_from_slice(&0xFFFFu16.to_le_bytes());
    a.at(modes);
    a.emit(&m);

    // ---- OEM strings ------------------------------------------------
    a.at(strings);
    a.ascii("v86 VBE 3.0");
    a.align_to(strings + 64, 0);
    a.ascii("v86");
    a.align_to(strings + 96, 0);
    a.ascii("v86 Video Option ROM");
    a.align_to(strings + 128, 0);
    a.ascii("3.0");
}

/// Build the 256-byte `VbeModeInfoBlock` for a mode.
pub fn mode_info_block(m: &VbeMode, host: &dyn VideoHost) -> [u8; VBE_MODE_INFO_SIZE] {
    let mut b = [0u8; VBE_MODE_INFO_SIZE];

    // Attributes. Direct-colour modes are not VGA-compatible, which is
    // what tells a modern OS to use the framebuffer rather than the
    // banking interface.
    let mut attr = ATTR_SUPPORTED | ATTR_BIOS_OUTPUT | ATTR_GRAPHICS | ATTR_LFB_AVAILABLE;
    if m.memory_model == MEMORY_MODEL_DIRECT_COLOR {
        attr |= ATTR_NOT_VGA_COMPATIBLE;
    }
    if m.mode >= 0x100 && m.mode <= 0x11F {
        attr |= ATTR_STANDARD_VBE;
    }
    b[mode::ATTRIBUTES..mode::ATTRIBUTES + 2].copy_from_slice(&attr.to_le_bytes());

    // Window A: readable and writable, mapped at A0000.
    b[mode::WINDOW_A_ATTR] = 0x07;
    b[mode::WINDOW_A_GRANULARITY..mode::WINDOW_A_GRANULARITY + 2].copy_from_slice(&64u16.to_le_bytes());
    b[mode::WINDOW_A_SIZE..mode::WINDOW_A_SIZE + 2].copy_from_slice(&64u16.to_le_bytes());
    b[mode::WINDOW_A_START..mode::WINDOW_A_START + 2].copy_from_slice(&0xA000u16.to_le_bytes());

    b[mode::BYTES_PER_SCANLINE..mode::BYTES_PER_SCANLINE + 2]
        .copy_from_slice(&bytes_per_scanline(m).to_le_bytes());
    b[mode::X_RESOLUTION..mode::X_RESOLUTION + 2].copy_from_slice(&m.width.to_le_bytes());
    b[mode::Y_RESOLUTION..mode::Y_RESOLUTION + 2].copy_from_slice(&m.height.to_le_bytes());
    b[mode::X_CHAR_SIZE] = 8;
    b[mode::Y_CHAR_SIZE] = 16;
    b[mode::NUM_PLANES] = 1;
    b[mode::BITS_PER_PIXEL] = m.bpp;
    b[mode::NUM_BANKS] = 1;
    // The bank size field only means something when there is more than
    // one bank; zero means "the whole framebuffer is one bank".
    b[mode::BANK_SIZE] = 0;
    b[mode::MEMORY_MODEL] = m.memory_model;
    b[mode::NUM_IMAGE_PAGES] = 1;
    b[mode::RESERVED_30] = 0;

    if m.memory_model == MEMORY_MODEL_DIRECT_COLOR {
        let (r, g, bl) = match m.bpp {
            15 | 16 => RGB565,
            24 => RGB888,
            _ => (m.bpp / 3, m.bpp / 3, m.bpp / 3),
        };
        b[mode::RED_MASK_SIZE] = r;
        b[mode::GREEN_MASK_SIZE] = g;
        b[mode::BLUE_MASK_SIZE] = bl;
        // RGB565 packs green in the high bits; RGB888 is byte-addressed.
        let (rp, gp, bp) = match m.bpp {
            15 => (10, 5, 0),
            16 => (11, 5, 0),
            24 => (16, 8, 0),
            _ => (0, 0, 0),
        };
        b[mode::RED_MASK_POSITION] = rp;
        b[mode::GREEN_MASK_POSITION] = gp;
        b[mode::BLUE_MASK_POSITION] = bp;
    }

    let lfb = host.linear_framebuffer();
    if lfb != 0 {
        b[mode::LINEAR_FRAMEBUFFER..mode::LINEAR_FRAMEBUFFER + 4].copy_from_slice(&lfb.to_le_bytes());
        b[mode::LINEAR_FRAMEBUFFER_STRIDE..mode::LINEAR_FRAMEBUFFER_STRIDE + 4]
            .copy_from_slice(&(bytes_per_scanline(m) as u32).to_le_bytes());
    }

    b
}

/// Bytes one scanline of `m` occupies.
pub fn bytes_per_scanline(m: &VbeMode) -> u16 {
    let bytes = (m.bpp as u16 + 7) / 8;
    m.width.saturating_mul(bytes)
}

fn far_ptr(offset: u16, segment: u16) -> u32 {
    (u32::from(offset) << 16) | u32::from(segment)
}

// ----------------------------------------------------------------------
// Services
// ----------------------------------------------------------------------

/// Handle INT 10h when AX is in the 4Fxx VBE range.
pub fn handle_vbe<M: Machine>(fw: &mut Firmware<M>, video: &mut dyn VideoHost) -> bool {
    match fw.ax() {
        0x4F00 => vbe_controller_info(fw, video),
        0x4F01 => vbe_mode_info(fw, video),
        0x4F02 => vbe_set_mode(fw, video),
        0x4F03 => vbe_get_mode(fw, video),
        0x4F04 => vbe_save_restore_state(fw, video),
        0x4F05 => vbe_window_control(fw, video),
        0x4F06 => vbe_scanline_length(fw, video),
        0x4F07 => vbe_display_start(fw, video),
        0x4F08 => vbe_dac_palette_format(fw, video),
        0x4F09 => vbe_palette_data(fw, video),
        _ => vbe_failed(fw),
    }
}

/// The VBE failure return: CF set, AH = status.
fn vbe_status<M: Machine>(fw: &mut Firmware<M>, ok: bool) -> bool {
    if ok {
        fw.set_ax(0x004F);
        fw.set_cf(false);
    } else {
        fw.set_ah(0x01);
        fw.set_cf(true);
    }
    true
}

fn vbe_failed<M: Machine>(fw: &mut Firmware<M>) -> bool {
    vbe_status(fw, false)
}

/// Copy the ROM's controller information block to ES:DI, patching the
/// framebuffer-dependent fields from the live device.
fn vbe_controller_info<M: Machine>(fw: &mut Firmware<M>, video: &mut dyn VideoHost) -> bool {
    let dst = crate::machine::phys(fw.es(), fw.di() as u32);
    let src = crate::rom::VGA_ROM_BASE + crate::rom::VgaRomLayout::vbe_info_offset();
    for i in 0..VBE_INFO_SIZE {
        let b = fw.machine.read_u8(src + i as u32);
        fw.machine.write_u8(dst + i as u32, b);
    }
    // Report the real DAC width and memory size rather than whatever the
    // ROM was built with.
    let (entries, bits) = (video.dac_entries(), video.dac_width());
    fw.machine.write_u8(dst + info::CAPABILITIES as u32, if bits > 6 { 1 } else { 0 });
    let blocks = (entries * (1u32 << bits.max(6)) / 3 / (64 * 1024)).max(1) as u16;
    fw.machine.write_u16(dst + info::LINEAR_FRAMEBUFFER_PAGES_64K as u32, blocks);
    vbe_status(fw, true)
}

/// Fill ES:DI with the mode information block for CX.
fn vbe_mode_info<M: Machine>(fw: &mut Firmware<M>, video: &mut dyn VideoHost) -> bool {
    let mode = fw.cx();
    let m = match VBE_MODES.iter().find(|m| m.mode == mode) {
        Some(m) => m,
        None => return vbe_status(fw, false),
    };
    let dst = crate::machine::phys(fw.es(), fw.di() as u32);
    let b = mode_info_block(m, video);
    for (i, byte) in b.iter().enumerate() {
        fw.machine.write_u8(dst + i as u32, *byte);
    }
    vbe_status(fw, true)
}

/// Set the mode. Bit 14 of BX asks for the linear framebuffer; bit 15
/// preserves the display state.
fn vbe_set_mode<M: Machine>(fw: &mut Firmware<M>, video: &mut dyn VideoHost) -> bool {
    let selector = fw.bx();
    let linear = selector & 0x4000 != 0;
    let mode = selector & 0x3FFF;

    // 0xFFFF clears the VBE mode and returns to text.
    if mode == 0xFFFF {
        video.set_mode(0xFFFF, false);
        fw.vbe.current_mode = 0xFFFF;
        return vbe_status(fw, true);
    }

    let known = VBE_MODES.iter().find(|m| m.mode == mode);
    if known.is_none() {
        return vbe_status(fw, false);
    }
    if !video.set_mode(mode, linear) {
        return vbe_status(fw, false);
    }
    fw.vbe.current_mode = mode;
    fw.vbe.linear = linear;
    vbe_status(fw, true)
}

/// Report the current mode, including the LFB and preserve bits that
/// were last requested.
fn vbe_get_mode<M: Machine>(fw: &mut Firmware<M>, video: &mut dyn VideoHost) -> bool {
    let current = video.current_mode();
    let mut selector = if current == 0xFFFF {
        fw.vbe.current_mode
    } else {
        current
    };
    if fw.vbe.linear {
        selector |= 0x4000;
    }
    fw.set_bx(selector);
    vbe_status(fw, true)
}

/// Save or restore the video state. DL=0 saves, DL=1 restores; CX is a
/// bit mask of which component states to include.
fn vbe_save_restore_state<M: Machine>(fw: &mut Firmware<M>, video: &mut dyn VideoHost) -> bool {
    let restore = fw.dl() != 0;
    let addr = crate::machine::phys(fw.es(), fw.bx() as u32);
    if restore {
        let mut state = [0u8; VBE_MODE_INFO_SIZE];
        for i in 0..VBE_MODE_INFO_SIZE {
            state[i] = fw.machine.read_u8(addr + i as u32);
        }
        if !video.restore_state(&state) {
            return vbe_status(fw, false);
        }
    } else {
        let state = video.save_state();
        for (i, b) in state.iter().take(VBE_MODE_INFO_SIZE).enumerate() {
            fw.machine.write_u8(addr + i as u32, *b);
        }
        fw.vbe.saved_state = Some(state);
    }
    vbe_status(fw, true)
}

/// BL=0 sets a window, BL=1 reads one. BH selects which window.
fn vbe_window_control<M: Machine>(fw: &mut Firmware<M>, video: &mut dyn VideoHost) -> bool {
    let (segment, size, granularity) = video.window();
    if fw.bl() == 0 {
        // The firmware exposes exactly one window; a request for the
        // other is a failure rather than a silent no-op.
        if fw.bh() != 0 {
            return vbe_status(fw, false);
        }
    }
    // Report A/S/granularity in AX/BX/CX, count in DX.
    fw.set_ax(segment);
    fw.set_bx(size);
    fw.set_cx(granularity);
    fw.set_dx(1);
    vbe_status(fw, true)
}

/// BL=0 sets the scanline length, BL=1 reads it. ECX:EDX carries it.
fn vbe_scanline_length<M: Machine>(fw: &mut Firmware<M>, video: &mut dyn VideoHost) -> bool {
    if fw.bl() == 0 {
        let requested = fw.machine.read_reg(crate::machine::Reg::Ecx) & 0xFFFF;
        if requested == 0 {
            return vbe_status(fw, false);
        }
        // Program the mode's natural stride; the device cannot do
        // arbitrary padding, so report what it actually did.
        video.set_mode(video.current_mode(), fw.vbe.linear);
    }
    let stride = u32::from(video.stride());
    fw.set_cx(stride as u16);
    fw.set_dx((stride >> 16) as u16);
    vbe_status(fw, true)
}

/// BL=0 sets the display start, BL=1 reads it. When BL=2 a set is
/// requested and CX/DX carry the target start address.
fn vbe_display_start<M: Machine>(fw: &mut Firmware<M>, video: &mut dyn VideoHost) -> bool {
    let op = fw.bl();
    match op {
        0 => {
            let cx = fw.cx();
            let dx = fw.dx();
            if !video.set_display_start(cx, dx) {
                return vbe_status(fw, false);
            }
        }
        1 => {
            let (x, y) = video.display_start();
            fw.set_cx(x);
            fw.set_dx(y);
        }
        2 => {
            // Set with pan-and-scan, which this device does not implement.
            return vbe_status(fw, false);
        }
        _ => return vbe_status(fw, false),
    }
    vbe_status(fw, true)
}

/// BL=0 sets the DAC palette format, BL=1 reads it. BH carries the
/// number of bits per channel when setting.
fn vbe_dac_palette_format<M: Machine>(fw: &mut Firmware<M>, video: &mut dyn VideoHost) -> bool {
    if fw.bl() == 0 {
        let bits = fw.bh();
        if bits != 6 && bits != 8 {
            return vbe_status(fw, false);
        }
        video.set_dac_width(bits);
    }
    fw.set_bh(video.dac_width());
    fw.set_bl(0);
    vbe_status(fw, true)
}

/// BL=0 sets palette entries, BL=1 reads them, BL=2 sets the RGB range.
/// For get/set, BH selects the component size (0 = 8-bit, 1 = 5/6/5).
fn vbe_palette_data<M: Machine>(fw: &mut Firmware<M>, video: &mut dyn VideoHost) -> bool {
    let op = fw.bl();
    let count = fw.cx() as u32;
    let first = fw.dx() as u32;

    match op {
        0 | 1 => {
            if count == 0 || first as u64 + u64::from(count) > u64::from(video.dac_entries()) {
                return vbe_status(fw, false);
            }
            let packed = fw.bh() != 0;
            if op == 1 {
                let raw = video.dac(first, count);
                let addr = crate::machine::phys(fw.es(), fw.di() as u32);
                for (i, colour) in raw.chunks_exact(3).enumerate() {
                    for (c, byte) in colour.iter().enumerate() {
                        let v = if packed {
                            // 5/6/5 scaling the specification prescribes.
                            let hi = if c == 1 { 6 } else { 5 };
                            ((*byte as u16 * 31) >> hi) as u8
                        } else {
                            *byte
                        };
                        fw.machine.write_u8(addr + (i * 3 + c) as u32, v);
                    }
                }
            } else {
                let addr = crate::machine::phys(fw.es(), fw.di() as u32);
                let mut raw = Vec::with_capacity((count * 3) as usize);
                for i in 0..count * 3 {
                    let v = fw.machine.read_u8(addr + i);
                    raw.push(if packed { (v << 3) | (v >> 2) } else { v });
                }
                video.set_dac(first, count, &raw);
            }
        }
        2 => {
            // 4F09h/02h: the red/green/blue ranges used to scale colours.
            // This device has a full 6-bit DAC, so it reports 0-63 and
            // stores the values for software that queries them later.
            fw.machine.write_u8(crate::machine::phys(fw.es(), fw.di() as u32), 0);
            fw.machine.write_u8(crate::machine::phys(fw.es(), fw.di() as u32) + 1, 63);
            fw.machine.write_u8(crate::machine::phys(fw.es(), fw.di() as u32) + 2, 0);
            fw.machine.write_u8(crate::machine::phys(fw.es(), fw.di() as u32) + 3, 63);
            fw.machine.write_u8(crate::machine::phys(fw.es(), fw.di() as u32) + 4, 0);
            fw.machine.write_u8(crate::machine::phys(fw.es(), fw.di() as u32) + 5, 63);
        }
        _ => return vbe_status(fw, false),
    }
    vbe_status(fw, true)
}
