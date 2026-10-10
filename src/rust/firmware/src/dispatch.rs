//! Firmware core: the `Firmware` struct, configuration, and the
//! top-level BIOS interrupt dispatcher.
//!
//! The firmware intercepts real-mode software interrupts whose IVT
//! entry points into the firmware marker area (0xF0000–0xFFFFF).
//! If guest software re-vectors an interrupt elsewhere, the firmware
//! declines and the guest handler runs (matching real hardware).

use crate::backend::DriveKind;
use crate::bda;
use crate::eltorito::BootInfo;
use crate::int13::DriveTable;
use crate::machine::{Flag, Machine, Reg, SegReg};
use crate::status;

/// Physical address where the VBE mode table and strings are
/// placed (option ROM area).
pub const VBE_ROM_BASE: u32 = 0xC8000;

/// Firmware configuration, set by the host at init time.
#[derive(Clone, Debug)]
pub struct Config {
    /// Base memory size in KiB (written to BDA 0x413).
    pub memory_kib: u16,
    /// Boot order: "floppy", "hd", "cd" in priority order.
    pub boot_order: Vec<&'static str>,
    /// Number of floppy drives.
    pub floppy_count: u8,
    /// Number of serial ports.
    pub serial_count: u8,
    /// Number of parallel ports.
    pub printer_count: u8,
    /// Whether a math coprocessor is present.
    pub math_coprocessor: bool,
    /// Whether the game I/O port is present.
    pub game_io: bool,
    /// Whether DMA is present.
    pub dma: bool,
    /// Whether to run the POST memory test.
    pub memory_test: bool,
    /// VGA memory size in bytes (for VBE LFB).
    pub vga_memory_size: u32,
    /// LFB base address (must match the emulator's VGA mapping).
    pub lfb_address: u32,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            memory_kib: 640,
            boot_order: vec!["floppy", "hd", "cd"],
            floppy_count: 1,
            serial_count: 1,
            printer_count: 1,
            math_coprocessor: false,
            game_io: false,
            dma: true,
            memory_test: false,
            vga_memory_size: 256 * 1024,
            lfb_address: 0xE0000000,
        }
    }
}

/// E820 memory map entry (24-byte descriptor).
#[derive(Copy, Clone, Debug)]
pub struct E820Entry {
    pub base: u64,
    pub length: u64,
    pub kind: u32, // 1 = usable, 2 = reserved, 3 = ACPI reclaim, 4 = ACPI NVS
}

/// Video state tracked by the firmware.
#[derive(Copy, Clone, Debug, Default)]
pub struct VideoState {
    pub mode: u8,
    pub page: u8,
    pub cols: u8,
    pub rows: u8,
}

/// Keyboard state tracked by the firmware.
#[derive(Copy, Clone, Debug, Default)]
pub struct KeyboardState {
    pub shift_flags: u8,
    pub led_state: u8,
    pub typematic_rate: u8,
}

/// CMOS/RTC state.
#[derive(Clone, Debug)]
pub struct Cmos {
    /// NVRAM bytes 0x0E–0x7F (114 bytes).
    pub nvram: [u8; 114],
    /// Status register D (update-in-progress flag).
    pub update_in_progress: bool,
}

impl Default for Cmos {
    fn default() -> Cmos {
        Cmos {
            nvram: [0u8; 114],
            update_in_progress: false,
        }
    }
}

/// VBE state tracked by the firmware.
#[derive(Clone, Debug, Default)]
pub struct VbeState {
    /// Current VBE mode (0 = not in VBE mode).
    pub current_mode: u16,
    /// Window number (0 or 1) for 4F05h.
    pub window: u8,
    /// Window granularity in KiB.
    pub granularity: u16,
    /// Window size in KiB.
    pub window_size: u16,
    /// DAC palette width (6 or 8 bits).
    pub dac_width: u8,
    /// Saved video state for 4F04h.
    pub saved_state: Option<Box<[u8; 256]>>,
}

/// The firmware core. Generic over the machine abstraction.
pub struct Firmware<M: Machine> {
    pub machine: M,
    pub config: Config,
    pub drives: DriveTable,
    pub video: VideoState,
    pub keyboard: KeyboardState,
    pub cmos: Cmos,
    pub e820: Vec<E820Entry>,
    pub a20_enabled: bool,
    pub last_status: u8,
    pub vbe: VbeState,
    pub boot_info: Option<BootInfo>,
    pub cached_catalog: Option<[u8; 2048]>,
    /// Set when INT 19h should be invoked at the end of POST.
    pub pending_boot: bool,
}

impl<M: Machine> Firmware<M> {
    pub fn new(machine: M, config: Config) -> Firmware<M> {
        Firmware {
            machine,
            config,
            drives: DriveTable::new(),
            video: VideoState::default(),
            keyboard: KeyboardState::default(),
            cmos: Cmos::default(),
            e820: Vec::new(),
            a20_enabled: true,
            last_status: status::SUCCESS,
            vbe: VbeState::default(),
            boot_info: None,
            cached_catalog: None,
            pending_boot: false,
        }
    }

    // ------------------------------------------------------------------
    // Register convenience accessors
    // ------------------------------------------------------------------
    pub fn al(&self) -> u8 {
        self.machine.read_reg8(Reg::Eax, false)
    }
    pub fn ah(&self) -> u8 {
        self.machine.read_reg8(Reg::Eax, true)
    }
    pub fn set_ah(&mut self, v: u8) {
        self.machine.write_reg8(Reg::Eax, true, v);
    }
    pub fn set_al(&mut self, v: u8) {
        self.machine.write_reg8(Reg::Eax, false, v);
    }
    pub fn set_ax(&mut self, v: u16) {
        self.machine.write_reg(Reg::Eax, v as u32);
    }
    pub fn ax(&self) -> u16 {
        self.machine.read_reg(Reg::Eax) as u16
    }
    pub fn bh(&self) -> u8 {
        self.machine.read_reg8(Reg::Ebx, true)
    }
    pub fn set_bh(&mut self, v: u8) {
        self.machine.write_reg8(Reg::Ebx, true, v);
    }
    pub fn bl(&self) -> u8 {
        self.machine.read_reg8(Reg::Ebx, false)
    }
    pub fn set_bl(&mut self, v: u8) {
        self.machine.write_reg8(Reg::Ebx, false, v);
    }
    pub fn set_bx(&mut self, v: u16) {
        self.machine.write_reg(Reg::Ebx, v as u32);
    }
    pub fn bx(&self) -> u16 {
        self.machine.read_reg(Reg::Ebx) as u16
    }
    pub fn cl(&self) -> u8 {
        self.machine.read_reg8(Reg::Ecx, false)
    }
    pub fn set_cl(&mut self, v: u8) {
        self.machine.write_reg8(Reg::Ecx, false, v);
    }
    pub fn set_cx(&mut self, v: u16) {
        self.machine.write_reg(Reg::Ecx, v as u32);
    }
    pub fn cx(&self) -> u16 {
        self.machine.read_reg(Reg::Ecx) as u16
    }
    pub fn ch(&self) -> u8 {
        self.machine.read_reg8(Reg::Ecx, true)
    }
    pub fn set_ch(&mut self, v: u8) {
        self.machine.write_reg8(Reg::Ecx, true, v);
    }
    pub fn dl(&self) -> u8 {
        self.machine.read_reg8(Reg::Edx, false)
    }
    pub fn set_dl(&mut self, v: u8) {
        self.machine.write_reg8(Reg::Edx, false, v);
    }
    pub fn dh(&self) -> u8 {
        self.machine.read_reg8(Reg::Edx, true)
    }
    pub fn set_dh(&mut self, v: u8) {
        self.machine.write_reg8(Reg::Edx, true, v);
    }
    pub fn set_dx(&mut self, v: u16) {
        self.machine.write_reg(Reg::Edx, v as u32);
    }
    pub fn dx(&self) -> u16 {
        self.machine.read_reg(Reg::Edx) as u16
    }
    pub fn si(&self) -> u16 {
        self.machine.read_reg(Reg::Esi) as u16
    }
    pub fn set_si(&mut self, v: u16) {
        self.machine.write_reg(Reg::Esi, v as u32);
    }
    pub fn di(&self) -> u16 {
        self.machine.read_reg(Reg::Edi) as u16
    }
    pub fn set_di(&mut self, v: u16) {
        self.machine.write_reg(Reg::Edi, v as u32);
    }
    pub fn es(&self) -> u16 {
        self.machine.read_seg(SegReg::Es)
    }
    pub fn set_es(&mut self, v: u16) {
        self.machine.write_seg(SegReg::Es, v);
    }
    pub fn ds(&self) -> u16 {
        self.machine.read_seg(SegReg::Ds)
    }
    pub fn set_ds(&mut self, v: u16) {
        self.machine.write_seg(SegReg::Ds, v);
    }

    pub fn set_cf(&mut self, v: bool) {
        self.machine.write_flag(Flag::Cf, v);
    }

    /// Set the carry flag and AH to a status code (error return).
    pub fn set_error(&mut self, st: u8) {
        self.set_cf(true);
        self.set_ah(st);
    }

    /// Clear the carry flag and set AH to 0 (success return).
    pub fn set_ok(&mut self) {
        self.set_cf(false);
        self.set_ah(status::SUCCESS);
    }
}

// ----------------------------------------------------------------------
// Top-level BIOS interrupt dispatcher
// ----------------------------------------------------------------------

/// Entry point for firmware-owned interrupts. The emulator calls
/// this when a real-mode software interrupt's IVT entry points into
/// the firmware marker area. Returns true if the firmware handled
/// the interrupt.
pub fn bios_interrupt<M: Machine>(fw: &mut Firmware<M>, vector: u8) -> bool {
    match vector {
        0x10 => crate::int10::handle_int10(fw),
        0x11 => handle_int11(fw),
        0x12 => handle_int12(fw),
        0x13 => {
            crate::int13::handle_int13(fw);
            true
        }
        0x14 => crate::int14::handle_int14(fw),
        0x15 => crate::int15::handle_int15(fw),
        0x16 => crate::int16::handle_int16(fw),
        0x17 => crate::int17::handle_int17(fw),
        0x19 => crate::post::handle_int19(fw),
        0x1A => crate::int1a::handle_int1a(fw),
        _ => false,
    }
}

/// INT 11h — get equipment list.
fn handle_int11<M: Machine>(fw: &mut Firmware<M>) -> bool {
    let mut equip: u16 = 0;
    if fw.config.floppy_count > 0 {
        equip |= bda::equip::FLOPPY_INSTALLED;
        equip |= bda::equip::floppy_drives(fw.config.floppy_count as u16);
    }
    if fw.config.math_coprocessor {
        equip |= bda::equip::MATH_COPROCESSOR;
    }
    equip |= bda::equip::VIDEO_MODE_80X25_COLOR;
    if fw.config.dma {
        equip |= bda::equip::DMA_PRESENT;
    }
    equip |= bda::equip::serial_cards(fw.config.serial_count as u16);
    if fw.config.game_io {
        equip |= bda::equip::GAME_IO;
    }
    equip |= bda::equip::printers(fw.config.printer_count as u16);
    fw.set_ax(equip);
    fw.set_cf(false);
    true
}

/// INT 12h — get base memory size in KiB.
fn handle_int12<M: Machine>(fw: &mut Firmware<M>) -> bool {
    fw.set_ax(fw.config.memory_kib);
    fw.set_cf(false);
    true
}
