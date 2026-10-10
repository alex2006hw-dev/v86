//! INT 1Ah — RTC/CMOS services.
//!
//! Implements the MC146818-compatible RTC/CMOS register file and
//! the standard INT 1Ah BIOS functions for time, date, and
//! alarm access.

use crate::machine::Machine;
use crate::status;
use crate::Firmware;

/// CMOS address port.
pub const CMOS_ADDR: u16 = 0x70;
/// CMOS data port.
pub const CMOS_DATA: u16 = 0x71;

/// CMOS register offsets.
pub mod reg {
    pub const SECONDS: u8 = 0x00;
    pub const SECONDS_ALARM: u8 = 0x01;
    pub const MINUTES: u8 = 0x02;
    pub const MINUTES_ALARM: u8 = 0x03;
    pub const HOURS: u8 = 0x04;
    pub const HOURS_ALARM: u8 = 0x05;
    pub const DAY_OF_WEEK: u8 = 0x06;
    pub const DAY_OF_MONTH: u8 = 0x07;
    pub const MONTH: u8 = 0x08;
    pub const YEAR: u8 = 0x09;
    pub const CENTURY: u8 = 0x32;
    pub const STATUS_A: u8 = 0x0A;
    pub const STATUS_B: u8 = 0x0B;
    pub const STATUS_C: u8 = 0x0C;
    pub const STATUS_D: u8 = 0x0D;
    pub const DIAGNOSTIC: u8 = 0x0E;
    pub const SHUTDOWN: u8 = 0x0F;
    pub const FLOPPY_TYPE: u8 = 0x10;
    pub const EQUIPMENT: u8 = 0x14;
    pub const MEMORY_LOW: u8 = 0x15;
    pub const MEMORY_HIGH: u8 = 0x16;
    pub const MEMORY_EXT_LOW: u8 = 0x17;
    pub const MEMORY_EXT_HIGH: u8 = 0x18;
    pub const CENTURY_AT: u8 = 0x32;
}

/// Handle an INT 1Ah call.
pub fn handle_int1a<M: Machine>(fw: &mut Firmware<M>) -> bool {
    let ah = fw.ah();
    match ah {
        0x00 => read_rtc_time(fw),
        0x01 => set_rtc_time(fw),
        0x02 => read_rtc_date(fw),
        0x03 => set_rtc_date(fw),
        0x04 => read_alarm(fw),
        0x05 => set_alarm(fw),
        0x06 => set_alarm_alt(fw),
        0x07 => reset_alarm(fw),
        0x08 => set_rtc_alt(fw),
        0x09 => read_rtc_alt(fw),
        0x0A => read_day_count(fw),
        0x0B => set_day_count(fw),
        0x80 => set_rtc_alt2(fw),
        0x88 => extended_memory_size(fw),
        _ => {
            fw.set_cf(true);
            fw.set_ah(status::INVALID_FUNCTION);
            true
        }
    }
}

/// Read the RTC time from the machine.
fn read_rtc_time_from_machine<M: Machine>(fw: &Firmware<M>) -> (u8, u8, u8, u8) {
    let rtc = fw.machine.rtc_time();
    let seconds = bcd_encode(rtc.second as u8);
    let minutes = bcd_encode(rtc.minute as u8);
    let hours = bcd_encode(rtc.hour as u8);
    let day_of_week = bcd_encode(rtc.day_of_week as u8);
    (seconds, minutes, hours, day_of_week)
}

/// Read the RTC date from the machine.
fn read_rtc_date_from_machine<M: Machine>(fw: &Firmware<M>) -> (u8, u8, u8, u8) {
    let rtc = fw.machine.rtc_time();
    let day = bcd_encode(rtc.day as u8);
    let month = bcd_encode(rtc.month as u8);
    let year = bcd_encode((rtc.year % 100) as u8);
    let century = bcd_encode((rtc.year / 100) as u8);
    (day, month, year, century)
}

/// AH=00h: read RTC time (CH=seconds, CL=minutes, DH=hours, DL=DST).
fn read_rtc_time<M: Machine>(fw: &mut Firmware<M>) -> bool {
    let (seconds, minutes, hours, _) = read_rtc_time_from_machine(fw);
    fw.set_ch(seconds);
    fw.set_cl(minutes);
    fw.set_dh(hours);
    fw.set_dl(0); // DST not supported
    fw.set_cf(false);
    true
}

/// AH=01h: set RTC time (CH=seconds, CL=minutes, DH=hours, DL=DST).
fn set_rtc_time<M: Machine>(fw: &mut Firmware<M>) -> bool {
    // In this emulation, we don't actually set the host RTC.
    // Just acknowledge.
    fw.set_cf(false);
    true
}

/// AH=02h: read RTC date (CH=century, CL=year, DH=month, DL=day).
fn read_rtc_date<M: Machine>(fw: &mut Firmware<M>) -> bool {
    let (day, month, year, century) = read_rtc_date_from_machine(fw);
    fw.set_ch(century);
    fw.set_cl(year);
    fw.set_dh(month);
    fw.set_dl(day);
    fw.set_cf(false);
    true
}

/// AH=03h: set RTC date (CH=century, CL=year, DH=month, DL=day).
fn set_rtc_date<M: Machine>(fw: &mut Firmware<M>) -> bool {
    fw.set_cf(false);
    true
}

/// AH=04h: read alarm (CH=seconds, CL=minutes, DH=hours).
fn read_alarm<M: Machine>(fw: &mut Firmware<M>) -> bool {
    fw.set_ch(0);
    fw.set_cl(0);
    fw.set_dh(0);
    fw.set_cf(false);
    true
}

/// AH=05h: set alarm (CH=seconds, CL=minutes, DH=hours).
fn set_alarm<M: Machine>(fw: &mut Firmware<M>) -> bool {
    fw.set_cf(false);
    true
}

/// AH=06h: set alarm (alternate).
fn set_alarm_alt<M: Machine>(fw: &mut Firmware<M>) -> bool {
    fw.set_cf(false);
    true
}

/// AH=07h: reset alarm.
fn reset_alarm<M: Machine>(fw: &mut Firmware<M>) -> bool {
    fw.set_cf(false);
    true
}

/// AH=08h: set RTC (alternate).
fn set_rtc_alt<M: Machine>(fw: &mut Firmware<M>) -> bool {
    fw.set_cf(false);
    true
}

/// AH=09h: read RTC (alternate).
fn read_rtc_alt<M: Machine>(fw: &mut Firmware<M>) -> bool {
    read_rtc_time(fw)
}

/// AH=0Ah: read day count since 1/1/1980 (CX=year, DH=month, DL=day).
fn read_day_count<M: Machine>(fw: &mut Firmware<M>) -> bool {
    let rtc = fw.machine.rtc_time();
    fw.set_cx(rtc.year as u16);
    fw.set_dh(rtc.month as u8);
    fw.set_dl(rtc.day as u8);
    fw.set_cf(false);
    true
}

/// AH=0Bh: set day count (CX=year, DH=month, DL=day).
fn set_day_count<M: Machine>(fw: &mut Firmware<M>) -> bool {
    fw.set_cf(false);
    true
}

/// AH=80h: set RTC (alternate 2).
fn set_rtc_alt2<M: Machine>(fw: &mut Firmware<M>) -> bool {
    fw.set_cf(false);
    true
}

/// AH=88h: extended memory size (same as INT 15h AH=88h).
fn extended_memory_size<M: Machine>(fw: &mut Firmware<M>) -> bool {
    let mem_kib = fw.config.memory_kib as u32;
    let extended_kib = if mem_kib > 640 {
        mem_kib - 640
    } else {
        0
    };
    fw.set_ax(extended_kib.min(65535) as u16);
    fw.set_cf(false);
    true
}

/// Encode a value as BCD.
fn bcd_encode(v: u8) -> u8 {
    ((v / 10) << 4) | (v % 10)
}

/// Decode a BCD value.
pub fn bcd_decode(v: u8) -> u8 {
    ((v >> 4) * 10) + (v & 0x0F)
}
