//! BIOS Data Area (physical 0x400–0x4FF) — constants and
//! accessors.
//!
//! The BDA is guest memory; the firmware reads and writes it
//! through the `Machine` interface, exactly like real ROM
//! BIOS code does.

use crate::machine::{Machine, SegReg};

pub const BDA_BASE: u32 = 0x0400;

// Serial and parallel port base addresses (words).
pub const COM1: u32 = 0x400;
pub const COM2: u32 = 0x402;
pub const COM3: u32 = 0x404;
pub const COM4: u32 = 0x406;
pub const LPT1: u32 = 0x408;
pub const LPT2: u32 = 0x40A;
pub const LPT3: u32 = 0x40C;

pub const EBDA_SEGMENT: u32 = 0x40E;

/// Equipment word (see `equipment_word()`).
pub const EQUIPMENT: u32 = 0x410;
/// Memory size in KiB (INT 12h).
pub const MEMORY_SIZE: u32 = 0x413;

/// Keyboard shift flags: two bytes at 0x417/0x418.
pub const KBD_SHIFT_FLAGS: u32 = 0x417;
/// Keyboard buffer head/tail (offsets within segment 0x40).
pub const KBD_BUF_HEAD: u32 = 0x41A;
pub const KBD_BUF_TAIL: u32 = 0x41C;
/// Keyboard buffer: 16 words (ASCII, scancode) pairs.
pub const KBD_BUF: u32 = 0x41E;
pub const KBD_BUF_LEN: u32 = 32;

/// Floppy motor status byte.
pub const FLOPPY_MOTOR: u32 = 0x43F;
/// Status of the last diskette operation (INT 13h AH=01h).
pub const LAST_FLOPPY_STATUS: u32 = 0x441;
/// Hard disk: last operation byte (0x474), and status
/// bytes through 0x48F.
pub const LAST_HD_STATUS: u32 = 0x474;

/// CRT mode (e.g. 0x03), columns, and cursor positions.
pub const CRT_MODE: u32 = 0x449;
pub const CRT_COLS: u32 = 0x44A;
/// Cursor positions for 8 pages, 2 bytes (row, col) each.
pub const CRT_CURSOR: u32 = 0x450;
/// 6845 CRTC base address: 0x3B4 (mono) or 0x3D4 (color).
pub const CRT_6845_BASE: u32 = 0x463;

/// Timer tick count since midnight (increments every 55ms).
pub const TIMER_TICKS: u32 = 0x46C;
/// Reset flag: 0x1234 on warm boot.
pub const RESET_FLAG: u32 = 0x472;

/// Equipment word bits.
pub mod equip {
    pub const FLOPPY_INSTALLED: u16 = 1 << 0;
    pub const MATH_COPROCESSOR: u16 = 1 << 1;
    /// Bits 4-5: initial video mode (00 none, 01 40x25
    /// color, 10 80x25 color, 11 80x25 b/w).
    pub const VIDEO_MODE_40X25_COLOR: u16 = 1 << 4;
    pub const VIDEO_MODE_80X25_COLOR: u16 = 2 << 4;
    pub const VIDEO_MODE_80X25_BW: u16 = 3 << 4;
    /// Bits 6-7: number of diskette drives, less 1.
    pub fn floppy_drives(n: u16) -> u16 {
        (n.saturating_sub(1) & 0x03) << 6
    }
    pub const DMA_PRESENT: u16 = 1 << 8;
    /// Bits 9-11: number of RS-232 serial cards.
    pub fn serial_cards(n: u16) -> u16 {
        (n & 0x07) << 9
    }
    pub const GAME_IO: u16 = 1 << 12;
    pub const SERIAL_PRINTER: u16 = 1 << 13;
    /// Bits 14-15: number of printers attached.
    pub fn printers(n: u16) -> u16 {
        (n & 0x03) << 14
    }
}

/// Read the equipment word.
pub fn equipment_word(m: &mut dyn Machine) -> u16 {
    m.read_u16(EQUIPMENT)
}

/// Write the equipment word.
pub fn set_equipment_word(m: &mut dyn Machine, value: u16) {
    m.write_u16(EQUIPMENT, value);
}

/// Read the base memory size in KiB (INT 12h source).
pub fn memory_kib(m: &mut dyn Machine) -> u16 {
    m.read_u16(MEMORY_SIZE)
}

/// Write the base memory size in KiB.
pub fn set_memory_kib(m: &mut dyn Machine, kib: u16) {
    m.write_u16(MEMORY_SIZE, kib);
}

/// Read the keyboard buffer head offset.
pub fn kbd_head(m: &mut dyn Machine) -> u16 {
    m.read_u16(KBD_BUF_HEAD)
}

/// Read the keyboard buffer tail offset.
pub fn kbd_tail(m: &mut dyn Machine) -> u16 {
    m.read_u16(KBD_BUF_TAIL)
}

/// Store a keystroke into the BIOS keyboard buffer
/// (BDA 0x41E–0x43D). Returns false when the buffer is
/// full.
pub fn kbd_store(m: &mut dyn Machine, ascii: u8, scancode: u8) -> bool {
    let head = kbd_head(m);
    let tail = kbd_tail(m);
    let next = if u32::from(tail) + 2 >= KBD_BUF + KBD_BUF_LEN {
        KBD_BUF
    } else {
        (tail + 2) as u32
    };
    if next == u32::from(head) {
        return false; // full
    }
    m.write_u8(BDA_BASE + u32::from(tail), ascii);
    m.write_u8(BDA_BASE + u32::from(tail) + 1, scancode);
    m.write_u16(KBD_BUF_TAIL, next as u16);
    true
}

/// Retrieve the oldest keystroke from the BIOS keyboard
/// buffer. Returns None when empty.
pub fn kbd_fetch(m: &mut dyn Machine) -> Option<(u8, u8)> {
    let head = kbd_head(m);
    let tail = kbd_tail(m);
    if head == tail {
        return None;
    }
    let ascii = m.read_u8(BDA_BASE + u32::from(head));
    let scancode = m.read_u8(BDA_BASE + u32::from(head) + 1);
    let next = if u32::from(head) + 2 >= KBD_BUF + KBD_BUF_LEN {
        KBD_BUF
    } else {
        (head + 2) as u32
    };
    m.write_u16(KBD_BUF_HEAD, next as u16);
    Some((ascii, scancode))
}

/// Reset the keyboard buffer to empty.
pub fn kbd_clear(m: &mut dyn Machine) {
    m.write_u16(KBD_BUF_HEAD, KBD_BUF as u16);
    m.write_u16(KBD_BUF_TAIL, KBD_BUF as u16);
}

/// Read the cursor position (row, col) of a display page.
pub fn cursor_pos(m: &mut dyn Machine, page: u8) -> (u8, u8) {
    let base = CRT_CURSOR + (page as u32) * 2;
    (m.read_u8(base), m.read_u8(base + 1))
}

/// Store the cursor position (row, col) of a display page.
pub fn set_cursor_pos(m: &mut dyn Machine, page: u8, row: u8, col: u8) {
    let base = CRT_CURSOR + (page as u32) * 2;
    m.write_u8(base, row);
    m.write_u8(base + 1, col);
}

/// Read the current video mode byte (BDA 0x449).
pub fn crt_mode(m: &mut dyn Machine) -> u8 {
    m.read_u8(CRT_MODE)
}

/// Store the current video mode byte.
pub fn set_crt_mode(m: &mut dyn Machine, mode: u8) {
    m.write_u8(CRT_MODE, mode);
}

/// Read the number of columns (BDA 0x44A).
pub fn crt_cols(m: &mut dyn Machine) -> u8 {
    m.read_u8(CRT_COLS)
}

/// Store the number of columns.
pub fn set_crt_cols(m: &mut dyn Machine, cols: u8) {
    m.write_u8(CRT_COLS, cols);
}

/// Write the interrupt vector table entry `n` to point at
/// `segment:offset`.
pub fn write_ivt_entry(m: &mut dyn Machine, vector: u8, segment: u16, offset: u16) {
    let addr = (vector as u32) * 4;
    m.write_u16(addr, offset);
    m.write_u16(addr + 2, segment);
}

/// Read the interrupt vector table entry `n`.
pub fn read_ivt_entry(m: &mut dyn Machine, vector: u8) -> (u16, u16) {
    let addr = (vector as u32) * 4;
    (m.read_u16(addr + 2), m.read_u16(addr))
}

/// Convenience: physical address of a BDA field.
pub fn bda_addr(offset: u32) -> u32 {
    BDA_BASE + offset
}

/// Segment used by all BDA accesses.
pub const BDA_SEG: SegReg = SegReg::Ds;
