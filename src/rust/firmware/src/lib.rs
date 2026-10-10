//! v86-firmware: permissively-licensed (MIT) PC/XT/AT BIOS firmware
//! for the v86 emulator.
//!
//! This crate implements a modernized PC BIOS derived from PCjs
//! (MIT, Jeff Parsons © 2012–2026) with additional features
//! implemented from public specifications:
//!
//! - INT 13h EDD (Enhanced Disk Drive) services
//! - El Torito ISO 9660 boot
//! - VESA VBE 2.0/3.0
//! - MC146818 CMOS/RTC
//! - E820 memory map
//! - A20 gate control
//!
//! The firmware is pure logic over the `Machine` trait, making it
//! testable on the host without the full emulator.

pub mod backend;
pub mod bda;
pub mod dispatch;
pub mod eltorito;
pub mod ffi;
pub mod int10;
pub mod int13;
pub mod int14;
pub mod int15;
pub mod int16;
pub mod int17;
pub mod int1a;
pub mod ivt;
pub mod machine;
pub mod post;
pub mod status;
pub mod vbe;

pub use dispatch::{bios_interrupt, Config, Firmware};
pub use machine::{Machine, Reg, SegReg, Flag, KeyEvent, RtcReading};
pub use post::run_post;
