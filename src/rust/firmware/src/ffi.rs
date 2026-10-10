//! WASI/host interface for the firmware.
//!
//! Lets a host (currently: Node.js driving the PCjs x86 CPU) embed the
//! firmware and supply the `Machine` implementation. Guest state lives
//! entirely on the host side: the firmware reaches it through the imported
//! `env::v86_*` callbacks below.
//!
//! Only compiled for wasm targets, so the crate's host-side unit tests
//! (which use their own `Machine`) are unaffected.

#![cfg(all(feature = "standalone", target_arch = "wasm32"))]

use crate::backend::{BlockInfo, DriveKind, RamDisk, SECTOR_SIZE, FLOPPY_1440K};
use crate::dispatch::{Config, E820Entry};
use crate::machine::{Flag, KeyEvent, Machine, Reg, RtcReading, SegReg};
use crate::Firmware;

// ----------------------------------------------------------------------
// Host callbacks, imported from the "env" module
// ----------------------------------------------------------------------

#[link(wasm_import_module = "env")]
extern "C" {
    fn v86_read8(addr: u32) -> u8;
    fn v86_write8(addr: u32, val: u8);
    fn v86_read_reg(i: u32) -> u32;
    fn v86_write_reg(i: u32, v: u32);
    fn v86_read_seg(i: u32) -> u32;
    fn v86_write_seg(i: u32, v: u32);
    fn v86_read_flag(i: u32) -> u32;
    fn v86_write_flag(i: u32, v: u32);
    fn v86_read_ip() -> u32;
    fn v86_write_ip(v: u32);
    fn v86_rtc(i: u32) -> u32;
    fn v86_poll_key() -> u32; // (pressed << 8) | scancode; 0xFFFF = none
    fn v86_reset();
    /// Pop a word from the guest stack, exactly as the v86 adapter does.
    fn v86_pop_stack_u16() -> u32;
    fn v86_peek_service_id() -> u32;
    fn v86_patch_saved_flags();
}

struct HostMachine;

impl Machine for HostMachine {
    fn read_u8(&mut self, addr: u32) -> u8 {
        unsafe { v86_read8(addr) }
    }
    fn write_u8(&mut self, addr: u32, val: u8) {
        unsafe { v86_write8(addr, val) }
    }

    fn read_reg(&self, r: Reg) -> u32 {
        unsafe { v86_read_reg(r as u32) }
    }
    fn write_reg(&mut self, r: Reg, v: u32) {
        unsafe { v86_write_reg(r as u32, v) }
    }

    fn read_seg(&self, r: SegReg) -> u16 {
        unsafe { v86_read_seg(r as u32) as u16 }
    }
    fn write_seg(&mut self, r: SegReg, v: u16) {
        unsafe { v86_write_seg(r as u32, v as u32) }
    }

    fn read_flag(&self, f: Flag) -> bool {
        unsafe { v86_read_flag(f as u32) != 0 }
    }
    fn write_flag(&mut self, f: Flag, v: bool) {
        unsafe { v86_write_flag(f as u32, v as u32) }
    }

    fn read_ip(&self) -> u32 {
        unsafe { v86_read_ip() }
    }
    fn write_ip(&mut self, v: u32) {
        unsafe { v86_write_ip(v) }
    }

    fn poll_key(&mut self) -> Option<KeyEvent> {
        let packed = unsafe { v86_poll_key() };
        if packed == 0xFFFF {
            None
        } else {
            Some(KeyEvent {
                scancode: (packed & 0xFF) as u8,
                pressed: (packed >> 8) & 1 != 0,
            })
        }
    }

    fn yield_cpu(&mut self) {
        // The host owns the clock; a blocking BIOS wait simply spins.
    }

    fn rtc_time(&self) -> RtcReading {
        unsafe {
            RtcReading {
                second: v86_rtc(0),
                minute: v86_rtc(1),
                hour: v86_rtc(2),
                day: v86_rtc(3),
                month: v86_rtc(4),
                year: v86_rtc(5),
                day_of_week: v86_rtc(6),
            }
        }
    }

    fn request_reset(&mut self) {
        unsafe { v86_reset() }
    }

    fn pop_stack_u16(&mut self) -> u16 {
        unsafe { v86_pop_stack_u16() as u16 }
    }

    fn peek_service_id(&self) -> u32 {
        unsafe { v86_peek_service_id() }
    }

    fn peek_stack_pointer(&self) -> u32 {
        unsafe { v86_pop_stack_u16() }
    }

    fn patch_saved_flags(&mut self) {
        unsafe { v86_patch_saved_flags() }
    }
}

// ----------------------------------------------------------------------
// Opaque handle
// ----------------------------------------------------------------------

/// Opaque firmware handle handed to the host.
pub struct FirmwareBox {
    inner: Firmware<HostMachine>,
    /// Scratch buffer the host writes disk images into.
    scratch: Vec<u8>,
}

impl FirmwareBox {
    fn inner_mut(&mut self) -> &mut Firmware<HostMachine> {
        &mut self.inner
    }
}

// ----------------------------------------------------------------------
// Exported functions
// ----------------------------------------------------------------------

/// Create a firmware instance. Returns null on allocation failure.
#[no_mangle]
pub extern "C" fn fw_create() -> *mut FirmwareBox {
    Box::into_raw(Box::new(FirmwareBox {
        inner: Firmware::new(HostMachine, Config::default()),
        scratch: vec![0u8; 4 * 1024 * 1024],
    }))
}

/// Destroy a firmware instance.
#[no_mangle]
pub extern "C" fn fw_destroy(f: *mut FirmwareBox) {
    if !f.is_null() {
        drop(unsafe { Box::from_raw(f) });
    }
}

/// Set the configured memory size in KiB (drives E820 / INT 12h).
#[no_mangle]
pub extern "C" fn fw_set_memory_kib(f: *mut FirmwareBox, kib: u32) {
    if f.is_null() {
        return;
    }
    unsafe { &mut *f }.inner_mut().config.memory_kib = kib as u16;
}

/// Write the system BIOS and video option ROM into guest memory and
/// point the IVT at their stubs.
#[no_mangle]
pub extern "C" fn fw_install_roms(f: *mut FirmwareBox) {
    if f.is_null() {
        return;
    }
    crate::post::install_roms(unsafe { &mut *f }.inner_mut());
}

/// Physical address of the ROM reset vector, so the harness can enter the
/// firmware the way the CPU does after reset.
#[no_mangle]
pub extern "C" fn fw_reset_vector() -> u32 {
    crate::rom::SYSTEM_ROM_BASE + 0xFFF0
}

/// Run POST.
#[no_mangle]
pub extern "C" fn fw_post(f: *mut FirmwareBox) {
    if f.is_null() {
        return;
    }
    crate::post::run_post(unsafe { &mut *f }.inner_mut());
}

/// Handle a BIOS interrupt. Returns 1 if handled, 0 if declined.
#[no_mangle]
pub extern "C" fn fw_interrupt(f: *mut FirmwareBox, vector: u32) -> u32 {
    if f.is_null() {
        return 0;
    }
    let fw = unsafe { &mut *f };
    crate::dispatch::dispatch_service(fw.inner_mut(), vector as u16) as u32
}

/// Add a RAM-backed floppy drive; returns its INT 13h number.
/// Sector count is capped at 65536 (a 32 MiB image).
#[no_mangle]
pub extern "C" fn fw_add_floppy(f: *mut FirmwareBox, sectors: u32) -> u32 {
    if f.is_null() {
        return 0xFF;
    }
    let sectors = (sectors as u64).min(65536);
    let fw = unsafe { &mut *f };
    let number = fw.inner_mut().drives.next_floppy();
    let geometry = match sectors {
        0..=2400 => crate::backend::FLOPPY_1200K,
        2410..=2880 => FLOPPY_1440K,
        _ => crate::backend::FLOPPY_2880K,
    };
    let info = BlockInfo {
        kind: DriveKind::Floppy,
        geometry,
        sector_size: SECTOR_SIZE,
        total_sectors: sectors,
        removable: true,
    };
    let data = vec![0u8; (sectors * SECTOR_SIZE as u64) as usize];
    fw.inner_mut()
        .drives
        .add(number, Box::new(RamDisk::new(info, data)));
    number as u32
}

/// Address of the host-writable scratch buffer.
#[no_mangle]
pub extern "C" fn fw_scratch_ptr(f: *mut FirmwareBox) -> *mut u8 {
    if f.is_null() {
        return core::ptr::null_mut();
    }
    unsafe { (*f).scratch.as_mut_ptr() }
}

/// Capacity of the scratch buffer in bytes.
#[no_mangle]
pub extern "C" fn fw_scratch_len(f: *mut FirmwareBox) -> u32 {
    if f.is_null() {
        return 0;
    }
    unsafe { (*f).scratch.len() as u32 }
}

/// Copy `len` bytes from the scratch buffer to sector 0 of `drive`.
#[no_mangle]
pub extern "C" fn fw_load_scratch(f: *mut FirmwareBox, drive: u32, len: u32) -> u32 {
    if f.is_null() || len == 0 {
        return 0;
    }
    let fw = unsafe { &mut *f };
    let len = (len as usize).min(fw.scratch.len());
    let ss = fw.inner_mut().drives.get(drive as u8).map(|d| d.info().sector_size).unwrap_or(512) as usize;
    let padded = (len + ss - 1) / ss * ss;
    let mut buf = vec![0u8; padded];
    buf[..len].copy_from_slice(&fw.scratch[..len]);
    match fw.inner_mut().drives.get(drive as u8) {
        Some(d) => match d.backend.write_sectors(0, &buf) {
            Ok(()) => 1,
            Err(_) => 0,
        },
        None => 0,
    }
}

/// Install a fixed E820 map (kind 1 = usable, 2 = reserved).
#[no_mangle]
pub extern "C" fn fw_set_e820(f: *mut FirmwareBox, entries: *const E820Entry, count: u32) {
    if f.is_null() || entries.is_null() {
        return;
    }
    let fw = unsafe { &mut *f };
    let inner = fw.inner_mut();
    inner.e820.clear();
    for i in 0..count as usize {
        let e = unsafe { *entries.add(i) };
        inner.e820.push(e);
    }
}