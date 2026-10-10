//! Interrupt Vector Table (physical 0x0000–0x03FF) helpers.

use crate::machine::{Machine, SegReg};

/// Physical base of the real-mode IVT.
pub const IVT_BASE: u32 = 0x0000;
pub const IVT_SIZE: u32 = 0x400;

/// Read the far pointer stored in IVT entry `vector`.
pub fn read_vector(m: &mut dyn Machine, vector: u8) -> (u16, u16) {
    let addr = IVT_BASE + (vector as u32) * 4;
    let offset = m.read_u16(addr);
    let segment = m.read_u16(addr + 2);
    (segment, offset)
}

/// Write a far pointer into IVT entry `vector`.
pub fn write_vector(m: &mut dyn Machine, vector: u8, segment: u16, offset: u16) {
    let addr = IVT_BASE + (vector as u32) * 4;
    m.write_u16(addr, offset);
    m.write_u16(addr + 2, segment);
}

/// Zero the entire IVT (as done during POST).
pub fn clear_ivt(m: &mut dyn Machine) {
    for i in 0..IVT_SIZE {
        m.write_u8(IVT_BASE + i, 0);
    }
}

/// Firmware marker area: BIOS service vectors point into
/// 0xF0000–0xFFFFF. The emulator intercepts real-mode
/// software interrupts whose IVT entry lands in this range
/// and dispatches them to the firmware; if guest software
/// re-vectors the interrupt elsewhere, the guest handler
/// runs (matching real hardware where the vector points
/// into ROM).
pub const FIRMWARE_ROM_BASE: u32 = 0xF0000;
pub const FIRMWARE_ROM_END: u32 = 0xFFFFF;

/// Marker segment/offset used for all firmware service
/// vectors. The values are never executed; they only mark
/// the vector as firmware-owned.
pub const FIRMWARE_MARKER_SEG: u16 = 0xF000;
pub const FIRMWARE_MARKER_OFF: u16 = 0x0000;

/// True when the IVT entry for `vector` points into the
/// firmware marker area.
pub fn is_firmware_vector(m: &mut dyn Machine, vector: u8) -> bool {
    let (segment, offset) = read_vector(m, vector);
    let physical = (u32::from(segment) << 4) + u32::from(offset);
    physical >= FIRMWARE_ROM_BASE && physical <= FIRMWARE_ROM_END
}

/// Install the firmware marker for a BIOS service vector.
pub fn install_firmware_vector(m: &mut dyn Machine, vector: u8) {
    write_vector(m, vector, FIRMWARE_MARKER_SEG, FIRMWARE_MARKER_OFF);
}

/// Segment register used for IVT-relative addressing.
pub const IVT_SEG: SegReg = SegReg::Ds;
