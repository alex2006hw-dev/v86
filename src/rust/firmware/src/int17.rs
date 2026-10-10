//! INT 17h — parallel port services.
//!
//! Implements the standard BIOS parallel port functions.

use crate::bda;
use crate::machine::Machine;
use crate::status;
use crate::Firmware;

/// LPT port base addresses from BDA.
pub const LPT_PORTS: [u32; 3] = [bda::LPT1, bda::LPT2, bda::LPT3];

/// Handle an INT 17h call.
pub fn handle_int17<M: Machine>(fw: &mut Firmware<M>) -> bool {
    let ah = fw.ah();
    match ah {
        0x00 => print_char(fw),
        0x01 => init_printer(fw),
        0x02 => get_status(fw),
        _ => {
            fw.set_cf(true);
            fw.set_ah(status::INVALID_FUNCTION);
            true
        }
    }
}

/// AH=00h: print character (AL=char, DX=port).
fn print_char<M: Machine>(fw: &mut Firmware<M>) -> bool {
    // Just acknowledge.
    fw.set_ah(0); // success
    fw.set_cf(false);
    true
}

/// AH=00h: initialize printer (DX=port).
fn init_printer<M: Machine>(fw: &mut Firmware<M>) -> bool {
    fw.set_ah(0); // success
    fw.set_cf(false);
    true
}

/// AH=02h: get printer status (DX=port).
fn get_status<M: Machine>(fw: &mut Firmware<M>) -> bool {
    // Return "ready" status.
    fw.set_ah(0x90); // not busy, not ack, no error, selected
    fw.set_cf(false);
    true
}
