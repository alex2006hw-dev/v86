//! INT 14h — serial port services.
//!
//! Implements the standard BIOS serial port functions for the
//! 16550A UART. The actual I/O is delegated to the host machine.

use crate::bda;
use crate::machine::Machine;
use crate::status;
use crate::Firmware;

/// COM port base addresses from BDA.
pub const COM_PORTS: [u32; 4] = [bda::COM1, bda::COM2, bda::COM3, bda::COM4];

/// Handle an INT 14h call.
pub fn handle_int14<M: Machine>(fw: &mut Firmware<M>) -> bool {
    let ah = fw.ah();
    match ah {
        0x00 => init_serial(fw),
        0x01 => send_char(fw),
        0x02 => receive_char(fw),
        0x03 => get_status(fw),
        _ => {
            fw.set_cf(true);
            fw.set_ah(status::INVALID_FUNCTION);
            true
        }
    }
}

/// AH=00h: initialize serial port (AL=parameters, DX=port).
fn init_serial<M: Machine>(fw: &mut Firmware<M>) -> bool {
    // Just acknowledge — the host handles the actual UART.
    fw.set_ax(0); // success
    fw.set_cf(false);
    true
}

/// AH=01h: send character (AL=char, DX=port).
fn send_char<M: Machine>(fw: &mut Firmware<M>) -> bool {
    // In this emulation, we just acknowledge.
    // The host would write to the actual serial port.
    fw.set_ah(0); // success
    fw.set_cf(false);
    true
}

/// AH=02h: receive character (DX=port).
fn receive_char<M: Machine>(fw: &mut Firmware<M>) -> bool {
    // No data available.
    fw.set_ah(0x80); // timeout
    fw.set_cf(true);
    true
}

/// AH=03h: get serial port status (DX=port).
fn get_status<M: Machine>(fw: &mut Firmware<M>) -> bool {
    // Return "not busy, data not ready" status.
    fw.set_ah(0x60); // THR empty, TEMT
    fw.set_al(0);
    fw.set_cf(false);
    true
}
