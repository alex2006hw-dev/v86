//! A very small 16-bit real-mode assembler.
//!
//! The firmware ships as a real ROM image: the reset vector, POST,
//! the hardware-interrupt stubs and the INT-vector stubs are all
//! genuine x86 instructions executed by the emulated CPU. Only the
//! *services* those stubs reach live on the host (see `rom.rs` and
//! `crate::dispatch`), which keeps the guest-visible side of the BIOS
//! — the part software can intercept, chain and re-vector — real
//! hardware behaviour.
//!
//! This module emits the handful of instructions that a BIOS stub
//! actually needs. It is deliberately not a general assembler: there
//! is no expression parser and no type system, just one `emit` call
//! per instruction with the operand encoding done for you.
//!
//! Every method writes into a fixed-size image at an explicit offset,
//! so a ROM is built by jumping around inside it rather than by
//! appending. Out-of-range writes and unresolved labels panic in debug
//! builds and are ignored in release builds, because a BIOS image that
//! silently fails to assemble is far worse than one that fails loudly.
//!
//! Encoding references: Intel SDM Vol. 2, "Instruction Format" and
//! the per-instruction opcode tables.

use std::collections::HashMap;

/// 16-bit general-purpose register, in encoding order.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Reg {
    Ax = 0,
    Cx = 1,
    Dx = 2,
    Bx = 3,
    Sp = 4,
    Bp = 5,
    Si = 6,
    Di = 7,
}

/// 8-bit register, in encoding order.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Reg8 {
    Al = 0,
    Cl = 1,
    Dl = 2,
    Bl = 3,
    Ah = 4,
    Ch = 5,
    Dh = 6,
    Bh = 7,
}

/// Segment register.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Seg {
    Es = 0,
    Cs = 1,
    Ss = 2,
    Ds = 3,
    Fs = 4,
    Gs = 5,
}

/// 16-bit operand: either a register or a memory reference.
///
/// A `Mem` reference is encoded as a 16-bit displacement from the
/// segment base, i.e. what the assembler calls a "moffs" for the
/// accumulator forms and a `disp16` ModRM form otherwise. Real BIOS
/// stubs only ever touch fixed BDA/VGA addresses, so that is enough.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Rm {
    Reg(Reg),
    Mem(u16),
}

fn reg_bits(r: Reg) -> u8 {
    r as u8
}

/// ModRM byte for a register operand form (`mod = 11`).
const MOD_REG: u8 = 0xC0;

fn modrm_reg(rm: u8, reg: u8) -> u8 {
    MOD_REG | ((reg & 7) << 3) | (rm & 7)
}

/// ModRM byte for `[disp16]` (`mod = 00`, `rm = 110`).
fn modrm_disp(reg: u8) -> u8 {
    (reg & 7) << 3 | 6
}

/// An operand size the assembler accepts.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Size {
    Byte,
    Word,
}

/// A resolved label: an offset inside the image.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct Label {
    pub offset: u16,
}

/// Where a fixup has to be written once all labels are known.
struct Fixup {
    /// Offset of the 16-bit field to patch.
    at: u16,
    /// Label it should contain.
    label: String,
    /// Adjustment to add after resolving (for near branches).
    bias: i32,
}

/// A fixed-size ROM image under construction.
pub struct Asm {
    buf: Vec<u8>,
    pos: usize,
    high_water: usize,
    labels: HashMap<String, u16>,
    fixups: Vec<Fixup>,
}

impl Asm {
    /// Create an image of `len` bytes, filled with `fill`.
    ///
    /// Unwritten bytes are `fill` rather than zero so that a stub that
    /// falls off the end of itself is immediately obvious in a hex dump.
    pub fn new(len: usize, fill: u8) -> Asm {
        Asm {
            buf: vec![fill; len],
            pos: 0,
            high_water: 0,
            labels: HashMap::new(),
            fixups: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// The bytes emitted so far, excluding the untouched fill.
    pub fn bytes(&self) -> &[u8] {
        &self.buf[..self.high_water]
    }

    /// The whole image, fill included. This is what a ROM loader wants.
    pub fn image(&self) -> &[u8] {
        &self.buf
    }

    pub fn into_image(self) -> Vec<u8> {
        self.buf
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    /// Move the write cursor to an absolute offset in the image.
    pub fn at(&mut self, offset: usize) {
        assert!(offset <= self.buf.len(), "cursor out of range");
        self.pos = offset;
    }

    /// Move the cursor forward by `n` bytes.
    pub fn skip(&mut self, n: usize) {
        let p = self.pos + n;
        assert!(p <= self.buf.len(), "cursor out of range");
        self.pos = p;
    }

    /// Define a label at the current cursor.
    pub fn label(&mut self, name: &str) {
        assert!(!self.labels.contains_key(name), "duplicate label: {}", name);
        self.labels.insert(name.to_string(), self.pos as u16);
    }

    pub fn has_label(&self, name: &str) -> bool {
        self.labels.contains_key(name)
    }

    /// Resolve a label to its offset, panicking if it is undefined.
    pub fn label_offset(&self, name: &str) -> u16 {
        *self
            .labels
            .get(name)
            .unwrap_or_else(|| panic!("undefined label: {}", name))
    }

    // ------------------------------------------------------------------
    // Raw emission
    // ------------------------------------------------------------------

    fn byte(&mut self, b: u8) {
        if self.pos < self.buf.len() {
            self.buf[self.pos] = b;
            self.high_water = self.high_water.max(self.pos + 1);
        }
        self.pos += 1;
    }

    /// Emit raw bytes verbatim.
    pub fn emit(&mut self, b: &[u8]) {
        for x in b {
            self.byte(*x);
        }
    }

    pub fn db(&mut self, b: u8) {
        self.byte(b)
    }

    pub fn dw(&mut self, w: u16) {
        self.byte(w as u8);
        self.byte((w >> 8) as u8)
    }

    pub fn dd(&mut self, d: u32) {
        self.dw(d as u16);
        self.dw((d >> 16) as u16)
    }

    /// Emit a NUL-terminated string.
    pub fn ascii(&mut self, s: &str) {
        let b = s.as_bytes();
        self.emit(b);
        self.byte(0)
    }

    /// Pad with `fill` until `offset`.
    pub fn align_to(&mut self, offset: usize, fill: u8) {
        while self.pos < offset {
            self.byte(fill);
        }
    }

    /// Write `w` at an absolute offset without moving the cursor.
    pub fn patch_dw(&mut self, offset: usize, w: u16) {
        assert!(offset + 1 < self.buf.len(), "patch out of range");
        self.buf[offset] = w as u8;
        self.buf[offset + 1] = (w >> 8) as u8;
    }

    /// Write `d` at an absolute offset without moving the cursor.
    pub fn patch_dd(&mut self, offset: usize, d: u32) {
        self.patch_dw(offset, d as u16);
        self.patch_dw(offset + 2, (d >> 16) as u16);
    }

    /// Reserve a 16-bit field to be filled in by [`Asm::label_ref`].
    pub fn dw_label(&mut self, name: &str) {
        let at = self.pos;
        self.dw(0);
        self.fixups.push(Fixup {
            at: at as u16,
            label: name.to_string(),
            bias: 0,
        });
    }

    /// Reserve a far pointer (offset:segment) to a label in `segment`.
    pub fn dw_far_label(&mut self, segment: u16, name: &str) {
        let at = self.pos;
        self.dw(0);
        self.dw(segment);
        self.fixups.push(Fixup {
            at: at as u16,
            label: name.to_string(),
            bias: 0,
        });
    }

    // ------------------------------------------------------------------
    // Branch fixups
    // ------------------------------------------------------------------

    /// Reserve an 8-bit displacement to `label`.
    ///
    /// A relative displacement is measured from the end of the
    /// instruction, so the fixup subtracts the width of the field that
    /// follows the opcode.
    fn rel8_fixup(&mut self, label: &str) {
        let at = self.pos;
        self.byte(0);
        self.fixups.push(Fixup {
            at: at as u16,
            label: label.to_string(),
            bias: -((at + 1) as i32),
        });
    }

    fn rel16_fixup(&mut self, label: &str) {
        let at = self.pos;
        self.dw(0);
        self.fixups.push(Fixup {
            at: at as u16,
            label: label.to_string(),
            bias: -((at + 2) as i32),
        });
    }

    // ------------------------------------------------------------------
    // Data movement
    // ------------------------------------------------------------------

    /// `MOV r16, imm16`
    pub fn mov_r_imm(&mut self, r: Reg, imm: u16) {
        self.byte(0xB8 + reg_bits(r));
        self.dw(imm)
    }

    /// `MOV AL, imm8`
    pub fn mov_al_imm(&mut self, imm: u8) {
        self.byte(0xB0);
        self.db(imm)
    }

    /// `MOV AH, imm8`
    pub fn mov_ah_imm(&mut self, imm: u8) {
        self.byte(0xB4);
        self.db(imm)
    }

    /// `MOV r16, r/m16`
    pub fn mov_r_rm(&mut self, r: Reg, rm: Rm) {
        self.byte(0x8B);
        match rm {
            Rm::Reg(s) => {
                self.byte(modrm_reg(reg_bits(s), reg_bits(r)));
            }
            Rm::Mem(d) => {
                self.byte(modrm_disp(reg_bits(r)));
                self.dw(d);
            }
        }
    }

    /// `MOV r/m16, r16`
    pub fn mov_rm_r(&mut self, rm: Rm, r: Reg) {
        self.byte(0x89);
        match rm {
            Rm::Reg(s) => {
                self.byte(modrm_reg(reg_bits(s), reg_bits(r)));
            }
            Rm::Mem(d) => {
                self.byte(modrm_disp(reg_bits(r)));
                self.dw(d);
            }
        }
    }

    /// `MOV AX, [disp16]` (the A1 moffs form).
    pub fn mov_ax_mem(&mut self, disp: u16) {
        self.byte(0xA1);
        self.dw(disp)
    }

    /// `MOV [disp16], AX` (the A3 moffs form).
    pub fn mov_mem_ax(&mut self, disp: u16) {
        self.byte(0xA3);
        self.dw(disp)
    }

    /// `MOV AL, [disp16]`
    pub fn mov_al_mem(&mut self, disp: u16) {
        self.byte(0xA0);
        self.dw(disp)
    }

    /// `MOV [disp16], AL`
    pub fn mov_mem_al(&mut self, disp: u16) {
        self.byte(0xA2);
        self.dw(disp)
    }

    /// `MOV r/m8, imm8`
    pub fn mov_rm8_imm(&mut self, rm: Rm, imm: u8) {
        self.byte(0xC6);
        match rm {
            Rm::Mem(d) => {
                self.byte(modrm_disp(0));
                self.dw(d);
            }
            Rm::Reg(_) => panic!("mov_rm8_imm expects a memory operand"),
        }
        self.db(imm)
    }

    /// `MOV sreg, r16`
    pub fn mov_seg_r(&mut self, s: Seg, r: Reg) {
        self.byte(0x8E);
        self.byte(modrm_reg(reg_bits(r), s as u8))
    }

    /// `MOV r16, sreg`
    pub fn mov_r_seg(&mut self, r: Reg, s: Seg) {
        self.byte(0x8C);
        self.byte(modrm_reg(reg_bits(r), s as u8))
    }

    /// `LEA r16, [disp16]`
    pub fn lea_r_mem(&mut self, r: Reg, disp: u16) {
        self.byte(0x8D);
        self.byte(modrm_disp(reg_bits(r)));
        self.dw(disp)
    }

    /// `LDS r16, m16:16`
    pub fn lds_r(&mut self, r: Reg, disp: u16) {
        self.byte(0xC5);
        self.byte(modrm_disp(reg_bits(r)));
        self.dw(disp)
    }

    /// `LES r16, m16:16`
    pub fn les_r(&mut self, r: Reg, disp: u16) {
        self.byte(0xC4);
        self.byte(modrm_disp(reg_bits(r)));
        self.dw(disp)
    }

    /// `XCHG AX, r16`
    pub fn xchg_ax(&mut self, r: Reg) {
        self.byte(0x90 + reg_bits(r))
    }

    // ------------------------------------------------------------------
    // Stack
    // ------------------------------------------------------------------

    pub fn push_r(&mut self, r: Reg) {
        self.byte(0x50 + reg_bits(r))
    }

    pub fn pop_r(&mut self, r: Reg) {
        self.byte(0x58 + reg_bits(r))
    }

    /// `PUSH imm16`
    pub fn push_imm(&mut self, imm: u16) {
        self.byte(0x68);
        self.dw(imm)
    }

    pub fn push_seg(&mut self, s: Seg) {
        // 06 /r for ES,CS,SS,DS,FS,GS in that encoding order.
        self.byte([0x06, 0x0E, 0x16, 0x1E, 0x0F, 0x1F][s as usize])
    }

    pub fn pop_seg(&mut self, s: Seg) {
        self.byte([0x07, 0x17, 0x1F, 0x1F, 0x1F, 0x1F][s as usize])
    }

    pub fn pushf(&mut self) {
        self.byte(0x9C)
    }

    pub fn popf(&mut self) {
        self.byte(0x9D)
    }

    pub fn pusha(&mut self) {
        self.byte(0x60)
    }

    pub fn popa(&mut self) {
        self.byte(0x61)
    }

    // ------------------------------------------------------------------
    // Arithmetic / logic (AL/AX forms plus a generic r/m16 group)
    // ------------------------------------------------------------------

    /// The ALU group opcode extension (`/r` digit) for an operation.
    fn alu_ext(op: Alu) -> u8 {
        match op {
            Alu::Add => 0,
            Alu::Or => 1,
            Alu::Adc => 2,
            Alu::Sbb => 3,
            Alu::And => 4,
            Alu::Sub => 5,
            Alu::Xor => 6,
            Alu::Cmp => 7,
        }
    }

    /// Base opcode of the `OP r, r/m` group (0x00-0x3F). Each operation
    /// occupies two opcodes: `r, r/m` then `r/m, r`.
    fn alu_group_base(op: Alu) -> u8 {
        Self::alu_ext(op) << 3
    }

    /// `OP r/m16, imm16`
    pub fn alu_rm_imm(&mut self, op: Alu, rm: Rm, imm: u16) {
        self.byte(0x81);
        match rm {
            Rm::Reg(s) => {
                self.byte(modrm_reg(reg_bits(s), Self::alu_ext(op)));
            }
            Rm::Mem(d) => {
                self.byte(modrm_disp(Self::alu_ext(op)));
                self.dw(d);
            }
        }
        self.dw(imm)
    }

    /// `OP r16, r/m16`
    pub fn alu_r_rm(&mut self, op: Alu, r: Reg, rm: Rm) {
        self.byte(Self::alu_group_base(op) | 0x03);
        match rm {
            Rm::Reg(s) => {
                self.byte(modrm_reg(reg_bits(s), reg_bits(r)));
            }
            Rm::Mem(d) => {
                self.byte(modrm_disp(reg_bits(r)));
                self.dw(d);
            }
        }
    }

    /// `OP r/m16, r16`
    pub fn alu_rm_r(&mut self, op: Alu, rm: Rm, r: Reg) {
        self.byte(Self::alu_group_base(op) | 0x01);
        match rm {
            Rm::Reg(s) => {
                self.byte(modrm_reg(reg_bits(s), reg_bits(r)));
            }
            Rm::Mem(d) => {
                self.byte(modrm_disp(reg_bits(r)));
                self.dw(d);
            }
        }
    }

    pub fn xor_ax_ax(&mut self) {
        self.alu_rm_r(Alu::Xor, Rm::Reg(Reg::Ax), Reg::Ax)
    }

    pub fn cmp_al_imm(&mut self, imm: u8) {
        self.byte(0x3C);
        self.db(imm)
    }

    pub fn test_al_imm(&mut self, imm: u8) {
        self.byte(0xA8);
        self.db(imm)
    }

    pub fn test_ax_imm(&mut self, imm: u16) {
        self.byte(0xF7);
        self.byte(modrm_disp(0));
        self.dw(imm)
    }

    pub fn inc_r(&mut self, r: Reg) {
        self.byte(0x40 + reg_bits(r))
    }

    pub fn dec_r(&mut self, r: Reg) {
        self.byte(0x48 + reg_bits(r))
    }

    /// `ADD r/m16, imm8` (sign-extended) — the short form of `alu_rm_imm`.
    pub fn add_rm_imm8(&mut self, rm: Rm, imm: i8) {
        self.byte(0x83);
        match rm {
            Rm::Reg(s) => {
                self.byte(modrm_reg(reg_bits(s), 0));
            }
            Rm::Mem(d) => {
                self.byte(modrm_disp(0));
                self.dw(d);
            }
        }
        self.db(imm as u8)
    }

    /// `SHL/SHR/SAR r/m16, imm8`
    pub fn shift_rm_imm8(&mut self, sh: Shift, rm: Rm, imm: u8) {
        self.byte(0xC1);
        match rm {
            Rm::Reg(s) => {
                self.byte(modrm_reg(reg_bits(s), sh.ext()));
            }
            Rm::Mem(d) => {
                self.byte(modrm_disp(sh.ext()));
                self.dw(d);
            }
        }
        self.db(imm)
    }

    // ------------------------------------------------------------------
    // Control flow
    // ------------------------------------------------------------------

    pub fn jmp_rel8(&mut self, label: &str) {
        self.byte(0xEB);
        self.rel8_fixup(label)
    }

    pub fn jmp_rel16(&mut self, label: &str) {
        self.byte(0xE9);
        self.rel16_fixup(label)
    }

    /// `JMP m16:16` with a resolved far label.
    pub fn jmp_far(&mut self, segment: u16, label: &str) {
        self.byte(0xEA);
        self.dw_label(label);
        self.dw(segment)
    }

    /// `JMP m16:16` for a label in this same image (`base_segment`).
    pub fn jmp_far_base(&mut self, base_segment: u16, label: &str) {
        self.jmp_far(base_segment, label)
    }

    pub fn call_rel16(&mut self, label: &str) {
        self.byte(0xE8);
        self.rel16_fixup(label)
    }

    pub fn jcc_rel8(&mut self, cc: Cond, label: &str) {
        self.byte(0x70 + cc as u8);
        self.rel8_fixup(label)
    }

    pub fn jcxz(&mut self, label: &str) {
        self.byte(0xE3);
        self.rel8_fixup(label)
    }

    pub fn loop_(&mut self, label: &str) {
        self.byte(0xE2);
        self.rel8_fixup(label)
    }

    pub fn ret(&mut self) {
        self.byte(0xC3)
    }

    pub fn retf(&mut self) {
        self.byte(0xCB)
    }

    // ------------------------------------------------------------------
    // Interrupts and I/O
    // ------------------------------------------------------------------

    pub fn int(&mut self, n: u8) {
        self.byte(0xCD);
        self.db(n)
    }

    pub fn iret(&mut self) {
        self.byte(0xCF)
    }

    pub fn in_al_dx(&mut self) {
        self.byte(0xEC)
    }

    pub fn in_ax_dx(&mut self) {
        self.byte(0xED)
    }

    pub fn in_al_imm(&mut self, port: u8) {
        self.byte(0xE4);
        self.db(port)
    }

    pub fn in_ax_imm(&mut self, port: u8) {
        self.byte(0xE5);
        self.db(port)
    }

    pub fn out_dx_al(&mut self) {
        self.byte(0xEE)
    }

    pub fn out_dx_ax(&mut self) {
        self.byte(0xEF)
    }

    pub fn out_imm_al(&mut self, port: u8) {
        self.byte(0xE6);
        self.db(port)
    }

    pub fn out_imm_ax(&mut self, port: u8) {
        self.byte(0xE7);
        self.db(port)
    }

    // ------------------------------------------------------------------
    // Flags and misc
    // ------------------------------------------------------------------

    pub fn cli(&mut self) {
        self.byte(0xFA)
    }

    pub fn sti(&mut self) {
        self.byte(0xFB)
    }

    pub fn cld(&mut self) {
        self.byte(0xFC)
    }

    pub fn std(&mut self) {
        self.byte(0xFD)
    }

    pub fn nop(&mut self) {
        self.byte(0x90)
    }

    pub fn hlt(&mut self) {
        self.byte(0xF4)
    }

    // ------------------------------------------------------------------
    // Finalisation
    // ------------------------------------------------------------------

    /// Resolve every pending label reference.
    ///
    /// Must be called before the image is used; an image with
    /// unresolved references would jump somewhere arbitrary.
    pub fn link(&mut self) {
        let fixups = std::mem::take(&mut self.fixups);
        for f in fixups {
            let target = self.label_offset(&f.label) as i32 + f.bias;
            let target = target as u16;
            self.patch_dw(f.at as usize, target);
        }
    }

    /// Link and return the whole image, fill included.
    pub fn finish(mut self) -> Vec<u8> {
        self.link();
        self.buf
    }
}

/// Arithmetic/logic operations of the 0x00–0x3F ALU group.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Alu {
    Add,
    Or,
    Adc,
    Sbb,
    And,
    Sub,
    Xor,
    Cmp,
}

/// Shift/rotate operations of the 0xC0/0xC1 group.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Shift {
    Rol,
    Ror,
    Rcl,
    Rcr,
    Shl,
    Shr,
    Sal,
    Sar,
}

impl Shift {
    fn ext(self) -> u8 {
        match self {
            Shift::Rol => 0,
            Shift::Ror => 1,
            Shift::Rcl => 2,
            Shift::Rcr => 3,
            Shift::Shl | Shift::Sal => 4,
            Shift::Shr => 5,
            Shift::Sar => 7,
        }
    }
}

/// Condition codes for `Jcc`.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Cond {
    Overflow = 0x0,
    NotOverflow = 0x1,
    Below = 0x2,
    AboveOrEqual = 0x3,
    Equal = 0x4,
    NotEqual = 0x5,
    BelowOrEqual = 0x6,
    Above = 0x7,
    Sign = 0x8,
    NotSign = 0x9,
    ParityEven = 0xA,
    ParityOdd = 0xB,
    Less = 0xC,
    GreaterOrEqual = 0xD,
    LessOrEqual = 0xE,
    Greater = 0xF,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_basic_moves() {
        let mut a = Asm::new(16, 0xFF);
        a.mov_r_imm(Reg::Ax, 0x1234);
        a.mov_al_imm(0x56);
        a.push_r(Reg::Bx);
        a.push_imm(0x0010);
        assert_eq!(a.bytes(), &[0xB8, 0x34, 0x12, 0xB0, 0x56, 0x53, 0x68, 0x10, 0x00]);
    }

    #[test]
    fn encodes_memory_operands() {
        let mut a = Asm::new(16, 0xFF);
        a.mov_ax_mem(0x046C);
        a.mov_mem_ax(0x046C);
        a.mov_rm_r(Rm::Mem(0x0449), Reg::Ax);
        a.mov_r_rm(Reg::Ax, Rm::Mem(0x044A));
        assert_eq!(
            a.bytes(),
            &[
                0xA1, 0x6C, 0x04, // mov ax,[046c]
                0xA3, 0x6C, 0x04, // mov [046c],ax
                0x89, 0x06, 0x49, 0x04, // mov [0449],ax
                0x8B, 0x06, 0x4A, 0x04, // mov ax,[044a]
            ]
        );
    }

    #[test]
    fn encodes_segment_registers() {
        let mut a = Asm::new(16, 0xFF);
        a.mov_seg_r(Seg::Ds, Reg::Ax);
        a.mov_seg_r(Seg::Es, Reg::Ax);
        a.mov_seg_r(Seg::Ss, Reg::Ax);
        a.mov_seg_r(Seg::Cs, Reg::Ax);
        // 8E /r: mod=11, reg field carries the segment selector.
        assert_eq!(&a.bytes()[0..2], &[0x8E, 0xD8]);
        assert_eq!(&a.bytes()[2..4], &[0x8E, 0xC0]);
        assert_eq!(&a.bytes()[4..6], &[0x8E, 0xD0]);
        assert_eq!(&a.bytes()[6..8], &[0x8E, 0xC8]);
    }

    #[test]
    fn encodes_interrupts_and_iret() {
        let mut a = Asm::new(8, 0xFF);
        a.int(0x66);
        a.iret();
        a.hlt();
        assert_eq!(a.bytes(), &[0xCD, 0x66, 0xCF, 0xF4]);
    }

    #[test]
    fn links_relative_branches() {
        let mut a = Asm::new(32, 0xFF);
        a.jmp_rel8("target");
        a.nop();
        a.nop();
        a.label("target");
        a.nop();
        a.link();
        // Layout: jmp(2) nop nop <target>. From the end of the jmp at
        // offset 2 to offset 4 is a displacement of +2.
        assert_eq!(&a.bytes()[0..2], &[0xEB, 0x02]);
        assert_eq!(a.bytes()[4], 0x90);
    }

    #[test]
    fn links_far_pointers() {
        let mut a = Asm::new(64, 0xFF);
        a.jmp_far(0xF000, "somewhere");
        a.at(32);
        a.label("somewhere");
        a.retf();
        a.link();
        // EA off16 seg16 -> offset 0x0020, segment 0xF000
        assert_eq!(&a.bytes()[0..5], &[0xEA, 0x20, 0x00, 0x00, 0xF0]);
    }

    #[test]
    fn patch_does_not_move_cursor() {
        let mut a = Asm::new(16, 0xFF);
        a.at(0);
        a.dw(0x1234);
        let p = a.pos();
        a.patch_dw(8, 0xABCD);
        assert_eq!(a.pos(), p);
        // patch_dw writes past the emitted prefix, so read the image.
        assert_eq!(&a.image()[8..10], &[0xCD, 0xAB]);
    }

    #[test]
    fn alu_group_opcodes_differ_per_operation() {
        let mut a = Asm::new(32, 0xFF);
        a.alu_r_rm(Alu::Add, Reg::Ax, Rm::Reg(Reg::Bx));   // 03 D8
        a.alu_r_rm(Alu::Sub, Reg::Ax, Rm::Mem(0x0449));     // 2B 06 49 04
        a.alu_rm_r(Alu::And, Rm::Reg(Reg::Cx), Reg::Ax);   // 21 C8
        a.alu_rm_r(Alu::Cmp, Rm::Mem(0x046C), Reg::Dx);     // 39 16 6C 04
        a.alu_rm_imm(Alu::Or, Rm::Reg(Reg::Ax), 0x0F);      // 81 C4 0F 00
        a.alu_rm_imm(Alu::Xor, Rm::Mem(0x0449), 0x20);      // 81 36 49 04 20 00
        // In `OP r, r/m` the reg field is the destination and rm the
        // source; in `OP r/m, r` it is the other way round. Both must
        // select the right one, which is what this pins down.
        assert_eq!(
            a.bytes(),
            &[
                0x03, 0xC3, // add ax, bx      (reg=ax, rm=bx)
                0x2B, 0x06, 0x49, 0x04, // sub ax, [0449] (reg=ax, rm=disp16)
                0x21, 0xC1, // and cx, ax      (rm=cx, reg=ax)
                0x39, 0x16, 0x6C, 0x04, // cmp [046c], dx (rm=disp16, reg=dx)
                0x81, 0xC8, 0x0F, 0x00, // or ax, 0x000F  (/1)
                0x81, 0x36, 0x49, 0x04, 0x20, 0x00, // xor [0449], 0x0020 (/6)
            ]
        );
    }

    #[test]
    fn xor_ax_ax_is_31_c0() {
        let mut a = Asm::new(4, 0xFF);
        a.xor_ax_ax();
        assert_eq!(a.bytes(), &[0x31, 0xC0]);
    }

    #[test]
    fn duplicate_labels_are_rejected() {
        let mut a = Asm::new(16, 0xFF);
        a.label("x");
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut b = a;
            b.label("x");
        }));
        assert!(r.is_err());
    }
}
