//! Adapter between the v86 emulator and the firmware crate.
//!
//! Two directions, and it is worth keeping them apart:
//!
//! * **In**: [`firmware_trap`] is called by the CPU when a ROM stub
//!   executes the trap interrupt. It pops the service id off the guest
//!   stack, runs the service, and lets the stub's `iret` continue.
//! * **Out**: the firmware reaches guest state through the [`Machine`]
//!   implementation below, which is a thin translation layer over v86's
//!   register, segment and memory accessors.
//!
//! The firmware's ROM images are built in Rust and written straight into
//! guest memory by [`init_firmware`], so there is no ROM blob to ship
//! and no licence to track for it.

use crate::cpu::cpu;
use crate::cpu::global_pointers::*;
use crate::cpu::memory;
use crate::cpu::misc_instr::{adjust_stack_reg, getcf, getof, getpf, getsf, getzf, push16};
use crate::cpu::cpu::FLAG_CARRY;
use crate::regs::CS;
use crate::firmware::backend::BlockBackend;
use crate::firmware::dispatch::{Config, Firmware};
use crate::firmware::machine::{Flag, KeyEvent, Machine, Reg, RtcReading, SegReg};
use crate::firmware::rom;

pub use crate::firmware::backend::BlockBackend as DriveBackend;

/// The interrupt vector the ROM stubs use to reach these services.
pub use crate::firmware::rom::TRAP_VECTOR;

/// True when `physical` is inside a ROM the firmware installed.
/// True when the built-in firmware is installed and servicing traps.
///
/// The ROM check alone is not sufficient. SeaBIOS and the Bochs BIOS live in
/// exactly the address windows this firmware claims — `F000:0000` and
/// `C000:0000` — so with `is_firmware_rom` as the only condition, an
/// `int 0x66` executed by a *third-party* ROM would be treated as a firmware
/// trap, serviced by nothing, and the interrupt would never reach the IVT.
/// SeaBIOS really does execute one, and swallowing it leaves CS:IP stale,
/// which the JIT reports as `table index is out of bounds` and the
/// interpreter limps past.
pub fn firmware_is_installed() -> bool {
    unsafe { !firmware_ptr().is_null() }
}

pub fn is_firmware_rom(physical: u32) -> bool {
    rom::is_firmware_rom(physical)
}

/// Number of traps the CPU has routed to the firmware, and the last
/// service id it dispatched. Diagnostic; cheap enough to always keep.
static mut TRAP_COUNT: u32 = 0;
static mut LAST_SERVICE: u32 = 0xFFFF;

/// Ring of the last few traps: `(return address, service id, stack top)`.
/// Small and fixed so it needs no allocation on the CPU's path.
static mut TRAP_RING: [(u32, u32, u32); 16] = [(0, 0, 0); 16];
static mut TRAP_RING_LEN: usize = 0;

#[no_mangle]
pub extern "C" fn v86_firmware_trap_ring_len() -> u32 {
    unsafe { TRAP_RING_LEN as u32 }
}

#[no_mangle]
pub extern "C" fn v86_firmware_trap_ring(i: u32) -> u32 {
    unsafe {
        if (i as usize) < TRAP_RING_LEN {
            TRAP_RING[i as usize].0
        } else {
            0
        }
    }
}

#[no_mangle]
pub extern "C" fn v86_firmware_trap_ring_service(i: u32) -> u32 {
    unsafe {
        if (i as usize) < TRAP_RING_LEN {
            TRAP_RING[i as usize].1
        } else {
            0
        }
    }
}

#[no_mangle]
pub extern "C" fn v86_firmware_trap_ring_stack(i: u32) -> u32 {
    unsafe {
        if (i as usize) < TRAP_RING_LEN {
            TRAP_RING[i as usize].2
        } else {
            0
        }
    }
}

/// How many firmware traps have fired. `0xFFFFFFFF` means one fired with
/// no firmware installed.
#[no_mangle]
pub extern "C" fn v86_firmware_trap_count() -> u32 {
    unsafe { TRAP_COUNT }
}

/// The service id of the most recent trap, or 0xFFFF if none.
#[no_mangle]
pub extern "C" fn v86_firmware_last_service() -> u32 {
    unsafe { LAST_SERVICE }
}

/// Whether the firmware instance exists.
#[no_mangle]
pub extern "C" fn v86_firmware_present() -> u32 {
    unsafe {
        if firmware_ptr().is_null() {
            0
        } else {
            1
        }
    }
}

/// Whether the host asked for tracing. Remembered separately from the
/// firmware instance because the host configures tracing *before*
/// [`init_firmware`], which builds a fresh one.
static mut TRACE_ENABLED: bool = false;

/// The firmware instance. `None` until the host calls [`init_firmware`].
///
/// Single-threaded by construction: the emulator's CPU loop is the only
/// caller and it runs on one thread, so a plain static matches how the
/// rest of this crate keeps CPU state.
static mut FIRMWARE: Option<Firmware<EmulatorMachine>> = None;

/// Take a raw pointer to the firmware instance.
///
/// `unsafe` because the caller must not hold it across anything else
/// that touches `FIRMWARE`.
unsafe fn firmware_ptr() -> *mut Firmware<EmulatorMachine> {
    let slot = std::ptr::addr_of_mut!(FIRMWARE);
    match (*slot).as_mut() {
        Some(fw) => fw as *mut Firmware<EmulatorMachine>,
        None => std::ptr::null_mut(),
    }
}

/// Build the ROM images, install them, and create the firmware instance.
///
/// Called once from the host before the CPU leaves reset.
pub fn init_firmware(config: Config) {
    let mut fw = Firmware::new(EmulatorMachine::new(), config);
    fw.trace.set_enabled(unsafe { TRACE_ENABLED });
    crate::firmware::install_roms(&mut fw);
    unsafe {
        FIRMWARE = Some(fw);
    }
}

/// Register a block device with the firmware's INT 13h drive table.
pub fn register_drive(number: u8, backend: Box<dyn BlockBackend>) {
    unsafe {
        let fw = firmware_ptr();
        if !fw.is_null() {
            (*fw).drives.add(number, backend);
        }
    }
}

/// Queue a scancode for the next IRQ 1 service.
pub fn push_key(scancode: u8, pressed: bool) {
    unsafe {
        let fw = firmware_ptr();
        if !fw.is_null() {
            (*fw).machine.keys.push(KeyEvent { scancode, pressed });
        }
    }
}

// ----------------------------------------------------------------------
// Host ABI
//
// Everything below is called from JavaScript. It is kept thin and
// panic-free: a trap here means the machine wedges with no diagnostic.
// ----------------------------------------------------------------------

/// Build the ROMs, install them and arm the trap. Returns 1 on success.
///
/// The CPU must be in reset when this is called: `reset_cpu` leaves
/// CS:IP at F000:FFF0, which is the ROM's reset vector, so the firmware
/// starts running as soon as the emulator resumes.
///
/// `boot_order` is a bitmask: 1 = floppy, 2 = hard disk, 4 = CD.
#[no_mangle]
pub extern "C" fn v86_firmware_init(
    memory_kib: u32,
    total_memory_kib: u32,
    floppy_count: u32,
    serial_count: u32,
    printer_count: u32,
    math_coprocessor: u32,
    boot_order: u32,
) -> i32 {
    let mut order: Vec<&'static str> = Vec::new();
    if boot_order & 1 != 0 {
        order.push("floppy");
    }
    if boot_order & 2 != 0 {
        order.push("hd");
    }
    if boot_order & 4 != 0 {
        order.push("cd");
    }
    if order.is_empty() {
        order.push("floppy");
        order.push("hd");
        order.push("cd");
    }

    init_firmware(Config {
        memory_kib: memory_kib.min(u16::MAX as u32) as u16,
        // The total is what the emulator's RAM actually is. It is passed
        // separately because the BDA word is 16 bits and means
        // conventional memory, which is not the same thing.
        total_memory_kib: total_memory_kib.max(u32::from(memory_kib.min(u16::MAX as u32))),
        boot_order: order,
        floppy_count: floppy_count.min(2) as u8,
        serial_count: serial_count.min(4) as u8,
        printer_count: printer_count.min(3) as u8,
        math_coprocessor: math_coprocessor != 0,
        ..Config::default()
    });
    1
}

/// Attach a RAM-backed floppy drive of `sectors` 512-byte sectors.
///
/// Returns the INT 13h drive number, or -1 if it could not be created.
/// The image bytes live in the firmware's own memory; the host writes
/// them through [`v86_firmware_drive_ptr`].
#[no_mangle]
pub extern "C" fn v86_firmware_add_floppy(sectors: u32) -> i32 {
    use crate::firmware::backend::{BlockInfo, DriveKind, RamDisk, FLOPPY_1200K, FLOPPY_1440K, FLOPPY_2880K};
    use crate::firmware::backend::{SECTOR_SIZE, Geometry};

    let sectors = (sectors as u64).min(65536);
    let geometry: Geometry = match sectors {
        0..=2400 => FLOPPY_1200K,
        2410..=2880 => FLOPPY_1440K,
        _ => FLOPPY_2880K,
    };
    let data = vec![0u8; (sectors * SECTOR_SIZE as u64) as usize];

    unsafe {
        let fw = firmware_ptr();
        if fw.is_null() {
            return -1;
        }
        let number = (*fw).drives.next_floppy();
        (*fw).drives.add(
            number,
            Box::new(RamDisk::new(
                BlockInfo {
                    kind: DriveKind::Floppy,
                    geometry,
                    sector_size: SECTOR_SIZE,
                    total_sectors: sectors,
                    removable: true,
                },
                data,
            )),
        );
        number as i32
    }
}

/// Trampoline for [`HostImage`], which stores the machine as an opaque
/// pointer rather than knowing its type.
///
/// # Safety
///
/// `ctx` must be a pointer to the `EmulatorMachine` owned by the firmware
/// instance, and must stay valid for as long as the drive exists.
unsafe fn host_image_read(ctx: *mut (), image: u8, offset: u64, buf: &mut [u8]) -> bool {
    let machine = &mut *(ctx as *mut EmulatorMachine);
    machine.read_host_image(image, offset, buf)
}

/// Register a host-backed block device and return its INT 13h drive number.
///
/// `image` is the host's handle for the image, `kind` is 0 for a hard disk
/// and 1 for a CD-ROM, and `total_sectors` is the image length in units of
/// `sector_size` (512 for a hard disk, 2048 for a CD).
///
/// Unlike [`v86_firmware_add_floppy`] the bytes are **not** copied into the
/// firmware: a CD image is hundreds of megabytes, and v86 hands images over
/// as JavaScript buffers that would have to be copied into the wasm heap to
/// be addressable. Sectors are pulled from the host on demand through
/// [`Machine::read_host_image`] instead.
///
/// Returns the drive number, or -1 on failure.
#[no_mangle]
pub extern "C" fn v86_firmware_add_drive(image: u32, kind: u32, total_sectors: u32, sector_size: u32) -> i32 {
    use crate::firmware::backend::{
        BlockInfo, DriveKind, Geometry, HostImage, SECTOR_SIZE,
    };

    let sector_size = if sector_size == 2048 { 2048 } else { SECTOR_SIZE };
    let total = total_sectors as u64;
    let kind = if kind == 1 { DriveKind::CdRom } else { DriveKind::HardDisk };

    unsafe {
        let fw = firmware_ptr();
        if fw.is_null() || total == 0 {
            return -1;
        }

        // A hard disk is addressed by CHS as well as LBA, so it needs a
        // plausible geometry. EDD callers use LBA and never see this; the
        // classic INT 13h calls that do would otherwise have nothing to
        // report. 16 heads and 63 sectors per track is the conventional
        // choice and covers any image up to about 2 TiB.
        let heads = 16u32;
        let sectors_per_track = 63u32;
        let cylinders = (total / (heads as u64 * sectors_per_track as u64)).clamp(1, 0x3FF) as u32;
        let geometry = Geometry {
            cylinders,
            heads,
            sectors_per_track,
        };

        let number = match kind {
            DriveKind::CdRom => (*fw).drives.next_cd(),
            _ => (*fw).drives.next_hd(),
        };

        let machine: *mut EmulatorMachine = &mut (*fw).machine;
        (*fw).drives.add(
            number,
            Box::new(HostImage::new(
                BlockInfo {
                    kind,
                    geometry,
                    sector_size,
                    total_sectors: total,
                    removable: kind == DriveKind::CdRom,
                },
                image as u8,
                machine as *mut (),
                host_image_read,
            )),
        );
        number as i32
    }
}

/// Register an option ROM image and return 0, or -1 if it could not be
/// read.
///
/// `image` is a handle into the host's `firmware_images` and `len` is the
/// length in bytes, exactly as [`v86_firmware_add_drive`] numbers its
/// images. The ROM is copied in rather than held by reference, because
/// POST has to place it in guest memory at an address the guest can
/// read, and the guest may read it back.
///
/// There is no limit on size beyond what a wasm allocation can do, but
/// the option-ROM run is `0xD0000`-`0xE0000`, so anything past 64 KiB
/// cannot be placed and is rejected here rather than half-run.
#[no_mangle]
pub extern "C" fn v86_firmware_add_option_rom(image: u32, len: u32) -> i32 {
    const MAX_ROM: usize = 64 * 1024;

    unsafe {
        let fw = firmware_ptr();
        if fw.is_null() || len == 0 || len as usize > MAX_ROM {
            return -1;
        }

        let mut data = vec![0u8; len as usize];
        let machine: *mut EmulatorMachine = &mut (*fw).machine;

        if !(*machine).read_host_image(image as u8, 0, &mut data) {
            return -1;
        }

        (*fw).option_roms.push(data);
        0
    }
}

/// Address of a drive's image buffer, or null. The host may read and
/// write it directly, which keeps image loading a single memory copy.
#[no_mangle]
pub extern "C" fn v86_firmware_drive_ptr(drive: i32) -> *mut u8 {
    unsafe {
        let fw = firmware_ptr();
        if fw.is_null() || drive < 0 {
            return std::ptr::null_mut();
        }
        match (*fw).drives.get(drive as u8) {
            Some(d) => match d.backend.downcast_ram_disk() {
                Some(rd) => rd.data.as_mut_ptr(),
                None => std::ptr::null_mut(),
            },
            None => std::ptr::null_mut(),
        }
    }
}

/// Length of a drive's image buffer in bytes, or 0.
#[no_mangle]
pub extern "C" fn v86_firmware_drive_len(drive: i32) -> u32 {
    unsafe {
        let fw = firmware_ptr();
        if fw.is_null() || drive < 0 {
            return 0;
        }
        match (*fw).drives.get(drive as u8) {
            Some(d) => match d.backend.downcast_ram_disk() {
                Some(rd) => rd.data.len() as u32,
                None => 0,
            },
            None => 0,
        }
    }
}

/// Turn firmware tracing on or off. Off by default: a guest that calls
/// INT 10h in a loop would otherwise fill the ring.
#[no_mangle]
pub extern "C" fn v86_firmware_set_trace(enabled: u32) {
    unsafe {
        TRACE_ENABLED = enabled != 0;
        let fw = firmware_ptr();
        if !fw.is_null() {
            (*fw).trace.set_enabled(TRACE_ENABLED);
        }
    }
}

/// Address of the trace log, for the host to read. The bytes are
/// `[tag][8 hex sequence digits][' ']line['\n']` repeated.
#[no_mangle]
pub extern "C" fn v86_firmware_trace_ptr() -> *const u8 {
    unsafe {
        let fw = firmware_ptr();
        if fw.is_null() {
            return std::ptr::null();
        }
        (*fw).trace.bytes().as_ptr()
    }
}

/// Length of the trace log in bytes.
#[no_mangle]
pub extern "C" fn v86_firmware_trace_len() -> u32 {
    unsafe {
        let fw = firmware_ptr();
        if fw.is_null() {
            return 0;
        }
        (*fw).trace.bytes().len() as u32
    }
}

/// Number of entries the ring dropped by wrapping.
#[no_mangle]
pub extern "C" fn v86_firmware_trace_dropped() -> u32 {
    unsafe {
        let fw = firmware_ptr();
        if fw.is_null() {
            return 0;
        }
        (*fw).trace.dropped
    }
}

/// Read the emulated real-time clock, so INT 1Ah agrees with the host's.
///
/// Packed as `second | minute<<8 | hour<<16 | day<<24`.
#[no_mangle]
pub extern "C" fn v86_firmware_set_rtc(packed: u32) {
    use crate::firmware::machine::RtcReading;
    unsafe {
        let fw = firmware_ptr();
        if fw.is_null() {
            return;
        }
        (*fw).machine.rtc = RtcReading {
            second: packed & 0xFF,
            minute: (packed >> 8) & 0xFF,
            hour: (packed >> 16) & 0xFF,
            day: (packed >> 24) & 0xFF,
            ..RtcReading::default()
        };
    }
}

/// Drop the firmware instance, e.g. when the machine is destroyed.
pub fn destroy_firmware() {
    unsafe {
        FIRMWARE = None;
    }
}

/// Run a firmware service.
///
/// Called from the CPU for `int <TRAP_VECTOR>` instructions whose
/// return address is inside a firmware ROM. The service id was pushed by
/// the stub; consuming it here is what lets the stub `iret` cleanly.
pub fn firmware_trap() {
    unsafe {
        TRAP_COUNT = TRAP_COUNT.wrapping_add(1);
        let fw = firmware_ptr();
        if !fw.is_null() {
            let slot = TRAP_RING_LEN % 16;
            TRAP_RING[slot] = (
                cpu::get_instruction_pointer() as u32,
                (*fw).machine.peek_service_id(),
                (*fw).machine.peek_stack_pointer(),
            );
            TRAP_RING_LEN += 1;
        }
        if fw.is_null() {
            // No firmware installed: there is no ROM left to service, and
            // the IVT entry points at code that is not there. Returning is
            // the only honest outcome; jumping into low RAM is not.
            dbg_log!("firmware trap with no firmware installed");
            return;
        }
        // Peek the service id before dispatch so a mismatch is visible
        // from the host rather than silently doing nothing.
        LAST_SERVICE = (*fw).machine.peek_service_id();
        crate::firmware::firmware_service(&mut *fw);
    }
}

/// The emulator-backed machine implementation.
pub struct EmulatorMachine {
    keys: Vec<KeyEvent>,
    /// The host's wall clock, so INT 1Ah agrees with the emulated RTC.
    pub rtc: RtcReading,
}

impl EmulatorMachine {
    pub fn new() -> EmulatorMachine {
        EmulatorMachine {
            keys: Vec::new(),
            rtc: RtcReading::default(),
        }
    }
}

impl Default for EmulatorMachine {
    fn default() -> Self {
        Self::new()
    }
}

/// Real-mode physical address from a segment and offset.
unsafe fn phys_of(seg: u16, off: u16) -> u32 {
    (u32::from(seg) << 4) + u32::from(off)
}

/// Offset from SS:SP to the FLAGS image of the interrupt frame a guest's
/// `int n` pushed, once the ROM stub's service id has been popped.
///
/// The CPU pushes IP, then CS, then FLAGS, so FLAGS sits highest. SeaBIOS
/// relies on the same layout: its `struct bregs` overlays the frame and
/// `regs->flags` is the word at this offset, which is how it returns carry
/// to its caller.
const SAVED_FLAGS_OFFSET: u32 = 4;

/// Physical address of the top of the current stack (SS:SP).
///
/// v86 keeps the stack size in `stack_size_32` and mirrors SP/ESP through
/// the same register slot, so this goes through the CPU's own accessor
/// rather than indexing `reg16` directly — reading `reg16[SP]` here would
/// see 0 whenever the stack is in 32-bit mode.
unsafe fn stack_phys() -> u32 {
    crate::cpu::misc_instr::get_stack_pointer(0) as u32
}

impl Machine for EmulatorMachine {
    fn read_u8(&mut self, addr: u32) -> u8 {
        memory::read8(addr) as u8
    }

    fn write_u8(&mut self, addr: u32, val: u8) {
        unsafe {
            memory::write8(addr, val as i32);
        }
    }

    fn read_reg(&self, r: Reg) -> u32 {
        unsafe { cpu::read_reg32(r as i32) as u32 }
    }

    fn write_reg(&mut self, r: Reg, v: u32) {
        unsafe {
            cpu::write_reg32(r as i32, v as i32);
        }
    }

    fn read_seg(&self, r: SegReg) -> u16 {
        unsafe { *sreg.offset(r as i32 as isize) }
    }

    fn write_seg(&mut self, r: SegReg, v: u16) {
        unsafe {
            if r == SegReg::Cs {
                // Use v86's own real-mode CS switch so the segment cache,
                // the code-size flag and the instruction pointer stay in
                // step. `instruction_pointer` is a *linear* address, so a
                // bare base write would leave the CPU executing wherever
                // the old segment pointed.
                cpu::switch_cs_real_mode(i32::from(v));
                return;
            }

            set_seg_base(r as i32 as isize, v);
            *sreg.offset(r as i32 as isize) = v;
        }
    }

    fn read_flag(&self, f: Flag) -> bool {
        unsafe {
            match f {
                Flag::Cf => getcf(),
                Flag::Pf => getpf(),
                Flag::Af => false,
                Flag::Zf => getzf(),
                Flag::Sf => getsf(),
                Flag::Tf => cpu::get_eflags() & (1 << 8) != 0,
                Flag::If => cpu::get_eflags() & (1 << 9) != 0,
                Flag::Df => cpu::get_eflags() & (1 << 10) != 0,
                Flag::Of => getof(),
            }
        }
    }

    fn write_flag(&mut self, f: Flag, v: bool) {
        unsafe {
            // Read through get_eflags() so that lazily-computed
            // condition codes are materialised before they are poked.
            let mut eflags = cpu::get_eflags();
            let bit = 1i32 << (f as i32);
            if v {
                eflags |= bit;
            } else {
                eflags &= !bit;
            }
            cpu::set_eflags(eflags);
        }
    }

    fn read_ip(&self) -> u32 {
        unsafe { cpu::get_real_eip() as u32 }
    }

    fn write_ip(&mut self, v: u32) {
        unsafe {
            // Same sequence as `far_jump`: move CS first, then recompute
            // the linear instruction pointer, then refresh the cached
            // state flags the interpreter and JIT both read.
            *instruction_pointer = cpu::get_seg_cs() + v as i32;
            cpu::update_state_flags();
        }
    }

    fn poll_key(&mut self) -> Option<KeyEvent> {
        if self.keys.is_empty() {
            None
        } else {
            Some(self.keys.remove(0))
        }
    }

    fn yield_cpu(&mut self) {}

    fn rtc_time(&self) -> RtcReading {
        // The host's clock is authoritative: a firmware that invented its
        // own would disagree with the emulated CMOS and with every guest
        // that reads both.
        self.rtc
    }

    fn request_reset(&mut self) {
        unsafe {
            cpu::reset_cpu();
        }
    }

    fn peek_service_id(&self) -> u32 {
        unsafe { memory::read16(stack_phys()) as u32 }
    }

    fn peek_stack_pointer(&self) -> u32 {
        unsafe { crate::cpu::misc_instr::get_stack_pointer(0) as u32 }
    }

    fn read_host_image(&mut self, image: u8, byte_offset: u64, buf: &mut [u8]) -> bool {
        unsafe {
            crate::cpu::cpu::js::read_host_image(
                image as i32,
                byte_offset as f64,
                buf.as_mut_ptr() as i32,
                buf.len() as i32,
            ) != 0
        }
    }

    fn patch_saved_flags(&mut self) {
        unsafe {
            // The trap fires *before* the `int` pushes anything: the CPU has
            // decoded `int 0x66` and `instruction_pointer` already holds the
            // address the return would have gone to, so the only thing on the
            // stack is the service id the stub pushed. Popping it (which
            // `firmware_service` has done) exposes the frame the *guest's*
            // original `int n` pushed, laid out exactly as the CPU laid it
            // out in the real-mode branch of `call_interrupt_vector`:
            //
            //     SS:SP + 0   IP     (pushed 16-bit)
            //     SS:SP + 2   CS
            //     SS:SP + 4   FLAGS
            //
            // The stub's `iret` pops that frame, so this is the only place a
            // service's carry flag can reach the caller. Patching at SS:SP
            // instead overwrites the return address -- and since it is the
            // low byte, setting carry there redirects the caller into the
            // middle of the instruction it was about to execute.
            let addr = stack_phys() + SAVED_FLAGS_OFFSET;
            let saved = memory::read16(addr) as u16;
            let cf: u16 = if cpu::get_cc_flags() & FLAG_CARRY != 0 { 1 } else { 0 };
            let patched = (saved & !1) | cf;
            if patched != saved
            {
                memory::write16(addr, i32::from(patched));
            }
        }
    }

    fn pop_stack_u16(&mut self) -> u16 {
        unsafe {
            let word = memory::read16(stack_phys());
            adjust_stack_reg(2);
            word as u16
        }
    }

    fn push_u16(&mut self, value: u16) {
        unsafe {
            // SP is decremented first so the write lands at the new top,
            // which is the same frame layout `pop_stack_u16` reverses.
            adjust_stack_reg(-2);
            memory::write16(stack_phys(), i32::from(value));
        }
    }

    fn chain_to(&mut self, segment: u16, offset: u16) {
        unsafe {
            push16(u32::from(segment) as i32).unwrap();
            *instruction_pointer = phys_of(segment, offset) as i32;
            *segment_offsets.offset(CS as isize) = phys_of(segment, 0) as i32;
            // Chain from an interrupt handler ends in an iret the caller
            // already pushed, so only the frame the caller expects is
            // missing: nothing. The far return address is on the stack.
            set_cs_real_mode_shim(segment);
        }
    }
}

// The helpers below exist only to keep the unsafe register pokes in one
// place; v86 exposes segment switching through `switch_cs_real_mode`,
// which is crate-private.
unsafe fn set_seg_base(index: isize, value: u16) {
    *segment_offsets.offset(index) = (u32::from(value) << 4) as i32;
    *segment_limits.offset(index) = 0xFFFF;
    *segment_access_bytes.offset(index) = 0xF2 | (if index == CS as isize { 0x08 } else { 0 });
    *segment_is_null.offset(index) = false;
}

unsafe fn set_cs_real_mode_shim(segment: u16) {
    set_seg_base(CS as isize, segment);
    *sreg.offset(CS as isize) = segment;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trap_vector_is_out_of_everyones_way() {
        // The trap is only honoured inside the firmware's ROM, but a
        // vector that some guest uses heavily would still be confusing
        // in a disassembly, so keep it out of the DOS/DPMI ranges.
        assert!(TRAP_VECTOR >= 0x60, "trap vector must avoid the DOS range");
        assert!(TRAP_VECTOR < 0x70, "trap vector must avoid DPMI");
    }
}
