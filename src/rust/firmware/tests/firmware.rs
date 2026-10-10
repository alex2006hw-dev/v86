//! Tests for the v86-firmware crate.

use v86_firmware::backend::{
    BlockBackend, BlockInfo, DriveKind, Geometry, RamDisk, SECTOR_SIZE, CD_SECTOR_SIZE,
};
use v86_firmware::bda;
use v86_firmware::dispatch::{bios_interrupt, Config, Firmware};
use v86_firmware::eltorito::{
    self, BootInfo, ElToritoError, MEDIA_1440K_FLOPPY, MEDIA_NO_EMULATION, ISO_SECTOR,
};
use v86_firmware::int13::DriveTable;
use v86_firmware::machine::{Flag, KeyEvent, Machine, Reg, RtcReading, SegReg};
use v86_firmware::status;

// ------------------------------------------------------------------
// Test machine implementation
// ------------------------------------------------------------------

/// A simple in-memory machine for testing.
struct TestMachine {
    memory: Vec<u8>,
    regs: [u32; 8],
    sregs: [u16; 6],
    eflags: u32,
    ip: u32,
    key_queue: Vec<KeyEvent>,
    rtc: RtcReading,
}

impl TestMachine {
    fn new() -> TestMachine {
        TestMachine {
            memory: vec![0u8; 1024 * 1024], // 1 MB
            regs: [0; 8],
            sregs: [0; 6],
            eflags: 0,
            ip: 0,
            key_queue: Vec::new(),
            rtc: RtcReading {
                year: 2026,
                month: 10,
                day: 9,
                hour: 12,
                minute: 0,
                second: 0,
                day_of_week: 5,
            },
        }
    }
}

impl Machine for TestMachine {
    fn read_u8(&mut self, addr: u32) -> u8 {
        self.memory[addr as usize]
    }
    fn write_u8(&mut self, addr: u32, val: u8) {
        self.memory[addr as usize] = val;
    }

    fn read_reg(&self, r: Reg) -> u32 {
        self.regs[r as usize]
    }
    fn write_reg(&mut self, r: Reg, v: u32) {
        self.regs[r as usize] = v;
    }

    fn read_seg(&self, r: SegReg) -> u16 {
        self.sregs[r as usize]
    }
    fn write_seg(&mut self, r: SegReg, v: u16) {
        self.sregs[r as usize] = v;
    }

    fn read_flag(&self, f: Flag) -> bool {
        (self.eflags & (1 << (f as u32))) != 0
    }
    fn write_flag(&mut self, f: Flag, v: bool) {
        if v {
            self.eflags |= 1 << (f as u32);
        } else {
            self.eflags &= !(1 << (f as u32));
        }
    }

    fn read_ip(&self) -> u32 {
        self.ip
    }
    fn write_ip(&mut self, v: u32) {
        self.ip = v;
    }

    fn poll_key(&mut self) -> Option<KeyEvent> {
        if self.key_queue.is_empty() {
            None
        } else {
            Some(self.key_queue.remove(0))
        }
    }

    fn yield_cpu(&mut self) {}

    fn rtc_time(&self) -> RtcReading {
        self.rtc
    }

    fn request_reset(&mut self) {}
}

// ------------------------------------------------------------------
// Helper functions
// ------------------------------------------------------------------

fn make_firmware() -> Firmware<TestMachine> {
    let machine = TestMachine::new();
    let config = Config::default();
    Firmware::new(machine, config)
}

fn make_floppy() -> RamDisk {
    let info = BlockInfo {
        kind: DriveKind::Floppy,
        geometry: v86_firmware::backend::FLOPPY_1440K,
        sector_size: SECTOR_SIZE,
        total_sectors: 2880,
        removable: true,
    };
    let mut data = vec![0u8; 2880 * 512];
    // Boot signature.
    data[510] = 0x55;
    data[511] = 0xAA;
    RamDisk::new(info, data)
}

fn make_hd() -> RamDisk {
    let info = BlockInfo {
        kind: DriveKind::HardDisk,
        geometry: v86_firmware::backend::hd_geometry(1024 * 1024),
        sector_size: SECTOR_SIZE,
        total_sectors: 1024 * 1024,
        removable: false,
    };
    let mut data = vec![0u8; 1024 * 1024 * 512];
    // Boot signature.
    data[510] = 0x55;
    data[511] = 0xAA;
    RamDisk::new(info, data)
}

// ------------------------------------------------------------------
// BDA tests
// ------------------------------------------------------------------

#[test]
fn test_bda_equipment_word() {
    let mut fw = make_firmware();
    bda::set_equipment_word(&mut fw.machine, 0x1234);
    assert_eq!(bda::equipment_word(&mut fw.machine), 0x1234);
}

#[test]
fn test_bda_memory_size() {
    let mut fw = make_firmware();
    bda::set_memory_kib(&mut fw.machine, 640);
    assert_eq!(bda::memory_kib(&mut fw.machine), 640);
}

#[test]
fn test_bda_keyboard_buffer() {
    let mut fw = make_firmware();
    bda::kbd_clear(&mut fw.machine);
    assert!(bda::kbd_store(&mut fw.machine, b'A', 0x1E));
    assert!(bda::kbd_store(&mut fw.machine, b'B', 0x30));
    let (a, s) = bda::kbd_fetch(&mut fw.machine).unwrap();
    assert_eq!(a, b'A');
    assert_eq!(s, 0x1E);
    let (a, s) = bda::kbd_fetch(&mut fw.machine).unwrap();
    assert_eq!(a, b'B');
    assert_eq!(s, 0x30);
    assert!(bda::kbd_fetch(&mut fw.machine).is_none());
}

#[test]
fn test_bda_cursor() {
    let mut fw = make_firmware();
    bda::set_cursor_pos(&mut fw.machine, 0, 10, 20);
    let (row, col) = bda::cursor_pos(&mut fw.machine, 0);
    assert_eq!(row, 10);
    assert_eq!(col, 20);
}

// ------------------------------------------------------------------
// INT 13h tests
// ------------------------------------------------------------------

#[test]
fn test_int13_reset() {
    let mut fw = make_firmware();
    fw.drives.add(0x00, Box::new(make_floppy()));
    fw.machine.write_reg8(Reg::Eax, true, 0x00); // AH=00h
    fw.machine.write_reg8(Reg::Edx, false, 0x00); // DL=0x00
    bios_interrupt(&mut fw, 0x13);
    assert!(!fw.machine.read_flag(Flag::Cf));
    assert_eq!(fw.machine.read_reg8(Reg::Eax, true), status::SUCCESS);
}

#[test]
fn test_int13_read_sectors_chs() {
    let mut fw = make_firmware();
    fw.drives.add(0x00, Box::new(make_floppy()));
    // AH=02h, AL=1 sector, CH=0, CL=1, DH=0, DL=0x00, ES:BX=0x0000:0x7C00
    fw.machine.write_reg8(Reg::Eax, true, 0x02);
    fw.machine.write_reg8(Reg::Eax, false, 1);
    fw.machine.write_reg8(Reg::Ecx, true, 0); // CH=0
    fw.machine.write_reg8(Reg::Ecx, false, 1); // CL=1
    fw.machine.write_reg8(Reg::Edx, true, 0); // DH=0
    fw.machine.write_reg8(Reg::Edx, false, 0x00); // DL=0x00
    fw.machine.write_seg(SegReg::Es, 0x0000);
    fw.machine.write_reg(Reg::Ebx, 0x7C00);
    bios_interrupt(&mut fw, 0x13);
    assert!(!fw.machine.read_flag(Flag::Cf));
    // Check that the boot signature was read.
    assert_eq!(fw.machine.read_u8(0x7C00 + 510), 0x55);
    assert_eq!(fw.machine.read_u8(0x7C00 + 511), 0xAA);
}

#[test]
fn test_int13_edd_install_check() {
    let mut fw = make_firmware();
    fw.drives.add(0x80, Box::new(make_hd()));
    // AH=41h, BX=55AAh, DL=0x80
    fw.machine.write_reg8(Reg::Eax, true, 0x41);
    fw.machine.write_reg(Reg::Ebx, 0x55AA);
    fw.machine.write_reg8(Reg::Edx, false, 0x80);
    bios_interrupt(&mut fw, 0x13);
    assert!(!fw.machine.read_flag(Flag::Cf));
    assert_eq!(fw.machine.read_reg(Reg::Ebx), 0xAA55);
}

#[test]
fn test_int13_edd_read_sectors() {
    let mut fw = make_firmware();
    fw.drives.add(0x80, Box::new(make_hd()));
    // Set up DAP at 0x0000:0x0100
    let dap_addr = 0x0100u32;
    fw.machine.write_u8(dap_addr, 0x10); // size
    fw.machine.write_u8(dap_addr + 1, 0); // reserved
    fw.machine.write_u16(dap_addr + 2, 1); // count
    fw.machine.write_u16(dap_addr + 4, 0x7C00); // buffer offset
    fw.machine.write_u16(dap_addr + 6, 0x0000); // buffer segment
    fw.machine.write_u32(dap_addr + 8, 0); // LBA low
    fw.machine.write_u32(dap_addr + 12, 0); // LBA high
    // AH=42h, DL=0x80, DS:SI=0x0000:0x0100
    fw.machine.write_reg8(Reg::Eax, true, 0x42);
    fw.machine.write_reg8(Reg::Edx, false, 0x80);
    fw.machine.write_seg(SegReg::Ds, 0x0000);
    fw.machine.write_reg(Reg::Esi, dap_addr as u32);
    bios_interrupt(&mut fw, 0x13);
    assert!(!fw.machine.read_flag(Flag::Cf));
    assert_eq!(fw.machine.read_u8(0x7C00 + 510), 0x55);
    assert_eq!(fw.machine.read_u8(0x7C00 + 511), 0xAA);
}

#[test]
fn test_int13_edd_get_drive_parameters() {
    let mut fw = make_firmware();
    fw.drives.add(0x80, Box::new(make_hd()));
    // AH=48h, DL=0x80, ES:DI=0x0000:0x0200
    fw.machine.write_reg8(Reg::Eax, true, 0x48);
    fw.machine.write_reg8(Reg::Edx, false, 0x80);
    fw.machine.write_seg(SegReg::Es, 0x0000);
    fw.machine.write_reg(Reg::Edi, 0x0200);
    bios_interrupt(&mut fw, 0x13);
    assert!(!fw.machine.read_flag(Flag::Cf));
    // Check the params structure.
    let size = fw.machine.read_u16(0x0200);
    assert_eq!(size, 30); // EDD_PARAMS_SIZE
}

// ------------------------------------------------------------------
// INT 10h tests
// ------------------------------------------------------------------

#[test]
fn test_int10_set_mode() {
    let mut fw = make_firmware();
    fw.machine.write_reg8(Reg::Eax, true, 0x00); // AH=00h
    fw.machine.write_reg8(Reg::Eax, false, 0x03); // AL=0x03
    bios_interrupt(&mut fw, 0x10);
    assert!(!fw.machine.read_flag(Flag::Cf));
    assert_eq!(fw.video.mode, 0x03);
}

#[test]
fn test_int10_tty_write() {
    let mut fw = make_firmware();
    fw.video.mode = 0x03;
    fw.video.cols = 80;
    fw.video.rows = 25;
    fw.machine.write_reg8(Reg::Eax, true, 0x0E); // AH=0Eh
    fw.machine.write_reg8(Reg::Eax, false, b'A'); // AL='A'
    bios_interrupt(&mut fw, 0x10);
    assert!(!fw.machine.read_flag(Flag::Cf));
    // Check that 'A' was written to video memory.
    assert_eq!(fw.machine.read_u8(0xB8000), b'A');
}

#[test]
fn test_int10_get_mode() {
    let mut fw = make_firmware();
    fw.video.mode = 0x03;
    fw.video.cols = 80;
    fw.video.page = 0;
    fw.machine.write_reg8(Reg::Eax, true, 0x0F); // AH=0Fh
    bios_interrupt(&mut fw, 0x10);
    assert!(!fw.machine.read_flag(Flag::Cf));
    assert_eq!(fw.machine.read_reg8(Reg::Eax, false), 0x03); // AL=mode
    assert_eq!(fw.machine.read_reg8(Reg::Eax, true), 80); // AH=cols
}

// ------------------------------------------------------------------
// INT 16h tests
// ------------------------------------------------------------------

#[test]
fn test_int16_peek_key_empty() {
    let mut fw = make_firmware();
    fw.machine.write_reg8(Reg::Eax, true, 0x01); // AH=01h
    bios_interrupt(&mut fw, 0x16);
    assert!(fw.machine.read_flag(Flag::Zf)); // ZF=1, no key
}

#[test]
fn test_int16_read_key() {
    let mut fw = make_firmware();
    fw.machine.key_queue.push(KeyEvent {
        scancode: 0x1E, // 'A' key (produces 'a' without shift)
        pressed: true,
    });
    fw.machine.write_reg8(Reg::Eax, true, 0x00); // AH=00h
    bios_interrupt(&mut fw, 0x16);
    assert!(!fw.machine.read_flag(Flag::Cf));
    assert_eq!(fw.machine.read_reg8(Reg::Eax, false), b'a'); // AL=ASCII
    assert_eq!(fw.machine.read_reg8(Reg::Eax, true), 0x1E); // AH=scancode
}

// ------------------------------------------------------------------
// INT 15h tests
// ------------------------------------------------------------------

#[test]
fn test_int15_a20_enable() {
    let mut fw = make_firmware();
    fw.machine.write_reg(Reg::Eax, 0x2401); // AX=2401h
    bios_interrupt(&mut fw, 0x15);
    assert!(!fw.machine.read_flag(Flag::Cf));
    assert!(fw.a20_enabled);
}

#[test]
fn test_int15_a20_disable() {
    let mut fw = make_firmware();
    fw.machine.write_reg(Reg::Eax, 0x2400); // AX=2400h
    bios_interrupt(&mut fw, 0x15);
    assert!(!fw.machine.read_flag(Flag::Cf));
    assert!(!fw.a20_enabled);
}

#[test]
fn test_int15_a20_query() {
    let mut fw = make_firmware();
    fw.a20_enabled = true;
    fw.machine.write_reg(Reg::Eax, 0x2402); // AX=2402h
    bios_interrupt(&mut fw, 0x15);
    assert!(!fw.machine.read_flag(Flag::Cf));
    assert_eq!(fw.machine.read_reg8(Reg::Eax, false), 1); // AL=1 (enabled)
}

#[test]
fn test_int15_e820() {
    let mut fw = make_firmware();
    fw.config.memory_kib = 640;
    fw.e820.push(v86_firmware::dispatch::E820Entry {
        base: 0,
        length: 0x9FC00,
        kind: 1,
    });
    fw.e820.push(v86_firmware::dispatch::E820Entry {
        base: 0x9FC00,
        length: 0x400,
        kind: 2,
    });
    fw.machine.write_reg(Reg::Eax, 0xE820); // AX=E820h
    fw.machine.write_reg(Reg::Ebx, 0); // continuation
    fw.machine.write_seg(SegReg::Es, 0x0000);
    fw.machine.write_reg(Reg::Edi, 0x0300);
    bios_interrupt(&mut fw, 0x15);
    assert!(!fw.machine.read_flag(Flag::Cf));
    // Check first entry.
    let base = fw.machine.read_u64(0x0300);
    assert_eq!(base, 0);
    let length = fw.machine.read_u64(0x0308);
    assert_eq!(length, 0x9FC00);
    let kind = fw.machine.read_u32(0x0310);
    assert_eq!(kind, 1);
}

// ------------------------------------------------------------------
// INT 1Ah tests
// ------------------------------------------------------------------

#[test]
fn test_int1a_read_rtc_time() {
    let mut fw = make_firmware();
    fw.machine.write_reg8(Reg::Eax, true, 0x00); // AH=00h
    bios_interrupt(&mut fw, 0x1A);
    assert!(!fw.machine.read_flag(Flag::Cf));
    // CH=seconds (BCD), CL=minutes (BCD), DH=hours (BCD)
    let seconds = fw.machine.read_reg8(Reg::Ecx, true);
    let minutes = fw.machine.read_reg8(Reg::Ecx, false);
    let hours = fw.machine.read_reg8(Reg::Edx, true);
    // 12:00:00
    assert_eq!(seconds, 0x00);
    assert_eq!(minutes, 0x00);
    assert_eq!(hours, 0x12);
}

#[test]
fn test_int1a_read_rtc_date() {
    let mut fw = make_firmware();
    fw.machine.write_reg8(Reg::Eax, true, 0x02); // AH=02h
    bios_interrupt(&mut fw, 0x1A);
    assert!(!fw.machine.read_flag(Flag::Cf));
    // CH=century, CL=year, DH=month, DL=day
    let century = fw.machine.read_reg8(Reg::Ecx, true);
    let year = fw.machine.read_reg8(Reg::Ecx, false);
    let month = fw.machine.read_reg8(Reg::Edx, true);
    let day = fw.machine.read_reg8(Reg::Edx, false);
    // 2026-10-09
    assert_eq!(century, 0x20);
    assert_eq!(year, 0x26);
    assert_eq!(month, 0x10);
    assert_eq!(day, 0x09);
}

// ------------------------------------------------------------------
// El Torito tests
// ------------------------------------------------------------------

#[test]
fn test_eltorito_not_iso() {
    let info = BlockInfo {
        kind: DriveKind::CdRom,
        geometry: v86_firmware::backend::CD_ROM_GEOMETRY,
        sector_size: CD_SECTOR_SIZE,
        total_sectors: 1024,
        removable: true,
    };
    let data = vec![0u8; 1024 * 2048];
    let mut cd = RamDisk::new(info, data);
    let result = eltorito::parse_boot_info(&mut cd);
    assert!(matches!(result, Err(ElToritoError::NotIso)));
}

#[test]
fn test_eltorito_emulated_media_sectors() {
    assert_eq!(eltorito::emulated_media_sectors(MEDIA_1440K_FLOPPY), Some(2880));
    assert_eq!(eltorito::emulated_media_sectors(MEDIA_NO_EMULATION), None);
}

// ------------------------------------------------------------------
// POST tests
// ------------------------------------------------------------------

#[test]
fn test_post_init() {
    let mut fw = make_firmware();
    v86_firmware::post::run_post(&mut fw);
    // Check that firmware vectors were installed (marker seg:off at F000:0000).
    assert_eq!(fw.machine.read_u16(0x10 * 4), 0x0000); // INT 10h offset
    assert_eq!(fw.machine.read_u16(0x10 * 4 + 2), 0xF000); // INT 10h segment
    // Check that the BDA was initialized.
    assert_eq!(bda::memory_kib(&mut fw.machine), 640);
    // Check that firmware vectors were installed.
    assert_eq!(fw.machine.read_u16(0x13 * 4), 0x0000); // INT 13h offset
    assert_eq!(fw.machine.read_u16(0x13 * 4 + 2), 0xF000); // INT 13h segment
}

// ------------------------------------------------------------------
// VBE tests
// ------------------------------------------------------------------

#[test]
fn test_vbe_get_controller_info() {
    let mut fw = make_firmware();
    // Install the VBE ROM data first (normally done during POST).
    v86_firmware::vbe::install_vbe_rom(&mut fw);
    fw.machine.write_reg(Reg::Eax, 0x4F00); // AX=4F00h
    fw.machine.write_seg(SegReg::Es, 0x0000);
    fw.machine.write_reg(Reg::Edi, 0x0400);
    bios_interrupt(&mut fw, 0x10);
    assert!(!fw.machine.read_flag(Flag::Cf));
    assert_eq!(fw.machine.read_reg(Reg::Eax), 0x004F);
    // Check signature.
    assert_eq!(fw.machine.read_u8(0x0400), b'V');
    assert_eq!(fw.machine.read_u8(0x0401), b'B');
    assert_eq!(fw.machine.read_u8(0x0402), b'E');
    assert_eq!(fw.machine.read_u8(0x0403), b'2');
}

#[test]
fn test_vbe_get_mode_info() {
    let mut fw = make_firmware();
    fw.machine.write_reg(Reg::Eax, 0x4F01); // AX=4F01h
    fw.machine.write_reg(Reg::Ecx, 0x101); // mode 0x101
    fw.machine.write_seg(SegReg::Es, 0x0000);
    fw.machine.write_reg(Reg::Edi, 0x0500);
    bios_interrupt(&mut fw, 0x10);
    assert!(!fw.machine.read_flag(Flag::Cf));
    assert_eq!(fw.machine.read_reg(Reg::Eax), 0x004F);
    // Check width/height.
    let width = fw.machine.read_u16(0x0500 + 18);
    let height = fw.machine.read_u16(0x0500 + 20);
    assert_eq!(width, 640);
    assert_eq!(height, 480);
}

#[test]
fn test_vbe_set_mode() {
    let mut fw = make_firmware();
    fw.machine.write_reg(Reg::Eax, 0x4F02); // AX=4F02h
    fw.machine.write_reg(Reg::Ebx, 0x101); // mode 0x101
    bios_interrupt(&mut fw, 0x10);
    assert!(!fw.machine.read_flag(Flag::Cf));
    assert_eq!(fw.machine.read_reg(Reg::Eax), 0x004F);
    assert_eq!(fw.vbe.current_mode, 0x101);
}

#[test]
fn test_vbe_get_mode() {
    let mut fw = make_firmware();
    fw.vbe.current_mode = 0x101;
    fw.machine.write_reg(Reg::Eax, 0x4F03); // AX=4F03h
    bios_interrupt(&mut fw, 0x10);
    assert!(!fw.machine.read_flag(Flag::Cf));
    assert_eq!(fw.machine.read_reg(Reg::Ebx), 0x101);
}

// ------------------------------------------------------------------
// Geometry tests
// ------------------------------------------------------------------

#[test]
fn test_chs_to_lba() {
    let geo = Geometry {
        cylinders: 80,
        heads: 2,
        sectors_per_track: 18,
    };
    // C=0, H=0, S=1 -> LBA 0
    assert_eq!(geo.chs_to_lba(0, 0, 1), Some(0));
    // C=0, H=0, S=18 -> LBA 17
    assert_eq!(geo.chs_to_lba(0, 0, 18), Some(17));
    // C=0, H=1, S=1 -> LBA 18
    assert_eq!(geo.chs_to_lba(0, 1, 1), Some(18));
    // C=1, H=0, S=1 -> LBA 36
    assert_eq!(geo.chs_to_lba(1, 0, 1), Some(36));
    // Invalid: sector 0
    assert_eq!(geo.chs_to_lba(0, 0, 0), None);
    // Invalid: head >= heads
    assert_eq!(geo.chs_to_lba(0, 2, 1), None);
}

#[test]
fn test_lba_to_chs() {
    let geo = Geometry {
        cylinders: 80,
        heads: 2,
        sectors_per_track: 18,
    };
    assert_eq!(geo.lba_to_chs(0), (0, 0, 1));
    assert_eq!(geo.lba_to_chs(17), (0, 0, 18));
    assert_eq!(geo.lba_to_chs(18), (0, 1, 1));
    assert_eq!(geo.lba_to_chs(36), (1, 0, 1));
}

// ------------------------------------------------------------------
// INT 11h/12h tests
// ------------------------------------------------------------------

#[test]
fn test_int11_equipment() {
    let mut fw = make_firmware();
    bios_interrupt(&mut fw, 0x11);
    assert!(!fw.machine.read_flag(Flag::Cf));
    let equip = fw.machine.read_reg(Reg::Eax) as u16;
    assert!(equip & bda::equip::FLOPPY_INSTALLED != 0);
    assert!(equip & bda::equip::VIDEO_MODE_80X25_COLOR != 0);
}

#[test]
fn test_int12_memory() {
    let mut fw = make_firmware();
    bios_interrupt(&mut fw, 0x12);
    assert!(!fw.machine.read_flag(Flag::Cf));
    assert_eq!(fw.machine.read_reg(Reg::Eax), 640);
}
