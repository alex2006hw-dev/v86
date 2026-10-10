//! Hardware-interrupt services.
//!
//! These are the services behind the ROM stubs for IRQ 0, 1, 6, 8 and
//! 12. They matter more than their size suggests: a guest that has
//! replaced INT 9h with its own handler and then chains to the original
//! pointer expects the BIOS to have done the housekeeping, and a
//! program that never touches INT 16h still depends on the timer tick
//! counter for its delays.

use crate::bda;
use crate::machine::Machine;
use crate::Firmware;

/// IRQ 0 / INT 08h — timer tick.
///
/// Advances the BIOS Data Area tick counter at 0x46C, sets the
/// equipment word's "timer tick pending" and "POST OK" bits, then chains
/// to the guest's INT 1Ch.
pub fn handle_irq0<M: Machine>(fw: &mut Firmware<M>) -> bool {
    let ticks = fw.machine.read_u32(bda::TIMER_TICKS).wrapping_add(1);
    fw.machine.write_u32(bda::TIMER_TICKS, ticks);
    // Some software polls only the high byte at 0x470.
    fw.machine.write_u8(bda::TIMER_TICKS_MIRROR, (ticks >> 8) as u8);

    // Equipment word 0x410: bit 7 "timer tick pending", bit 8 "POST in
    // progress", bit 9 "POST OK". Whatever service consumes the tick
    // clears bit 7; set it here every tick.
    let equip = fw.machine.read_u16(bda::EQUIPMENT);
    fw.machine.write_u16(bda::EQUIPMENT, equip | (1 << 7) | (1 << 9));

    // Chain to INT 1Ch, but only if the guest installed one. POST zeroed
    // the IVT, so an all-zero entry means "no user handler", and calling
    // it would jump into low RAM.
    let (seg, off) = crate::ivt::read_vector(&mut fw.machine, 0x1C);
    if seg != 0 || off != 0 {
        fw.machine.chain_to(seg, off);
    }
    true
}

/// IRQ 1 / INT 09h — keyboard.
///
/// Takes the next scancode the host queued, updates the shift-state and
/// lock-key bytes, and appends the key to the BIOS keyboard buffer.
pub fn handle_irq1<M: Machine>(fw: &mut Firmware<M>) -> bool {
    let key = match fw.machine.poll_key() {
        Some(k) => k,
        // A spurious IRQ 1 (which the controller does emit) must be
        // acknowledged without disturbing the buffer.
        None => return true,
    };

    let extended = key.scancode & 0xE0 == 0xE0;
    let code = key.scancode & 0x7F;
    let released = !key.pressed;

    // An extended key's release produces no entry; only the press does.
    if extended && released {
        return true;
    }

    // BDA 0x417 bits 0-1 Shift, bit 2 Ctrl, bit 3 Alt. Extended Alt is
    // the separate bit 3 of the second byte.
    if extended {
        let alt = fw.machine.read_u8(bda::KBD_SHIFT_FLAGS + 1);
        fw.machine.write_u8(
            bda::KBD_SHIFT_FLAGS + 1,
            if released { alt & !0x08 } else { alt | 0x08 },
        );
    }

    let shift = fw.machine.read_u8(bda::KBD_SHIFT_FLAGS);
    let new_shift = match (code, released) {
        (0x2A | 0x36, _) => {
            if released { shift & !0x03 } else { shift | 0x03 }
        }
        (0x1D, _) => {
            if released { shift & !0x04 } else { shift | 0x04 }
        }
        (0x38, _) => {
            if released { shift & !0x08 } else { shift | 0x08 }
        }
        _ => shift,
    };
    fw.machine.write_u8(bda::KBD_SHIFT_FLAGS, new_shift);

    // Toggle keys, and the LEDs that mirror them (BDA 0x418 bits 0-2).
    if !released {
        let flags = fw.machine.read_u8(bda::KBD_FLAGS);
        let toggled = match code {
            0x3A => flags ^ 0x40,                      // Caps Lock
            0x45 if shift & 0x03 == 0 => flags ^ 0x20, // Num Lock
            0x46 if shift & 0x03 == 0 => flags ^ 0x10, // Scroll Lock
            _ => flags,
        };
        if toggled != flags {
            fw.machine.write_u8(bda::KBD_FLAGS, toggled);
            let led = fw.machine.read_u8(bda::KBD_FLAGS + 1);
            let led = if toggled & 0x40 != 0 { led | 0x02 } else { led & !0x02 };
            let led = if toggled & 0x20 != 0 { led | 0x01 } else { led & !0x01 };
            let led = if toggled & 0x10 != 0 { led | 0x04 } else { led & !0x04 };
            fw.machine.write_u8(bda::KBD_FLAGS + 1, led);
        }
    }

    // Ctrl-Break: reported as a 0x00 prefix so a reader that only tests
    // for a zero ASCII still recognises it.
    let entry = if !released && code == 0x03 && new_shift & 0x04 != 0 {
        let flags = fw.machine.read_u8(bda::KBD_FLAGS);
        fw.machine.write_u8(bda::KBD_FLAGS, flags | 0x80);
        fw.keyboard.push_bda(&mut fw.machine, 0x00);
        0x03
    } else {
        if released {
            let flags = fw.machine.read_u8(bda::KBD_FLAGS);
            fw.machine.write_u8(bda::KBD_FLAGS, flags & !0x80);
        }
        if extended {
            0xE0 | code
        } else {
            code
        }
    };
    fw.keyboard.push_bda(&mut fw.machine, entry);
    true
}

/// IRQ 6 / INT 0Eh — diskette.
///
/// A real BIOS acknowledges the drive and lets the DMA-completion
/// interrupt fire; here the motor status byte is updated so INT 16h
/// AH=01h can report drive activity.
pub fn handle_irq6<M: Machine>(fw: &mut Firmware<M>) -> bool {
    let motor = fw.machine.read_u8(bda::FLOPPY_MOTOR);
    fw.machine.write_u8(bda::FLOPPY_MOTOR, motor & !0x80);
    fw.machine.acknowledge_floppy();
    true
}

/// IRQ 8 / INT 70h — real-time clock.
///
/// Acknowledges the clock by clearing the interrupt-request flag in
/// CMOS status register C.
pub fn handle_irq8<M: Machine>(fw: &mut Firmware<M>) -> bool {
    fw.machine.acknowledge_rtc();
    fw.cmos.update_in_progress = false;
    true
}

/// IRQ 12 / INT 74h — PS/2 mouse.
///
/// A real BIOS acknowledges the controller and chains; the mouse itself
/// is polled through INT 15h C2xxh, so there is no state to maintain
/// beyond letting the request through.
pub fn handle_irq12<M: Machine>(_fw: &mut Firmware<M>) -> bool {
    true
}
