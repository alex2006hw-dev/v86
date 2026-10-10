//! VESA BIOS Extensions (VBE) 2.0+ — INT 10h AX=4F00h–4F09h.
//!
//! Implements the VBE 2.0/3.0 specification from public
//! documentation. The VBE mode table and strings are placed at
//! physical 0xC8000 (option ROM area).

use crate::dispatch::VBE_ROM_BASE;
use crate::machine::Machine;
use crate::Firmware;

/// VBE controller info signature.
pub const VBE_SIGNATURE: &[u8; 4] = b"VBE2";
/// VBE version (3.0).
pub const VBE_VERSION_MAJOR: u8 = 3;
pub const VBE_VERSION_MINOR: u8 = 0;

/// VBE mode info structure size.
pub const VBE_MODE_INFO_SIZE: usize = 256;
/// VBE controller info structure size.
pub const VBE_CONTROLLER_INFO_SIZE: usize = 512;

/// A VBE mode descriptor.
#[derive(Copy, Clone, Debug)]
pub struct VbeMode {
    pub mode: u16,
    pub width: u16,
    pub height: u16,
    pub bpp: u8,
    pub memory_model: u8,
    pub lfb: bool,
}

/// VBE 1.2 standard mode table.
pub const VBE_MODES: &[VbeMode] = &[
    VbeMode { mode: 0x100, width: 640, height: 400, bpp: 8, memory_model: 4, lfb: true },
    VbeMode { mode: 0x101, width: 640, height: 480, bpp: 8, memory_model: 4, lfb: true },
    VbeMode { mode: 0x102, width: 800, height: 600, bpp: 4, memory_model: 3, lfb: true },
    VbeMode { mode: 0x103, width: 800, height: 600, bpp: 8, memory_model: 4, lfb: true },
    VbeMode { mode: 0x104, width: 1024, height: 768, bpp: 4, memory_model: 3, lfb: true },
    VbeMode { mode: 0x105, width: 1024, height: 768, bpp: 8, memory_model: 4, lfb: true },
    VbeMode { mode: 0x106, width: 1280, height: 1024, bpp: 4, memory_model: 3, lfb: true },
    VbeMode { mode: 0x107, width: 1280, height: 1024, bpp: 8, memory_model: 4, lfb: true },
    // Direct color modes (VBE 1.2+).
    VbeMode { mode: 0x10D, width: 320, height: 200, bpp: 15, memory_model: 6, lfb: true },
    VbeMode { mode: 0x10E, width: 320, height: 200, bpp: 16, memory_model: 6, lfb: true },
    VbeMode { mode: 0x10F, width: 320, height: 200, bpp: 24, memory_model: 6, lfb: true },
    VbeMode { mode: 0x110, width: 640, height: 480, bpp: 15, memory_model: 6, lfb: true },
    VbeMode { mode: 0x111, width: 640, height: 480, bpp: 16, memory_model: 6, lfb: true },
    VbeMode { mode: 0x112, width: 640, height: 480, bpp: 24, memory_model: 6, lfb: true },
    VbeMode { mode: 0x113, width: 800, height: 600, bpp: 15, memory_model: 6, lfb: true },
    VbeMode { mode: 0x114, width: 800, height: 600, bpp: 16, memory_model: 6, lfb: true },
    VbeMode { mode: 0x115, width: 800, height: 600, bpp: 24, memory_model: 6, lfb: true },
    VbeMode { mode: 0x116, width: 1024, height: 768, bpp: 15, memory_model: 6, lfb: true },
    VbeMode { mode: 0x117, width: 1024, height: 768, bpp: 16, memory_model: 6, lfb: true },
    VbeMode { mode: 0x118, width: 1024, height: 768, bpp: 24, memory_model: 6, lfb: true },
    VbeMode { mode: 0x119, width: 1280, height: 1024, bpp: 15, memory_model: 6, lfb: true },
    VbeMode { mode: 0x11A, width: 1280, height: 1024, bpp: 16, memory_model: 6, lfb: true },
    VbeMode { mode: 0x11B, width: 1280, height: 1024, bpp: 24, memory_model: 6, lfb: true },
];

/// Install the VBE mode table and strings at 0xC8000.
pub fn install_vbe_rom<M: Machine>(fw: &mut Firmware<M>) {
    let base = VBE_ROM_BASE;

    // Controller info at base.
    let mut info = [0u8; VBE_CONTROLLER_INFO_SIZE];
    info[0..4].copy_from_slice(VBE_SIGNATURE);
    info[4..6].copy_from_slice(&0x0300u16.to_le_bytes()); // version 3.0
    // OEM string pointer (seg:off far pointer, 32-bit).
    let oem_str_addr = base + 0x100;
    info[6..10].copy_from_slice(&oem_str_addr.to_le_bytes());
    // Capabilities: bit 0 = DAC width switchable, bit 1 = legacy VGA not present.
    info[10..14].copy_from_slice(&0x00000001u32.to_le_bytes());
    // Mode list pointer (seg:off far pointer, 32-bit).
    let mode_list_addr = base + 0x200;
    info[14..18].copy_from_slice(&mode_list_addr.to_le_bytes());
    // Total memory in 64KB blocks.
    let mem_blocks = (fw.config.vga_memory_size / 65536) as u16;
    info[12..14].copy_from_slice(&mem_blocks.to_le_bytes());
    // OEM vendor name pointer (VBE 2.0+: offset 22).
    let vendor_addr = base + 0x120;
    info[22..26].copy_from_slice(&vendor_addr.to_le_bytes());
    // OEM product name pointer (offset 26).
    let product_addr = base + 0x140;
    info[26..30].copy_from_slice(&product_addr.to_le_bytes());
    // OEM product revision pointer (offset 30).
    let rev_addr = base + 0x160;
    info[30..34].copy_from_slice(&rev_addr.to_le_bytes());

    for (i, b) in info.iter().enumerate() {
        fw.machine.write_u8(base + i as u32, *b);
    }

    // OEM string "PCjs VBE 3.0" at base+0x100.
    let oem_str = b"PCjs VBE 3.0\0";
    for (i, b) in oem_str.iter().enumerate() {
        fw.machine.write_u8(base + 0x100 + i as u32, *b);
    }

    // Vendor name.
    let vendor = b"PCjs\0";
    for (i, b) in vendor.iter().enumerate() {
        fw.machine.write_u8(base + 0x120 + i as u32, *b);
    }

    // Product name.
    let product = b"v86 VBE\0";
    for (i, b) in product.iter().enumerate() {
        fw.machine.write_u8(base + 0x140 + i as u32, *b);
    }

    // Product revision.
    let rev = b"3.0\0";
    for (i, b) in rev.iter().enumerate() {
        fw.machine.write_u8(base + 0x160 + i as u32, *b);
    }

    // Mode list at base+0x200: word per mode, terminated by 0xFFFF.
    let mut addr = base + 0x200;
    for mode in VBE_MODES {
        fw.machine.write_u16(addr, mode.mode);
        addr += 2;
    }
    fw.machine.write_u16(addr, 0xFFFF);
}

/// Handle a VBE call (AX=4F00h–4F09h).
pub fn handle_vbe<M: Machine>(fw: &mut Firmware<M>) -> bool {
    let ax = fw.ax();
    match ax {
        0x4F00 => vbe_get_controller_info(fw),
        0x4F01 => vbe_get_mode_info(fw),
        0x4F02 => vbe_set_mode(fw),
        0x4F03 => vbe_get_mode(fw),
        0x4F04 => vbe_save_restore_state(fw),
        0x4F05 => vbe_window_control(fw),
        0x4F06 => vbe_scanline_length(fw),
        0x4F07 => vbe_display_start(fw),
        0x4F08 => vbe_dac_palette_format(fw),
        0x4F09 => vbe_palette_data(fw),
        _ => false,
    }
}

/// AX=4F00h: get controller info (ES:DI -> 512-byte buffer).
fn vbe_get_controller_info<M: Machine>(fw: &mut Firmware<M>) -> bool {
    let addr = crate::machine::phys(fw.es(), fw.di() as u32);
    let base = VBE_ROM_BASE;
    // Copy the controller info from our ROM area.
    for i in 0..VBE_CONTROLLER_INFO_SIZE {
        let b = fw.machine.read_u8(base + i as u32);
        fw.machine.write_u8(addr + i as u32, b);
    }
    fw.set_ax(0x004F);
    fw.set_cf(false);
    true
}

/// AX=4F01h: get mode info (CX=mode, ES:DI -> 256-byte buffer).
fn vbe_get_mode_info<M: Machine>(fw: &mut Firmware<M>) -> bool {
    let mode = fw.cx();
    let addr = crate::machine::phys(fw.es(), fw.di() as u32);

    let vbe_mode = match VBE_MODES.iter().find(|m| m.mode == mode) {
        Some(m) => *m,
        None => {
            fw.set_ax(0x014F); // failed
            fw.set_cf(true);
            return true;
        }
    };

    let mut info = [0u8; VBE_MODE_INFO_SIZE];
    // Mode attributes: bit 0 = supported, bit 1 = optional, bit 2 = BIOS output, bit 3 = color, bit 4 = graphics, bit 5 = not VGA compatible, bit 6 = windowed, bit 7 = LFB.
    let mut mode_attr: u16 = 0x0001 | 0x0008 | 0x0010; // supported, color, graphics
    if vbe_mode.lfb {
        mode_attr |= 0x0080; // LFB available
    }
    info[0..2].copy_from_slice(&mode_attr.to_le_bytes());
    // Window attributes: bit 0 = exists, bit 1 = readable, bit 2 = writable.
    info[2] = 0x07;
    // Window granularity in KiB.
    info[4..6].copy_from_slice(&64u16.to_le_bytes());
    // Window size in KiB.
    info[6..8].copy_from_slice(&64u16.to_le_bytes());
    // Window A start segment.
    info[8..10].copy_from_slice(&0xA000u16.to_le_bytes());
    // Window function pointer (unused in this emulation).
    info[10..12].copy_from_slice(&0u16.to_le_bytes());
    info[12..14].copy_from_slice(&0u16.to_le_bytes());
    // Bytes per scanline.
    let bytes_per_line = (vbe_mode.width as u16) * ((vbe_mode.bpp as u16 + 7) / 8);
    info[16..18].copy_from_slice(&bytes_per_line.to_le_bytes());
    // Width and height.
    info[18..20].copy_from_slice(&vbe_mode.width.to_le_bytes());
    info[20..22].copy_from_slice(&vbe_mode.height.to_le_bytes());
    // Character cell width/height.
    info[22] = 8;
    info[23] = 16;
    // Number of memory planes.
    info[24] = 1;
    // Bits per pixel.
    info[25] = vbe_mode.bpp;
    // Number of banks.
    info[26] = 1;
    // Memory model.
    info[27] = vbe_mode.memory_model;
    // Bank size in KiB.
    info[28] = 0;
    // Number of image pages.
    info[29] = 1;
    // Reserved.
    info[30] = 0;
    // Direct color fields.
    if vbe_mode.memory_model == 6 {
        info[31] = vbe_mode.bpp; // bits per primary color
        info[32] = 0; // bit position of red
        info[33] = 8; // bit position of green (for 16bpp)
        info[34] = 16; // bit position of blue (for 24bpp)
    }
    // LFB base address.
    let lfb = fw.config.lfb_address;
    info[40..44].copy_from_slice(&lfb.to_le_bytes());

    for (i, b) in info.iter().enumerate() {
        fw.machine.write_u8(addr + i as u32, *b);
    }
    fw.set_ax(0x004F);
    fw.set_cf(false);
    true
}

/// AX=4F02h: set VBE mode (BX=mode, bit 14 = LFB, bit 15 = preserve).
fn vbe_set_mode<M: Machine>(fw: &mut Firmware<M>) -> bool {
    let mode = fw.bx();
    let use_lfb = mode & 0x4000 != 0;
    let mode_num = mode & 0x3FFF;

    if !VBE_MODES.iter().any(|m| m.mode == mode_num) {
        fw.set_ax(0x014F);
        fw.set_cf(true);
        return true;
    }

    fw.vbe.current_mode = mode_num;
    fw.set_ax(0x004F);
    fw.set_cf(false);
    true
}

/// AX=4F03h: get current VBE mode.
fn vbe_get_mode<M: Machine>(fw: &mut Firmware<M>) -> bool {
    fw.set_bx(fw.vbe.current_mode);
    fw.set_ax(0x004F);
    fw.set_cf(false);
    true
}

/// AX=4F04h: save/restore video state (DL=0 save, 1 restore, CX=mask).
fn vbe_save_restore_state<M: Machine>(fw: &mut Firmware<M>) -> bool {
    let dl = fw.dl();
    let addr = crate::machine::phys(fw.es(), fw.bx() as u32);
    if dl == 0 {
        // Save state.
        let mut state = [0u8; 256];
        state[0] = fw.vbe.current_mode as u8;
        state[1] = (fw.vbe.current_mode >> 8) as u8;
        for (i, b) in state.iter().enumerate() {
            fw.machine.write_u8(addr + i as u32, *b);
        }
        fw.vbe.saved_state = Some(Box::new(state));
    } else if dl == 1 {
        // Restore state.
        if let Some(state) = &fw.vbe.saved_state {
            fw.vbe.current_mode = u16::from_le_bytes([state[0], state[1]]);
        }
    }
    fw.set_ax(0x004F);
    fw.set_cf(false);
    true
}

/// AX=4F05h: window control (BH=0 set, 1 get; BL=window; DX=window number).
fn vbe_window_control<M: Machine>(fw: &mut Firmware<M>) -> bool {
    let bl = fw.bl();
    if bl == 0 {
        // Set window.
        fw.vbe.window = fw.dl();
    } else {
        // Get window.
        fw.set_dl(fw.vbe.window);
    }
    fw.set_ax(0x004F);
    fw.set_cf(false);
    true
}

/// AX=4F06h: set/get scanline length (BL=0 set, 1 get; CX=bytes).
fn vbe_scanline_length<M: Machine>(fw: &mut Firmware<M>) -> bool {
    let bl = fw.bl();
    if bl == 0 {
        // Set scanline length.
        fw.set_cx(fw.cx());
    } else {
        // Get scanline length.
        fw.set_cx(fw.cx());
    }
    fw.set_ax(0x004F);
    fw.set_cf(false);
    true
}

/// AX=4F07h: set/get display start (BL=0 set, 1 get; CX/DX=coords).
fn vbe_display_start<M: Machine>(fw: &mut Firmware<M>) -> bool {
    fw.set_ax(0x004F);
    fw.set_cf(false);
    true
}

/// AX=4F08h: set/get DAC palette format (BL=0 set, 1 get; BH=bits).
fn vbe_dac_palette_format<M: Machine>(fw: &mut Firmware<M>) -> bool {
    let bl = fw.bl();
    if bl == 0 {
        fw.vbe.dac_width = fw.bh();
    } else {
        fw.set_bh(fw.vbe.dac_width);
    }
    fw.set_ax(0x004F);
    fw.set_cf(false);
    true
}

/// AX=4F09h: set/get palette data (BL=0 set, 1 get; ES:DI -> buffer).
fn vbe_palette_data<M: Machine>(fw: &mut Firmware<M>) -> bool {
    fw.set_ax(0x004F);
    fw.set_cf(false);
    true
}
