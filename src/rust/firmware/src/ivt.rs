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

/// Segment register used for IVT-relative addressing.
pub const IVT_SEG: SegReg = SegReg::Ds;
