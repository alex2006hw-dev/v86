//! Abstract interface to the emulated machine.
//!
//! The firmware core is pure logic over this trait; the host
//! emulator (and the unit tests) provide concrete implementations.

/// General-purpose register index (32-bit view).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Reg {
    Eax,
    Ecx,
    Edx,
    Ebx,
    Esp,
    Ebp,
    Esi,
    Edi,
}

/// Segment register index.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum SegReg {
    Es,
    Cs,
    Ss,
    Ds,
    Fs,
    Gs,
}

/// Individual eflags bit.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Flag {
    Cf = 0,
    Pf = 2,
    Af = 4,
    Zf = 6,
    Sf = 7,
    Tf = 8,
    If = 9,
    Df = 10,
    Of = 11,
}

impl Flag {
    pub fn mask(self) -> u32 {
        1u32 << (self as u32)
    }
}

/// Real-time clock reading, as provided by the host.
///
/// The default is a fixed, valid date rather than the epoch so that a
/// host which has not wired up a clock still yields a BIOS that passes
/// `INT 1Ah` sanity checks instead of reporting year 1970.
#[derive(Copy, Clone, Debug)]
pub struct RtcReading {
    pub year: u32,    // full year, e.g. 2026
    pub month: u32,   // 1..=12
    pub day: u32,     // 1..=31
    pub hour: u32,    // 0..=23
    pub minute: u32,  // 0..=59
    pub second: u32,  // 0..=59
    pub day_of_week: u32, // 0=Sunday .. 6=Saturday
}

impl Default for RtcReading {
    fn default() -> RtcReading {
        RtcReading {
            year: 1980,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
            day_of_week: 2, // 1980-01-01 was a Tuesday
        }
    }
}

/// Keyboard scancode event from the host input backend.
#[derive(Copy, Clone, Debug)]
pub struct KeyEvent {
    pub scancode: u8,
    pub pressed: bool,
}

/// Abstract emulated machine.
pub trait Machine {
    // ------------------------------------------------------------------
    // Physical (unsegmented) memory access
    // ------------------------------------------------------------------
    fn read_u8(&mut self, addr: u32) -> u8;
    fn write_u8(&mut self, addr: u32, val: u8);

    fn read_u16(&mut self, addr: u32) -> u16 {
        u16::from_le_bytes([self.read_u8(addr), self.read_u8(addr + 1)])
    }

    fn write_u16(&mut self, addr: u32, val: u16) {
        let bytes = val.to_le_bytes();
        self.write_u8(addr, bytes[0]);
        self.write_u8(addr + 1, bytes[1]);
    }

    fn read_u32(&mut self, addr: u32) -> u32 {
        u32::from_le_bytes([
            self.read_u8(addr),
            self.read_u8(addr + 1),
            self.read_u8(addr + 2),
            self.read_u8(addr + 3),
        ])
    }

    fn read_u64(&mut self, addr: u32) -> u64 {
        let lo = self.read_u32(addr) as u64;
        let hi = self.read_u32(addr + 4) as u64;
        lo | (hi << 32)
    }

    fn write_u32(&mut self, addr: u32, val: u32) {
        let bytes = val.to_le_bytes();
        for i in 0..4usize {
            self.write_u8(addr + i as u32, bytes[i]);
        }
    }

    // ------------------------------------------------------------------
    // CPU registers
    // ------------------------------------------------------------------
    fn read_reg(&self, r: Reg) -> u32;
    fn write_reg(&mut self, r: Reg, v: u32);

    fn read_seg(&self, r: SegReg) -> u16;
    fn write_seg(&mut self, r: SegReg, v: u16);

    fn read_flag(&self, f: Flag) -> bool;
    fn write_flag(&mut self, f: Flag, v: bool);

    fn read_ip(&self) -> u32;
    fn write_ip(&mut self, v: u32);

    /// Convenience: read a byte-sized register view.
    fn read_reg8(&self, r: Reg, high: bool) -> u8 {
        let v = self.read_reg(r) as u16;
        (if high { v >> 8 } else { v & 0xFF }) as u8
    }

    /// Convenience: write a byte-sized register view (AL/AH style).
    fn write_reg8(&mut self, r: Reg, high: bool, v: u8) {
        let cur = self.read_reg(r) as u16;
        let new = if high {
            (cur & 0x00FF) | ((v as u16) << 8)
        } else {
            (cur & 0xFF00) | (v as u16)
        };
        self.write_reg(r, new as u32);
    }

    // ------------------------------------------------------------------
    // Input and clock hooks
    // ------------------------------------------------------------------
    /// Poll for a pending keyboard event (non-blocking).
    fn poll_key(&mut self) -> Option<KeyEvent>;

    /// Yield execution. Used by blocking services (e.g. INT 16h
    /// AH=00h waiting for a key); the host may advance other
    /// subsystems.
    fn yield_cpu(&mut self);

    /// Current wall-clock time for the emulated RTC.
    fn rtc_time(&self) -> RtcReading;

    /// Request a machine reset (used by port 0x92 fast-reset and
    /// triple-fault style resets).
    fn request_reset(&mut self);

    // ------------------------------------------------------------------
    // Hardware acknowledgement
    // ------------------------------------------------------------------
    //
    // A real BIOS services an IRQ by talking to the controller that
    // raised it. Leaving these to the host keeps the firmware from
    // having to know the emulated chipset's register layout.

    /// Acknowledge the floppy controller's interrupt.
    fn acknowledge_floppy(&mut self) {}

    /// Acknowledge the real-time clock by clearing the interrupt-request
    /// flag in CMOS status register C.
    fn acknowledge_rtc(&mut self) {}

    /// Perform a far call to `segment:offset`, as an interrupt service
    /// chaining to the next handler in a chain does. The default
    /// implementation is a no-op so hosts that do not chain need do
    /// nothing.
    fn chain_to(&mut self, _segment: u16, _offset: u16) {}

    /// Pop a 16-bit word from the current real-mode stack (SS:SP) and
    /// advance SP.
    ///
    /// A ROM stub pushes its service id and then traps, so this is how
    /// the firmware learns which service was requested.
    fn pop_stack_u16(&mut self) -> u16;

    /// Read the top of the stack without consuming it. For diagnostics:
    /// a host can show which service a stub asked for without having to
    /// disturb the dispatch.
    fn peek_service_id(&self) -> u32;

    /// The current real-mode stack pointer value, for diagnostics.
    fn peek_stack_pointer(&self) -> u32;

    /// Copy part of a host-owned image into `buf`.
    ///
    /// `image` identifies the image by the number the host gave it,
    /// `byte_offset` is from the start of the image, and `buf.len()` is the
    /// number of bytes wanted. Return `true` on success.
    ///
    /// Only hosts with images too large to copy into memory implement this;
    /// the default says "no", and the drive then reports a failed read.
    fn read_host_image(&mut self, image: u8, byte_offset: u64, buf: &mut [u8]) -> bool {
        let _ = (image, byte_offset, buf);
        false
    }

    /// Copy the carry flag into the FLAGS image the trap interrupt pushed.
    ///
    /// This is not optional. `INT n` pushes FLAGS *before* the handler
    /// runs, and the handler's `IRET` pops that image back — so a service
    /// that sets carry in its own FLAGS would have the change thrown away,
    /// and the caller would see the flags it had on entry. Every BIOS
    /// service that reports status through carry needs this, which is why
    /// real firmware does the same thing with `pushf`/`pop ax`/`push`.
    ///
    /// The default is a no-op for hosts that do not push a frame.
    fn patch_saved_flags(&mut self) {}
}

/// Real-mode physical address of a segmented pointer.
pub fn phys(seg: u16, off: u32) -> u32 {
    (u32::from(seg) << 4).wrapping_add(off)
}

/// Far pointer stored as a 32-bit little-endian value (offset, segment).
pub fn far_ptr(v: u32) -> (u16, u16) {
    ((v >> 16) as u16, (v & 0xFFFF) as u16)
}
