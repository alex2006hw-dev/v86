//! INT 15h — miscellaneous system services.
//!
//! Implements E820 memory map, E801/88h extended memory, A20
//! gate control, and other miscellaneous functions.

use crate::machine::Machine;
use crate::status;
use crate::Firmware;

/// Handle an INT 15h call.
pub fn handle_int15<M: Machine>(fw: &mut Firmware<M>) -> bool {
    let ax = fw.ax();
    match ax {
        0xE820 => e820_memory_map(fw),
        0xE801 => e801_memory_size(fw),
        _ => {
            match fw.ah() {
                0x88 => extended_memory_size(fw),
                0x52 => a20_gate(fw), // AX=5200h/5201h/5202h
                0x24 => a20_gate_24(fw), // AX=2400h/2401h/2402h
                0xC0 => get_config_table(fw),
                0xC1 => get_ebda_segment(fw),
                _ => {
                    // Not implemented.
                    fw.set_cf(true);
                    fw.set_ah(status::INVALID_FUNCTION);
                    true
                }
            }
        }
    }
}

/// AX=E820h: get system memory map (ES:DI -> buffer, EBX=continuation).
fn e820_memory_map<M: Machine>(fw: &mut Firmware<M>) -> bool {
    let addr = crate::machine::phys(fw.es(), fw.di() as u32);
    let ebx = fw.machine.read_reg(crate::machine::Reg::Ebx);

    // Each SMAP entry is a 24-byte descriptor: 8 base, 8 length, 4 type,
    // 4 ACPI extended attributes.
    if ebx as usize >= fw.e820.len() {
        // No more entries. The spec terminates the map with a call that
        // fails: carry set and AH = 04h ("function not supported"), with
        // EBX left alone.
        //
        // Resetting EBX to 0 here instead -- the convention some firmware
        // uses, so a caller can test `while (ebx)` -- made the guard above
        // unreachable, so this function could never report the end of the
        // map at all, and a caller walking it by cursor got the first entry
        // forever.
        fw.set_cf(true);
        fw.set_ah(0x04);
        return true;
    }

    let entry = &fw.e820[ebx as usize];
    let mut buf = [0u8; 24];
    buf[0..8].copy_from_slice(&entry.base.to_le_bytes());
    buf[8..16].copy_from_slice(&entry.length.to_le_bytes());
    buf[16..20].copy_from_slice(&entry.kind.to_le_bytes());
    buf[20..24].copy_from_slice(&0u32.to_le_bytes()); // ACPI extended attributes

    for (i, b) in buf.iter().enumerate() {
        fw.machine.write_u8(addr + i as u32, *b);
    }

    // Update EBX to the next entry index.
    fw.machine.write_reg(crate::machine::Reg::Ebx, ebx + 1);

    fw.set_cf(false);
    fw.set_ah(0);
    true
}

/// AX=E801h: get extended memory size (up to 4 GB).
fn e801_memory_size<M: Machine>(fw: &mut Firmware<M>) -> bool {
    // 1 MiB, in KiB, is where extended memory begins -- the conventional
    // 640 KiB is a subset of it, not the boundary.
    const ONE_MIB_KIB: u32 = 1024;
    let total_kib = fw.config.total_memory_kib;

    let extended_kib = total_kib.saturating_sub(ONE_MIB_KIB);
    // CX = extended memory between 1 MB and 16 MB (in KB).
    let cx = extended_kib.min(15 * 1024);
    // DX = extended memory above 16 MB (in 64 KB blocks).
    let dx = if extended_kib > 15 * 1024 {
        (extended_kib - 15 * 1024) / 64
    } else {
        0
    };
    fw.set_cx(cx as u16);
    fw.set_dx(dx as u16);
    fw.set_ax(cx as u16);
    fw.set_bx(dx as u16);
    fw.set_cf(false);
    true
}

/// AH=88h: get extended memory size (legacy, up to 64 MB).
fn extended_memory_size<M: Machine>(fw: &mut Firmware<M>) -> bool {
    let total_kib = fw.config.total_memory_kib;
    let extended_kib = total_kib.saturating_sub(1024);
    // Legacy function returns up to 64 MB (65535 KB).
    fw.set_ax(extended_kib.min(65535) as u16);
    fw.set_cf(false);
    true
}

/// AX=5200h/5201h/5202h: A20 gate control (alternate).
fn a20_gate<M: Machine>(fw: &mut Firmware<M>) -> bool {
    let al = fw.al();
    match al {
        0x00 => {
            // Query A20 status.
            fw.set_al(if fw.a20_enabled { 1 } else { 0 });
            fw.set_cf(false);
        }
        0x01 => {
            // Enable A20.
            fw.a20_enabled = true;
            fw.set_cf(false);
        }
        0x02 => {
            // Disable A20.
            fw.a20_enabled = false;
            fw.set_cf(false);
        }
        _ => {
            fw.set_cf(true);
            fw.set_ah(status::INVALID_FUNCTION);
        }
    }
    true
}

/// AX=2400h/2401h/2402h: A20 gate control.
fn a20_gate_24<M: Machine>(fw: &mut Firmware<M>) -> bool {
    let al = fw.al();
    match al {
        0x00 => {
            // Disable A20.
            fw.a20_enabled = false;
            fw.set_cf(false);
        }
        0x01 => {
            // Enable A20.
            fw.a20_enabled = true;
            fw.set_cf(false);
        }
        0x02 => {
            // Query A20 status.
            fw.set_al(if fw.a20_enabled { 1 } else { 0 });
            fw.set_cf(false);
        }
        _ => {
            fw.set_cf(true);
            fw.set_ah(status::INVALID_FUNCTION);
        }
    }
    true
}

/// AH=C0h: get configuration table pointer.
fn get_config_table<M: Machine>(fw: &mut Firmware<M>) -> bool {
    // Return 0:0 to indicate no configuration table.
    fw.set_es(0);
    fw.set_bx(0);
    fw.set_cf(false);
    true
}

/// AH=C1h: get EBDA segment.
fn get_ebda_segment<M: Machine>(fw: &mut Firmware<M>) -> bool {
    // Return 0 to indicate no EBDA.
    fw.set_es(0);
    fw.set_cf(false);
    true
}
