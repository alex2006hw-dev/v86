//! INT 10h — video services (text mode).
//!
//! Implements the standard INT 10h functions AH=00h–0Fh for
//! text-mode operation on the VGA-compatible display. Graphics
//! mode support is provided through VBE (see `vbe.rs`).

use crate::bda;
use crate::machine::Machine;
use crate::Firmware;

/// Video memory base for color text mode (0xB8000).
pub const VIDEO_COLOR_BASE: u32 = 0xB8000;
/// Video memory base for monochrome text mode (0xB0000).
pub const VIDEO_MONO_BASE: u32 = 0xB0000;

/// Handle an INT 10h call.
pub fn handle_int10<M: Machine>(fw: &mut Firmware<M>) -> bool {
    let ah = fw.ah();
    match ah {
        0x00 => set_video_mode(fw),
        0x01 => set_cursor_type(fw),
        0x02 => set_cursor_position(fw),
        0x03 => read_cursor_position(fw),
        0x05 => set_display_page(fw),
        0x06 => scroll_up(fw),
        0x07 => scroll_down(fw),
        0x08 => read_char_attr(fw),
        0x09 => write_char_attr(fw),
        0x0A => write_char_only(fw),
        0x0E => tty_write(fw),
        0x0F => get_video_mode(fw),
        _ => {
            // VBE functions (AX=4F00h–4F09h) are handled by vbe.rs.
            if fw.ax() >= 0x4F00 && fw.ax() <= 0x4F09 {
                crate::vbe::handle_vbe(fw)
            } else {
                false
            }
        }
    }
}

/// AH=00h: set video mode.
fn set_video_mode<M: Machine>(fw: &mut Firmware<M>) -> bool {
    let mode = fw.al();
    fw.video.mode = mode;
    fw.video.page = 0;
    // Set up BDA fields.
    bda::set_crt_mode(&mut fw.machine, mode);
    match mode {
        0x00 | 0x01 => {
            fw.video.cols = 40;
            fw.video.rows = 25;
        }
        0x02 | 0x03 => {
            fw.video.cols = 80;
            fw.video.rows = 25;
        }
        0x07 => {
            fw.video.cols = 80;
            fw.video.rows = 25;
        }
        _ => {
            fw.video.cols = 80;
            fw.video.rows = 25;
        }
    }
    bda::set_crt_cols(&mut fw.machine, fw.video.cols);
    // Clear the screen (fill with spaces).
    clear_screen(fw);
    // Set cursor to home.
    set_cursor_position(fw);
    fw.set_cf(false);
    true
}

/// AH=01h: set cursor type (CH=start line, CL=end line).
fn set_cursor_type<M: Machine>(fw: &mut Firmware<M>) -> bool {
    let ch = fw.ch();
    let cl = fw.cl();
    // Store cursor mode in BDA 0x460.
    fw.machine.write_u8(0x460, ch);
    fw.machine.write_u8(0x461, cl);
    fw.set_cf(false);
    true
}

/// AH=02h: set cursor position (DH=row, DL=col, BH=page).
fn set_cursor_position<M: Machine>(fw: &mut Firmware<M>) -> bool {
    let row = fw.dh();
    let col = fw.dl();
    let page = fw.bh();
    fw.video.page = page;
    bda::set_cursor_pos(&mut fw.machine, page, row, col);
    fw.set_cf(false);
    true
}

/// AH=03h: read cursor position (BH=page).
/// Returns CH=start, CL=end, DH=row, DL=col.
fn read_cursor_position<M: Machine>(fw: &mut Firmware<M>) -> bool {
    let page = fw.bh();
    let (row, col) = bda::cursor_pos(&mut fw.machine, page);
    let ch = fw.machine.read_u8(0x460);
    let cl = fw.machine.read_u8(0x461);
    fw.set_ch(ch);
    fw.set_cl(cl);
    fw.set_dh(row);
    fw.set_dl(col);
    fw.set_cf(false);
    true
}

/// AH=05h: set display page.
fn set_display_page<M: Machine>(fw: &mut Firmware<M>) -> bool {
    let page = fw.al();
    fw.video.page = page;
    fw.machine.write_u8(0x462, page);
    fw.set_cf(false);
    true
}

/// AH=06h: scroll up (AL=lines, BH=attr, CH/CL=row/col of upper
/// left, DH/DL=row/col of lower right).
fn scroll_up<M: Machine>(fw: &mut Firmware<M>) -> bool {
    let lines = fw.al();
    let attr = fw.bh();
    let row_top = fw.ch() as u32;
    let col_left = fw.cl() as u32;
    let row_bottom = fw.dh() as u32;
    let col_right = fw.dl() as u32;
    scroll_region(fw, lines, attr, row_top, col_left, row_bottom, col_right, true);
    fw.set_cf(false);
    true
}

/// AH=07h: scroll down (same parameters as scroll up).
fn scroll_down<M: Machine>(fw: &mut Firmware<M>) -> bool {
    let lines = fw.al();
    let attr = fw.bh();
    let row_top = fw.ch() as u32;
    let col_left = fw.cl() as u32;
    let row_bottom = fw.dh() as u32;
    let col_right = fw.dl() as u32;
    scroll_region(fw, lines, attr, row_top, col_left, row_bottom, col_right, false);
    fw.set_cf(false);
    true
}

/// Scroll a region of the screen.
fn scroll_region<M: Machine>(
    fw: &mut Firmware<M>,
    lines: u8,
    attr: u8,
    row_top: u32,
    col_left: u32,
    row_bottom: u32,
    col_right: u32,
    up: bool,
) {
    let cols = fw.video.cols as u32;
    let base = VIDEO_COLOR_BASE;
    let lines = lines as u32;

    if lines == 0 {
        // Clear the entire region.
        for row in row_top..=row_bottom {
            for col in col_left..=col_right {
                let addr = base + (row * cols + col) * 2;
                fw.machine.write_u8(addr, b' ');
                fw.machine.write_u8(addr + 1, attr);
            }
        }
        return;
    }

    if up {
        // Move lines up.
        for row in row_top..row_bottom {
            let src_row = row + lines;
            if src_row > row_bottom {
                // Clear this row.
                for col in col_left..=col_right {
                    let addr = base + (row * cols + col) * 2;
                    fw.machine.write_u8(addr, b' ');
                    fw.machine.write_u8(addr + 1, attr);
                }
            } else {
                for col in col_left..=col_right {
                    let src = base + (src_row * cols + col) * 2;
                    let dst = base + (row * cols + col) * 2;
                    let ch = fw.machine.read_u8(src);
                    let at = fw.machine.read_u8(src + 1);
                    fw.machine.write_u8(dst, ch);
                    fw.machine.write_u8(dst + 1, at);
                }
            }
        }
        // Clear the bottom rows.
        for row in row_bottom.saturating_sub(lines - 1)..=row_bottom {
            for col in col_left..=col_right {
                let addr = base + (row * cols + col) * 2;
                fw.machine.write_u8(addr, b' ');
                fw.machine.write_u8(addr + 1, attr);
            }
        }
    } else {
        // Move lines down.
        for row in (row_top..=row_bottom).rev() {
            let src_row = row.saturating_sub(lines);
            if src_row < row_top {
                for col in col_left..=col_right {
                    let addr = base + (row * cols + col) * 2;
                    fw.machine.write_u8(addr, b' ');
                    fw.machine.write_u8(addr + 1, attr);
                }
            } else {
                for col in col_left..=col_right {
                    let src = base + (src_row * cols + col) * 2;
                    let dst = base + (row * cols + col) * 2;
                    let ch = fw.machine.read_u8(src);
                    let at = fw.machine.read_u8(src + 1);
                    fw.machine.write_u8(dst, ch);
                    fw.machine.write_u8(dst + 1, at);
                }
            }
        }
    }
}

/// AH=08h: read character and attribute at cursor (BH=page).
fn read_char_attr<M: Machine>(fw: &mut Firmware<M>) -> bool {
    let page = fw.bh();
    let (row, col) = bda::cursor_pos(&mut fw.machine, page);
    let base = VIDEO_COLOR_BASE;
    let addr = base + (row as u32 * fw.video.cols as u32 + col as u32) * 2;
    let ch = fw.machine.read_u8(addr);
    let at = fw.machine.read_u8(addr + 1);
    fw.set_al(ch);
    fw.set_ah(at);
    fw.set_cf(false);
    true
}

/// AH=09h: write character and attribute (AL=char, BH=page,
/// BL=attr, CX=count).
fn write_char_attr<M: Machine>(fw: &mut Firmware<M>) -> bool {
    let ch = fw.al();
    let page = fw.bh();
    let attr = fw.bl();
    let count = fw.cx() as u32;
    let (row, col) = bda::cursor_pos(&mut fw.machine, page);
    let base = VIDEO_COLOR_BASE;
    let cols = fw.video.cols as u32;
    let mut r = row as u32;
    let mut c = col as u32;
    for _ in 0..count {
        let addr = base + (r * cols + c) * 2;
        fw.machine.write_u8(addr, ch);
        fw.machine.write_u8(addr + 1, attr);
        c += 1;
        if c >= cols {
            c = 0;
            r += 1;
        }
    }
    bda::set_cursor_pos(&mut fw.machine, page, r as u8, c as u8);
    fw.set_cf(false);
    true
}

/// AH=0Ah: write character only (AL=char, BH=page, CX=count).
fn write_char_only<M: Machine>(fw: &mut Firmware<M>) -> bool {
    let ch = fw.al();
    let page = fw.bh();
    let count = fw.cx() as u32;
    let (row, col) = bda::cursor_pos(&mut fw.machine, page);
    let base = VIDEO_COLOR_BASE;
    let cols = fw.video.cols as u32;
    let mut r = row as u32;
    let mut c = col as u32;
    for _ in 0..count {
        let addr = base + (r * cols + c) * 2;
        fw.machine.write_u8(addr, ch);
        c += 1;
        if c >= cols {
            c = 0;
            r += 1;
        }
    }
    bda::set_cursor_pos(&mut fw.machine, page, r as u8, c as u8);
    fw.set_cf(false);
    true
}

/// AH=0Eh: TTY write (AL=char, BH=page, BL=attr for graphics).
fn tty_write<M: Machine>(fw: &mut Firmware<M>) -> bool {
    let ch = fw.al();
    let page = fw.bh();
    let (row, col) = bda::cursor_pos(&mut fw.machine, page);
    let base = VIDEO_COLOR_BASE;
    let cols = fw.video.cols as u32;
    let rows = fw.video.rows as u32;
    let mut r = row as u32;
    let mut c = col as u32;

    match ch {
        0x07 => {
            // Bell — no-op in this emulation.
        }
        0x08 => {
            // Backspace.
            if c > 0 {
                c -= 1;
            }
        }
        0x09 => {
            // Tab.
            c = (c + 8) & !7;
            if c >= cols {
                c = cols - 1;
            }
        }
        0x0A => {
            // Line feed.
            r += 1;
        }
        0x0D => {
            // Carriage return.
            c = 0;
        }
        _ => {
            // Printable character.
            let addr = base + (r * cols + c) * 2;
            fw.machine.write_u8(addr, ch);
            fw.machine.write_u8(addr + 1, 0x07);
            c += 1;
        }
    }

    if c >= cols {
        c = 0;
        r += 1;
    }
    if r >= rows {
        // Scroll up one line.
        scroll_region(fw, 1, 0x07, 0, 0, rows - 1, cols - 1, true);
        r = rows - 1;
    }

    bda::set_cursor_pos(&mut fw.machine, page, r as u8, c as u8);
    fw.set_cf(false);
    true
}

/// AH=0Fh: get video mode.
fn get_video_mode<M: Machine>(fw: &mut Firmware<M>) -> bool {
    fw.set_al(fw.video.mode);
    fw.set_ah(fw.video.cols);
    fw.set_bh(fw.video.page);
    fw.set_cf(false);
    true
}

/// Clear the entire screen.
fn clear_screen<M: Machine>(fw: &mut Firmware<M>) {
    let base = VIDEO_COLOR_BASE;
    let cols = fw.video.cols as u32;
    let rows = fw.video.rows as u32;
    for row in 0..rows {
        for col in 0..cols {
            let addr = base + (row * cols + col) * 2;
            fw.machine.write_u8(addr, b' ');
            fw.machine.write_u8(addr + 1, 0x07);
        }
    }
}
