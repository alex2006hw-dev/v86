//! Adapter between the v86 emulator and the firmware crate.
//!
//! Implements the `Machine` trait over the emulator's global CPU
//! and memory state, and provides the interrupt/POST hooks that
//! the CPU calls into.

use crate::cpu;
use crate::firmware::backend::{BlockBackend, BlockInfo, DriveKind};
use crate::firmware::dispatch::{self, Config, Firmware};
use crate::firmware::machine::{Flag, KeyEvent, Machine, Reg, RtcReading, SegReg};
use crate::memory;

/// The emulator-backed machine implementation.
pub struct EmulatorMachine {
    keyboard_queue: Vec<KeyEvent>,
}

impl EmulatorMachine {
    pub fn new() -> EmulatorMachine {
        EmulatorMachine {
            keyboard_queue: Vec::new(),
        }
    }
}

impl Machine for EmulatorMachine {
    fn read_u8(&mut self, addr: u32) -> u8 {
        unsafe { memory::read8(addr) as u8 }
    }
    fn write_u8(&mut self, addr: u32, val: u8) {
        unsafe { memory::write8(addr, val as i32) };
    }

    fn read_reg(&self, r: Reg) -> u32 {
        unsafe {
            match r {
                Reg::Eax => cpu::read_reg32(0) as u32,
                Reg::Ecx => cpu::read_reg32(1) as u32,
                Reg::Edx => cpu::read_reg32(2) as u32,
                Reg::Ebx => cpu::read_reg32(3) as u32,
                Reg::Esp => cpu::read_reg32(4) as u32,
                Reg::Ebp => cpu::read_reg32(5) as u32,
                Reg::Esi => cpu::read_reg32(6) as u32,
                Reg::Edi => cpu::read_reg32(7) as u32,
            }
        }
    }
    fn write_reg(&mut self, r: Reg, v: u32) {
        unsafe {
            match r {
                Reg::Eax => cpu::write_reg32(0, v as i32),
                Reg::Ecx => cpu::write_reg32(1, v as i32),
                Reg::Edx => cpu::write_reg32(2, v as i32),
                Reg::Ebx => cpu::write_reg32(3, v as i32),
                Reg::Esp => cpu::write_reg32(4, v as i32),
                Reg::Ebp => cpu::write_reg32(5, v as i32),
                Reg::Esi => cpu::write_reg32(6, v as i32),
                Reg::Edi => cpu::write_reg32(7, v as i32),
            }
        }
    }

    fn read_seg(&self, r: SegReg) -> u16 {
        unsafe {
            match r {
                SegReg::Es => cpu::get_seg(0),
                SegReg::Cs => cpu::get_seg(1),
                SegReg::Ss => cpu::get_seg(2),
                SegReg::Ds => cpu::get_seg(3),
                SegReg::Fs => cpu::get_seg(4),
                SegReg::Gs => cpu::get_seg(5),
            }
        }
    }
    fn write_seg(&mut self, r: SegReg, v: u16) {
        unsafe {
            match r {
                SegReg::Es => cpu::set_seg(0, v),
                SegReg::Cs => cpu::set_seg(1, v),
                SegReg::Ss => cpu::set_seg(2, v),
                SegReg::Ds => cpu::set_seg(3, v),
                SegReg::Fs => cpu::set_seg(4, v),
                SegReg::Gs => cpu::set_seg(5, v),
            }
        }
    }

    fn read_flag(&self, f: Flag) -> bool {
        unsafe {
            let eflags = cpu::get_eflags();
            (eflags & (1 << (f as u32))) != 0
        }
    }
    fn write_flag(&mut self, f: Flag, v: bool) {
        unsafe {
            let mut eflags = cpu::get_eflags();
            if v {
                eflags |= 1 << (f as u32);
            } else {
                eflags &= !(1 << (f as u32));
            }
            cpu::set_eflags(eflags);
        }
    }

    fn read_ip(&self) -> u32 {
        unsafe { cpu::get_instruction_pointer() as u32 }
    }
    fn write_ip(&mut self, v: u32) {
        unsafe { cpu::set_instruction_pointer(v as i32) };
    }

    fn poll_key(&mut self) -> Option<KeyEvent> {
        if self.keyboard_queue.is_empty() {
            None
        } else {
            Some(self.keyboard_queue.remove(0))
        }
    }

    fn yield_cpu(&mut self) {
        // In the emulator, we can't actually yield. The firmware
        // will just spin until a key is available.
    }

    fn rtc_time(&self) -> RtcReading {
        // Return a default RTC reading. The host should override
        // this with actual time.
        RtcReading {
            year: 2026,
            month: 10,
            day: 9,
            hour: 12,
            minute: 0,
            second: 0,
            day_of_week: 5,
        }
    }

    fn request_reset(&mut self) {
        unsafe { cpu::reset_cpu(); }
    }
}

/// The firmware instance, initialized at startup.
pub struct FirmwareAdapter {
    pub firmware: Firmware<EmulatorMachine>,
}

/// Global firmware instance (set at startup).
static mut FIRMWARE: Option<FirmwareAdapter> = None;

/// Initialize the global firmware instance.
pub fn init_firmware(config: Config) {
    unsafe {
        FIRMWARE = Some(FirmwareAdapter::new(config));
    }
}

/// Try to handle a firmware interrupt. Returns true if the
/// firmware handled it.
pub fn try_firmware_interrupt(vector: u8) -> bool {
    unsafe {
        if let Some(ref mut fw) = FIRMWARE {
            fw.try_handle_interrupt(vector)
        } else {
            false
        }
    }
}

/// Run POST (called from reset_cpu).
pub fn run_firmware_post() {
    unsafe {
        if let Some(ref mut fw) = FIRMWARE {
            fw.run_post();
        }
    }
}

impl FirmwareAdapter {
    pub fn new(config: Config) -> FirmwareAdapter {
        let machine = EmulatorMachine::new();
        FirmwareAdapter {
            firmware: Firmware::new(machine, config),
        }
    }

    /// Called by the CPU when a real-mode software interrupt's
    /// IVT entry points into the firmware marker area.
    pub fn try_handle_interrupt(&mut self, vector: u8) -> bool {
        dispatch::bios_interrupt(&mut self.firmware, vector)
    }

    /// Called by the CPU during reset to run POST.
    pub fn run_post(&mut self) {
        dispatch::run_post(&mut self.firmware);
    }

    /// Register a block device with the firmware.
    pub fn register_drive(&mut self, number: u8, backend: Box<dyn BlockBackend>) {
        self.firmware.drives.add(number, backend);
    }

    /// Push a keyboard event into the firmware queue.
    pub fn push_key(&mut self, event: KeyEvent) {
        self.firmware.machine.keyboard_queue.push(event);
    }
}
