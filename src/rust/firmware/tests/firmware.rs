//! Tests for the v86-firmware crate.

use v86_firmware::backend::{
    BlockInfo, DriveKind, Geometry, RamDisk, SECTOR_SIZE, CD_SECTOR_SIZE,
};
use v86_firmware::bda;
use v86_firmware::dispatch::{dispatch_service, firmware_service, Config, Firmware};
use v86_firmware::eltorito::{
    self, ElToritoError, MEDIA_1440K_FLOPPY, MEDIA_NO_EMULATION,
};
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
    /// SS:SP, so the tests can drive `firmware_service` the way the CPU
    /// does: the ROM stub pushes a service id and then traps.
    stack: Vec<u16>,
    /// Index in `stack` of the IP slot of the interrupt frame a guest's
    /// `int n` pushed, if one is currently outstanding. `patch_saved_flags`
    /// needs it: the FLAGS image is `SS:SP + 4`, three words above whatever
    /// SS:SP happens to be once the service id has been popped.
    frame_ip: Option<usize>,
    key_queue: Vec<KeyEvent>,
    rtc: RtcReading,
    /// Far calls services asked for instead of performing them.
    chains: Vec<(u16, u16)>,
}

impl TestMachine {
    fn new() -> TestMachine {
        TestMachine {
            memory: vec![0u8; 1024 * 1024], // 1 MB
            regs: [0; 8],
            sregs: [0; 6],
            eflags: 0,
            ip: 0,
            stack: Vec::new(),
            frame_ip: None,
            key_queue: Vec::new(),
            chains: Vec::new(),
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

    fn pop_stack_u16(&mut self) -> u16 {
        self.stack.pop().unwrap_or(0)
    }

    fn push_u16(&mut self, value: u16) {
        self.stack.push(value);
    }

    fn peek_service_id(&self) -> u32 {
        self.stack.last().copied().unwrap_or(0xFFFF) as u32
    }

    fn peek_stack_pointer(&self) -> u32 {
        self.stack.len() as u32
    }

    fn patch_saved_flags(&mut self) {
        // The CPU pushes IP, then CS, then FLAGS, so the FLAGS image is the
        // third word above the frame's IP. Patching anywhere else silently
        // corrupts the return address instead, which the regression test
        // `saved_flags_are_patched_in_place_not_over_the_return_address`
        // exists to catch.
        let Some(ip_slot) = self.frame_ip else { return };
        let slot = ip_slot + 2;
        if let Some(word) = self.stack.get_mut(slot)
        {
            *word = (*word & !1) | (self.eflags & 1) as u16;
        }
    }

    /// Far calls recorded instead of performed, so a test can assert that
    /// an interrupt chained.
    fn chain_to(&mut self, segment: u16, offset: u16) {
        self.chains.push((segment, offset));
    }
}

impl TestMachine {
    /// Push a service id, as a ROM stub does before trapping.
    fn push_service(&mut self, id: u16) {
        self.stack.push(id);
    }

    /// Model a guest's `int n`: the CPU pushes FLAGS, CS and IP, and only
    /// then does the ROM stub push its service id.
    fn push_int_frame(&mut self, ip: u16, cs: u16, flags: u16) {
        self.stack.push(ip);
        self.stack.push(cs);
        self.stack.push(flags);
        self.frame_ip = Some(self.stack.len() - 3);
    }

    /// The FLAGS word of the outstanding frame, as `iret` would restore it.
    fn frame_flags(&self) -> Option<u16> {
        self.frame_ip.map(|i| self.stack[i + 2])
    }
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
    dispatch_service(&mut fw, 0x13);
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
    dispatch_service(&mut fw, 0x13);
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
    dispatch_service(&mut fw, 0x13);
    assert!(!fw.machine.read_flag(Flag::Cf));
    assert_eq!(fw.machine.read_reg(Reg::Ebx), 0xAA55);

    // The rest of the answer is as specified, and each field was wrong
    // before: AH carried the classic INT 13h success code instead of the
    // Extensions major version, DH carried a nonsense 0x20, and DL still
    // held the drive number the caller asked about instead of the number of
    // drives. Found by running the same boot sector under
    // `examples/firmware-oracle.js`.
    assert_eq!(fw.machine.read_reg8(Reg::Eax, true), 0x03, "AH = EDD major version");
    assert_eq!(fw.machine.read_reg8(Reg::Edx, true), 0x00, "DH = EDD minor version");
    assert_eq!(fw.machine.read_reg8(Reg::Edx, false), 1, "DL = number of drives");
    assert_eq!(fw.machine.read_reg(Reg::Ecx), 0x0007, "CX = extension bitmap");
}

#[test]
fn test_int13_edd_install_check_reports_the_drive_count() {
    // DL counts drives, so adding a second one has to change it.
    let mut fw = make_firmware();
    fw.drives.add(0x00, Box::new(make_floppy()));
    fw.drives.add(0x80, Box::new(make_hd()));

    fw.machine.write_reg8(Reg::Eax, true, 0x41);
    fw.machine.write_reg(Reg::Ebx, 0x55AA);
    fw.machine.write_reg8(Reg::Edx, false, 0x00);
    dispatch_service(&mut fw, 0x13);

    assert!(!fw.machine.read_flag(Flag::Cf));
    assert_eq!(fw.machine.read_reg8(Reg::Edx, false), 2);
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
    dispatch_service(&mut fw, 0x13);
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
    dispatch_service(&mut fw, 0x13);
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
    dispatch_service(&mut fw, 0x10);
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
    dispatch_service(&mut fw, 0x10);
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
    dispatch_service(&mut fw, 0x10);
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
    dispatch_service(&mut fw, 0x16);
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
    dispatch_service(&mut fw, 0x16);
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
    dispatch_service(&mut fw, 0x15);
    assert!(!fw.machine.read_flag(Flag::Cf));
    assert!(fw.a20_enabled);
}

#[test]
fn test_int15_a20_disable() {
    let mut fw = make_firmware();
    fw.machine.write_reg(Reg::Eax, 0x2400); // AX=2400h
    dispatch_service(&mut fw, 0x15);
    assert!(!fw.machine.read_flag(Flag::Cf));
    assert!(!fw.a20_enabled);
}

#[test]
fn test_int15_a20_query() {
    let mut fw = make_firmware();
    fw.a20_enabled = true;
    fw.machine.write_reg(Reg::Eax, 0x2402); // AX=2402h
    dispatch_service(&mut fw, 0x15);
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
    dispatch_service(&mut fw, 0x15);
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
    dispatch_service(&mut fw, 0x1A);
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
    dispatch_service(&mut fw, 0x1A);
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

/// Build a minimal El Torito image: a primary volume descriptor, a boot
/// record pointing at a catalogue, and a catalogue with a no-emulation
/// default entry.
fn make_bootable_iso(catalog_lba: u32, image_lba: u32) -> RamDisk {
    let sectors = 64usize;
    let info = BlockInfo {
        kind: DriveKind::CdRom,
        geometry: v86_firmware::backend::CD_ROM_GEOMETRY,
        sector_size: CD_SECTOR_SIZE,
        total_sectors: sectors as u64,
        removable: true,
    };
    let mut data = vec![0u8; sectors * 2048];

    let put = |data: &mut Vec<u8>, lba: usize, f: &dyn Fn(&mut [u8])| {
        let mut s = vec![0u8; 2048];
        s[0] = if lba == 16 { 0x01 } else if lba == 17 { 0x00 } else { 0xFF };
        s[1..6].copy_from_slice(b"CD001");
        s[6] = 0x01;
        f(&mut s);
        data[lba * 2048..lba * 2048 + 2048].copy_from_slice(&s);
    };

    put(&mut data, 16, &|_| {});
    put(&mut data, 17, &|s| {
        s[7..7 + 23].copy_from_slice(b"EL TORITO SPECIFICATION");
        s[0x47..0x4B].copy_from_slice(&catalog_lba.to_le_bytes());
        s[0x4B] = 0x00; // no emulation
    });
    put(&mut data, 18, &|_| {});
    put(&mut data, catalog_lba as usize, &|s| {
        // The catalogue sector opens with the validation entry itself: no
        // signature. That is how real images do it -- Debian's isohybrid
        // boot info block at LBA 1119 is a validation entry with nothing
        // before it -- and it is what this firmware expects.
        //
        // Every byte not set below stays zero, including the identifier
        // string: any stray character there changes the checksum and the
        // entry is rejected.
        let mut v = [0u8; 32];
        v[0] = 0x01;                       // header id
        v[1] = 0x00;                       // platform: 80x86
        v[30] = 0x55;                      // key bytes
        v[31] = 0xAA;
        // The sixteen little-endian words must sum to zero. Word 0 is the
        // header id (1) and word 15 is 0xAA55 (43605), so word 7 supplies
        // 65536 - 43605 - 1 = 21930, which is 0x55AA.
        v[14] = 0xAA;
        v[15] = 0x55;
        s[0..32].copy_from_slice(&v);

        let mut e = [0u8; 32];
        e[0] = 0x88;                       // boot indicator
        e[1] = 0x00;                       // no emulation
        e.write_uint16_le(2, 0x07C0);      // load segment
        e.write_uint16_le(6, 1);           // virtual sector count
        e.write_uint32_le(8, image_lba);   // load RBA
        s[32..64].copy_from_slice(&e);
    });
    data[image_lba as usize * 2048 + 510] = 0x55;
    data[image_lba as usize * 2048 + 511] = 0xAA;

    RamDisk::new(info, data)
}

/// A tiny little-endian writer, so the test does not need unsafe.
trait LeWrite {
    fn write_uint16_le(&mut self, offset: usize, value: u16);
    fn write_uint32_le(&mut self, offset: usize, value: u32);
}

impl LeWrite for [u8] {
    fn write_uint16_le(&mut self, offset: usize, value: u16)
    {
        self[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }
    fn write_uint32_le(&mut self, offset: usize, value: u32)
    {
        self[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
}

#[test]
fn test_eltorito_parses_a_bootable_image() {
    // Regression test. `is_volume_descriptor` compared six bytes of the
    // sector against the five bytes of "CD001", so it was never true and
    // every bootable image was rejected as NotIso. Only the negative case
    // was ever tested, which is why nothing caught it.
    let mut cd = make_bootable_iso(20, 21);
    let info = eltorito::parse_boot_info(&mut cd).expect("a bootable image must parse");

    assert_eq!(info.catalog_lba, 20);
    assert_eq!(info.default_entry.media_type, MEDIA_NO_EMULATION);
    assert_eq!(info.default_entry.load_rba, 21);
    assert_eq!(info.default_entry.load_segment, 0x07C0);
}

#[test]
fn test_eltorito_rejects_a_bad_catalogue_checksum() {
    let mut cd = make_bootable_iso(20, 21);
    // Corrupt the key bytes the entry must carry.
    cd.data[20 * 2048 + 31] ^= 0xFF;
    let result = eltorito::parse_boot_info(&mut cd);
    assert!(
        matches!(result, Err(ElToritoError::InvalidCatalog)),
        "a bad catalogue checksum must be refused, got {:?}",
        result.err()
    );
}

#[test]
fn test_eltorito_rejects_a_missing_boot_record() {
    // No boot record: the volume descriptor scan must reach the terminator
    // and give up rather than reading a catalogue from wherever.
    let mut cd = make_bootable_iso(20, 21);
    let sector = 17 * 2048;
    cd.data[sector..sector + 2048].fill(0);
    cd.data[sector + 18] = 0xFF; // terminator

    let result = eltorito::parse_boot_info(&mut cd);
    assert!(matches!(result, Err(ElToritoError::NotElTorito)));
}

#[test]
fn test_int15_e820_terminates_with_carry_set() {
    // The map ends with a *failing* call, not with EBX reset to zero.
    // Resetting it made the exhaustion guard unreachable, so a caller
    // walking the map by cursor was handed the first entry forever.
    let mut fw = make_firmware();

    // The map is built by POST, which this harness does not run, so give it
    // the shape a small machine would report.
    fw.e820 = vec![
        v86_firmware::dispatch::E820Entry { base: 0, length: 0x9_FC00, kind: 1 },
        v86_firmware::dispatch::E820Entry {
            base: 0x10_0000,
            length: 0x7F_0000,
            kind: 1,
        },
    ];

    // Walk the whole map the way a loader does: EBX is the cursor and the
    // loop stops when carry comes back set.
    let mut bx = 0u32;
    let mut entries = Vec::new();
    loop
    {
        // INT 15h dispatches on AH, so the selector has to be in AX every
        // time -- not just the first call.
        fw.machine.write_reg(Reg::Eax, 0xE820);
        fw.machine.write_reg(Reg::Ebx, bx);
        dispatch_service(&mut fw, 0x15);

        if fw.machine.read_flag(Flag::Cf)
        {
            assert_eq!(
                fw.machine.read_reg8(Reg::Eax, true),
                0x04,
                "the terminating call reports AH=04h"
            );
            break;
        }

        entries.push(bx);
        bx = fw.machine.read_reg(Reg::Ebx);
        assert!(entries.len() < 32, "the map must terminate");
    }

    assert_eq!(entries.len(), 2, "both entries were returned, got {:?}", entries);
    assert_eq!(
        entries,
        vec![0, 1],
        "entries were returned in cursor order"
    );
    assert_eq!(bx, 2, "the cursor points past the last entry when the map ends");
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
fn test_post_installs_real_rom_stubs() {
    let mut fw = make_firmware();
    // install_roms is what the host does before the CPU leaves reset;
    // run_post is what the ROM itself does once it is running.
    v86_firmware::post::install_roms(&mut fw);
    v86_firmware::post::run_post(&mut fw);

    // The IVT holds far pointers; the ROM window check wants a physical
    // address, so convert rather than comparing packed values.
    let mut far = |v: u32| {
        fw.machine.read_u16(v * 4) as u32 | ((fw.machine.read_u16(v * 4 + 2) as u32) << 16)
    };
    let physical = |p: u32| ((p >> 16) << 4) + (p & 0xFFFF);

    // INT 10h belongs to the video option ROM, not the system BIOS.
    let ptr = far(0x10);
    assert_eq!(ptr >> 16, 0xC000, "INT 10h should point into the VGA option ROM");
    assert!(v86_firmware::rom::is_firmware_rom(physical(ptr)));

    // The BIOS services belong to the system ROM.
    for v in [0x11u32, 0x12, 0x13, 0x15, 0x16, 0x19] {
        let ptr = far(v);
        assert_eq!(
            ptr >> 16, 0xF000,
            "INT {:02X}h should point into the system ROM, got {:04X}:{:04X}",
            v, ptr >> 16, ptr & 0xFFFF
        );
        assert!(
            v86_firmware::rom::is_firmware_rom(physical(ptr)),
            "INT {:02X}h lands outside every firmware ROM",
            v
        );
    }

    // INT 1Ah is shared: the system BIOS stubs it and the video ROM
    // overrides it, because the VBE information block is found through
    // INT 1Ah AH=4Fh. The video ROM must win.
    let ptr = far(0x1A);
    assert_eq!(ptr >> 16, 0xC000, "INT 1Ah belongs to the video ROM");

    // And the code at each vector really is a trap stub.
    let at10 = far(0x10) & 0xFFFF;
    let seg10 = (far(0x10) >> 16) as u16;
    let base = u32::from(seg10) << 4;
    assert_eq!(fw.machine.read_u8(base + at10), 0x68, "push service id");
    assert_eq!(
        fw.machine.read_u8(base + at10 + 3),
        0xCD,
        "int"
    );
    assert_eq!(
        fw.machine.read_u8(base + at10 + 4),
        v86_firmware::rom::TRAP_VECTOR
    );

    // The BDA is initialised.
    assert_eq!(bda::memory_kib(&mut fw.machine), 640);
}

#[test]
fn test_reset_vector_is_reachable_in_the_system_rom() {
    let mut fw = make_firmware();
    v86_firmware::post::install_roms(&mut fw);
    let reset = v86_firmware::rom::SYSTEM_ROM_BASE + 0xFFF0;
    assert_eq!(fw.machine.read_u8(reset), 0xE9, "reset vector jumps to POST");
}

// ------------------------------------------------------------------
// Option ROM tests
// ------------------------------------------------------------------

/// A valid 512-byte option ROM image, as the host would build one.
fn make_option_rom(entry: &[u8]) -> Vec<u8> {
    let mut data = vec![0u8; 512];
    data[0] = 0x55;
    data[1] = 0xAA;
    data[2] = 1;
    for (i, b) in entry.iter().enumerate() {
        data[3 + i] = *b;
    }
    let sum: u32 = data.iter().map(|&b| u32::from(b)).sum();
    data[511] = (-(sum as i32) & 0xFF) as u8;
    data
}

#[test]
fn option_roms_are_registered_and_left_for_a_second_post() {
    let mut fw = make_firmware();
    fw.option_roms.push(make_option_rom(&[0xCB])); // retf
    assert_eq!(fw.option_roms.len(), 1);
}

#[test]
fn a_registered_option_rom_is_placed_where_a_guest_can_read_it() {
    let mut fw = make_firmware();
    let rom = make_option_rom(&[0xCB]); // retf
    fw.option_roms.push(rom.clone());
    v86_firmware::post::install_roms(&mut fw);
    v86_firmware::option_rom::run_all(&mut fw);

    // It lands at the base of the run, byte for byte.
    let base = v86_firmware::option_rom::OPTION_ROM_BASE;
    for (i, b) in rom.iter().enumerate() {
        assert_eq!(fw.machine.read_u8(base + i as u32), *b, "byte {} differs", i);
    }
}

#[test]
fn a_registered_option_rom_is_entered_with_a_far_call() {
    let mut fw = make_firmware();
    fw.option_roms.push(make_option_rom(&[0xCB])); // retf
    v86_firmware::post::install_roms(&mut fw);
    v86_firmware::option_rom::run_all(&mut fw);

    // CS:IP names the entry at offset 3 of the placed image.
    let base = v86_firmware::option_rom::OPTION_ROM_BASE;
    let segment = (base >> 4) as u16;
    assert_eq!(fw.machine.read_seg(SegReg::Cs), segment);
    assert_eq!(fw.machine.read_ip(), 3);

    // The stack holds the far-return frame: offset pushed first, then the
    // segment, so a `retf` finds CS at [SP] and IP at [SP+2].
    let stack = &fw.machine.stack;
    let top = stack.len();
    assert_eq!(stack[top - 1], v86_firmware::rom::SYSTEM_ROM_SEG);
    let return_offset = stack[top - 2] as u32;
    assert!(return_offset > 0, "no return address was pushed");
}

#[test]
fn an_option_rom_return_lands_next_to_the_int_19h_stub() {
    let mut fw = make_firmware();
    fw.option_roms.push(make_option_rom(&[0xCB]));
    v86_firmware::post::install_roms(&mut fw);
    v86_firmware::option_rom::run_all(&mut fw);

    // Whatever the ROM returns to has to be code in the system ROM, and
    // it has to reach the bootstrap rather than fall into the filler.
    // A far call pushes the offset first and the segment second, so the
    // offset is the second word from the top.
    let top = fw.machine.stack.len();
    let return_offset = fw.machine.stack[top - 2] as u32;
    let stub = fw.roms.as_ref().unwrap().stub(0x19).unwrap() as u32;
    assert!(v86_firmware::rom::is_firmware_rom(
        v86_firmware::rom::SYSTEM_ROM_BASE + return_offset
    ));
    // The first byte is the far jump, and its target is the INT 19h stub.
    let at = v86_firmware::rom::SYSTEM_ROM_BASE + return_offset;
    assert_eq!(fw.machine.read_u8(at), 0xEA, "the return path is a far jump");
    let target = fw.machine.read_u16(at + 1) as u32;
    assert_eq!(target, stub, "the return path jumps to the INT 19h stub");
}

#[test]
fn an_invalid_option_rom_is_not_entered_but_does_not_stall_the_rest() {
    let mut fw = make_firmware();
    // A corrupt image: broken checksum.
    let mut bad = make_option_rom(&[0xCB]);
    bad[0x40] ^= 0xFF;
    fw.option_roms.push(bad);
    // A good one behind it.
    fw.option_roms.push(make_option_rom(&[0xCB]));
    v86_firmware::post::install_roms(&mut fw);
    v86_firmware::option_rom::run_all(&mut fw);

    // The good ROM is at the *second* slot, because rejection skips past
    // what the bad image claimed.
    let second = v86_firmware::option_rom::OPTION_ROM_BASE + 512;
    assert_eq!(fw.machine.read_seg(SegReg::Cs), (second >> 4) as u16);
    assert_eq!(fw.machine.read_ip(), 3);
    // And the good ROM's own bytes are there, not the rejected one's.
    assert_eq!(fw.machine.read_u8(second), 0x55);
    assert_eq!(fw.machine.read_u8(second + 1), 0xAA);
}

#[test]
fn post_runs_option_roms_before_the_bootstrap() {
    let mut fw = make_firmware();
    fw.option_roms.push(make_option_rom(&[0xCB]));
    v86_firmware::post::install_roms(&mut fw);
    v86_firmware::post::run_post(&mut fw);

    // run_post ends by handing over to a registered ROM, so the CPU is
    // left in it and the normal bootstrap never runs.
    let base = v86_firmware::option_rom::OPTION_ROM_BASE;
    assert_eq!(fw.machine.read_seg(SegReg::Cs), (base >> 4) as u16,
        "POST should have entered the option ROM");
    assert!(fw.pending_boot, "POST should still report completion");
}

#[test]
fn a_second_option_rom_is_placed_after_the_first() {
    let mut fw = make_firmware();
    let first = make_option_rom(&[0xCB]);
    let second = make_option_rom(&[0xCB]);
    fw.option_roms.push(first.clone());
    fw.option_roms.push(second.clone());
    v86_firmware::post::install_roms(&mut fw);
    v86_firmware::option_rom::run_all(&mut fw);

    // Only the first is entered -- the second is placed for a guest that
    // scans, which is what a real BIOS does too.
    let base = v86_firmware::option_rom::OPTION_ROM_BASE;
    for (i, b) in first.iter().enumerate() {
        assert_eq!(fw.machine.read_u8(base + i as u32), *b);
    }
    for (i, b) in second.iter().enumerate() {
        assert_eq!(fw.machine.read_u8(base + 512 + i as u32), *b, "byte {} of the second", i);
    }
    // Both ROMs are run, in registration order, so the CPU is left in the
    // second one.
    assert_eq!(fw.machine.read_seg(SegReg::Cs), ((base + 512) >> 4) as u16);
    assert_eq!(fw.machine.read_ip(), 3);
}

#[test]
fn an_option_rom_that_does_not_fit_stops_the_run_rather_than_overflowing() {
    let mut fw = make_firmware();
    // 64 KiB of valid image would consume the whole run, so there is
    // nowhere for a second one.
    let mut big = make_option_rom(&[0xCB]);
    big.resize(64 * 1024, 0);
    big[2] = 128; // 128 blocks = 64 KiB
    // The checksum byte has to be zero while the sum is taken, or the
    // value it replaces is still counted.
    big[511] = 0;
    let sum: u32 = big.iter().map(|&b| u32::from(b)).sum();
    big[511] = (-(sum as i32) & 0xFF) as u8;
    assert_eq!(v86_firmware::option_rom::validate(&big), Ok(64 * 1024));
    fw.option_roms.push(big);
    fw.option_roms.push(make_option_rom(&[0xCB]));
    v86_firmware::post::install_roms(&mut fw);
    v86_firmware::option_rom::run_all(&mut fw);

    // The first filled the run, so the second was not placed: reading
    // past the run is out of bounds and would panic if it had tried.
    let base = v86_firmware::option_rom::OPTION_ROM_BASE;
    assert_eq!(fw.machine.read_seg(SegReg::Cs), (base >> 4) as u16);
    assert_eq!(fw.machine.read_ip(), 3);
}


#[test]
fn e820_covers_all_installed_memory() {
    // A guest reads E820 to decide where its page tables may go, so a map
    // that stops short of the machine is a bug rather than a limit. This
    // is exactly what the bzImage cold boot hit: the map was built from
    // the 16-bit conventional figure and ended at 64 MiB on a 128 MiB
    // machine, so the kernel placed page tables where the emulator has
    // no memory.
    let mut fw = make_firmware();
    fw.config.memory_kib = 640;
    fw.config.total_memory_kib = 128 * 1024;
    v86_firmware::post::install_roms(&mut fw);
    v86_firmware::post::run_post(&mut fw);

    let last = fw.e820.last().expect("the map is not empty");
    assert_eq!(last.kind, 1, "the final entry is usable memory");
    assert_eq!(
        last.base + last.length,
        128 * 1024 * 1024,
        "E820 must describe all installed memory, not the conventional figure"
    );
}

#[test]
fn int15_extended_memory_reports_installed_not_conventional() {
    let mut fw = make_firmware();
    fw.config.memory_kib = 640;
    fw.config.total_memory_kib = 64 * 1024;

    // AH=88h: extended memory above 1 MiB, so 64 MiB less the first MiB.
    fw.machine.write_reg(Reg::Eax, 0x88_00);
    assert!(dispatch_service(&mut fw, 0x15));
    assert_eq!(fw.machine.read_reg(Reg::Eax) & 0xFFFF, (64 * 1024 - 1024) as u32, "AX");

    // AX=E801h: CX counts 1-16 MiB and DX the 64 KiB blocks above it.
    fw.machine.write_reg(Reg::Eax, 0xE801);
    assert!(dispatch_service(&mut fw, 0x15));
    assert_eq!(fw.machine.read_reg(Reg::Ecx) & 0xFFFF, 15 * 1024, "CX");
    assert_eq!(fw.machine.read_reg(Reg::Edx) & 0xFFFF,
        ((64 * 1024 - 1024 - 15 * 1024) / 64) as u32, "DX");
}

// ------------------------------------------------------------------
// VBE tests
// ------------------------------------------------------------------

#[test]
fn test_vbe_get_controller_info() {
    let mut fw = make_firmware();
    // Install the VBE ROM data first (normally done during POST).
    // The VBE data now lives in the video option ROM; install_roms
    // writes both images and the IVT that points at them.
    v86_firmware::post::install_roms(&mut fw);
    fw.machine.write_reg(Reg::Eax, 0x4F00); // AX=4F00h
    fw.machine.write_seg(SegReg::Es, 0x0000);
    fw.machine.write_reg(Reg::Edi, 0x0400);
    dispatch_service(&mut fw, 0x10);
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
    dispatch_service(&mut fw, 0x10);
    assert!(!fw.machine.read_flag(Flag::Cf));
    assert_eq!(fw.machine.read_reg(Reg::Eax), 0x004F);
    // Check width/height.
    let width = fw.machine.read_u16(0x0500 + 18);
    let height = fw.machine.read_u16(0x0500 + 20);
    assert_eq!(width, 640);
    assert_eq!(height, 480);
}

/// A video chip that can produce every advertised mode.
struct FakeVideo {
    mode: u16,
    linear: bool,
    stride: u16,
    dac: Vec<u8>,
}

impl v86_firmware::vbe::VideoHost for FakeVideo {
    fn set_mode(&mut self, mode: u16, linear: bool) -> bool {
        self.mode = mode;
        self.linear = linear;
        self.stride = v86_firmware::vbe::bytes_per_scanline(
            &v86_firmware::vbe::VBE_MODES
                .iter()
                .find(|m| m.mode == mode)
                .copied()
                .unwrap_or(v86_firmware::vbe::VBE_MODES[0]),
        );
        true
    }
    fn current_mode(&self) -> u16 { self.mode }
    fn linear_framebuffer(&self) -> u32 { 0xE000_0000 }
    fn stride(&self) -> u16 { self.stride }
    fn display_start(&self) -> (u16, u16) { (0, 0) }
    fn set_display_start(&mut self, _x: u16, _y: u16) -> bool {
        // This fake does not model a display start address; recording one
        // would let a test assert on state the rest of the firmware never
        // reads back.
        true
    }
    fn dac(&self, first: u32, count: u32) -> Vec<u8> {
        let a = (first * 3) as usize;
        let b = a + (count * 3) as usize;
        self.dac[a..b].to_vec()
    }
    fn set_dac(&mut self, first: u32, count: u32, data: &[u8]) {
        let a = (first * 3) as usize;
        let b = a + (count * 3) as usize;
        self.dac[a..b].copy_from_slice(&data[..(count * 3) as usize]);
    }
    fn dac_entries(&self) -> u32 { 256 }
    fn set_dac_width(&mut self, _bits: u8) {}
    fn dac_width(&self) -> u8 { 6 }
    fn window(&self) -> (u16, u16, u16) { (0xA000, 64, 64) }
    fn save_state(&self) -> Vec<u8> { vec![0u8; 256] }
    fn restore_state(&mut self, _state: &[u8]) -> bool { true }
}

#[test]
fn test_vbe_set_mode_programs_the_chip() {
    let mut fw = make_firmware();
    fw.set_video_chip(Box::new(FakeVideo { mode: 0xFFFF, linear: false, stride: 0, dac: vec![0u8; 768] }));
    fw.machine.write_reg(Reg::Eax, 0x4F02); // AX=4F02h
    fw.machine.write_reg(Reg::Ebx, 0x4117); // mode 0x117 + linear framebuffer
    dispatch_service(&mut fw, 0x10);
    assert!(!fw.machine.read_flag(Flag::Cf));
    assert_eq!(fw.machine.read_reg(Reg::Eax), 0x004F);
    assert_eq!(fw.vbe.current_mode, 0x117);
    assert!(fw.vbe.linear, "bit 14 must be recorded for 4F03h");

    // 4F03h echoes the mode back with the LFB bit still set.
    fw.machine.write_reg(Reg::Eax, 0x4F03);
    dispatch_service(&mut fw, 0x10);
    assert_eq!(fw.machine.read_reg(Reg::Ebx), 0x4117);
}

#[test]
fn test_vbe_set_mode_fails_on_an_unknown_mode() {
    let mut fw = make_firmware();
    fw.set_video_chip(Box::new(FakeVideo { mode: 0xFFFF, linear: false, stride: 0, dac: vec![0u8; 768] }));
    fw.machine.write_reg(Reg::Eax, 0x4F02);
    fw.machine.write_reg(Reg::Ebx, 0x199); // not in the mode table
    dispatch_service(&mut fw, 0x10);
    assert!(fw.machine.read_flag(Flag::Cf), "unknown mode must set carry");
}

#[test]
fn test_vbe_set_mode_fails_when_the_chip_cannot_produce_it() {
    // NullVideoHost cannot program anything: reporting success here would
    // hand the guest a black screen.
    let mut fw = make_firmware();
    fw.machine.write_reg(Reg::Eax, 0x4F02);
    fw.machine.write_reg(Reg::Ebx, 0x101);
    dispatch_service(&mut fw, 0x10);
    assert!(fw.machine.read_flag(Flag::Cf));
    assert_ne!(fw.machine.read_reg(Reg::Eax) & 0xFF00, 0x4F00);
}

#[test]
fn test_vbe_scanline_length_reports_the_real_stride() {
    // 4F06h used to echo its input back, which is how panning silently
    // broke. It must report what the chip is actually scanning out.
    let mut fw = make_firmware();
    fw.set_video_chip(Box::new(FakeVideo { mode: 0xFFFF, linear: false, stride: 0, dac: vec![0u8; 768] }));
    fw.machine.write_reg(Reg::Eax, 0x4F02);
    fw.machine.write_reg(Reg::Ebx, 0x111); // 640x480x16
    dispatch_service(&mut fw, 0x10);
    fw.machine.write_reg(Reg::Ecx, 0x9999); // a request the chip ignores
    fw.machine.write_reg(Reg::Edx, 0x9999);
    fw.set_bl(1);
    fw.machine.write_reg(Reg::Eax, 0x4F06);
    dispatch_service(&mut fw, 0x10);
    assert!(!fw.machine.read_flag(Flag::Cf));
    assert_eq!(fw.cx(), 640 * 2, "stride must be width * bytes-per-pixel");
}

#[test]
fn test_vbe_palette_data_moves_bytes() {
    let mut fw = make_firmware();
    fw.set_video_chip(Box::new(FakeVideo { mode: 0xFFFF, linear: false, stride: 0, dac: vec![0u8; 768] }));
    // Write three entries at index 0.
    fw.machine.write_u8(0x0600, 0x11);
    fw.machine.write_u8(0x0601, 0x22);
    fw.machine.write_u8(0x0602, 0x33);
    fw.set_bl(0);
    fw.set_bh(0);
    fw.set_cx(1);
    fw.set_dx(0);
    fw.set_es(0);
    fw.set_di(0x0600);
    fw.machine.write_reg(Reg::Eax, 0x4F09);
    dispatch_service(&mut fw, 0x10);
    assert!(!fw.machine.read_flag(Flag::Cf), "palette write must succeed");

    // And read one back.
    fw.set_bl(1);
    fw.set_di(0x0700);
    fw.machine.write_reg(Reg::Eax, 0x4F09);
    dispatch_service(&mut fw, 0x10);
    assert!(!fw.machine.read_flag(Flag::Cf));
    assert_eq!(fw.machine.read_u8(0x0700), 0x11);
    assert_eq!(fw.machine.read_u8(0x0701), 0x22);
    assert_eq!(fw.machine.read_u8(0x0702), 0x33);
}

#[test]
fn test_vbe_get_mode() {
    let mut fw = make_firmware();
    fw.vbe.current_mode = 0x101;
    fw.machine.write_reg(Reg::Eax, 0x4F03); // AX=4F03h
    dispatch_service(&mut fw, 0x10);
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
    dispatch_service(&mut fw, 0x11);
    assert!(!fw.machine.read_flag(Flag::Cf));
    let equip = fw.machine.read_reg(Reg::Eax) as u16;
    assert!(equip & bda::equip::FLOPPY_INSTALLED != 0);
    assert!(equip & bda::equip::VIDEO_MODE_80X25_COLOR != 0);
}

#[test]
fn test_int12_memory() {
    let mut fw = make_firmware();
    dispatch_service(&mut fw, 0x12);
    assert!(!fw.machine.read_flag(Flag::Cf));
    assert_eq!(fw.machine.read_reg(Reg::Eax), 640);
}

// ------------------------------------------------------------------
// Interrupt frame tests
//
// The ROM stubs trap by executing `int 0x66`, which fires *before* the
// CPU pushes anything, so the only thing on the stack when the service
// runs is the service id. Popping it leaves the frame the *guest's*
// original `int n` pushed: IP at SS:SP, CS at SS:SP+2, FLAGS at
// SS:SP+4. The stub's `iret` restores that frame, which makes the
// saved FLAGS word the only channel through which a service can return
// carry to its caller.
// ------------------------------------------------------------------

#[test]
fn saved_flags_carry_reaches_the_caller() {
    let mut fw = make_firmware();

    // A guest `int 13h` with carry *clear* on entry.
    fw.machine.push_int_frame(0x1234, 0x07C0, 0x0202);
    fw.machine.push_service(0x13);

    // Read a drive that does not exist: the service reports failure.
    fw.machine.write_reg(Reg::Edx, 0x7F);
    firmware_service(&mut fw);

    // The stub's `iret` will restore this word, so this is the carry the
    // caller observes.
    assert_eq!(
        fw.machine.frame_flags(),
        Some(0x0203),
        "carry set by the service must reach the caller through the saved FLAGS"
    );
}

#[test]
fn saved_flags_are_patched_in_place_not_over_the_return_address() {
    // Regression test. Patching at SS:SP instead of SS:SP+4 writes the
    // carry bit into the *low byte of the saved return address*, which
    // redirects the caller into the middle of the instruction it was
    // about to run. It is invisible in any test that only looks at the
    // flag, and in the emulator it hangs the guest inside its own boot
    // sector.
    let mut fw = make_firmware();

    let return_ip = 0x012Bu16;
    fw.machine.push_int_frame(return_ip, 0x07C0, 0x0202);
    fw.machine.push_service(0x13);

    fw.machine.write_reg(Reg::Edx, 0x7F);
    firmware_service(&mut fw);

    let frame_ip = fw.machine.frame_ip.expect("frame outstanding");
    let stack = &fw.machine.stack;

    assert_eq!(
        stack[frame_ip],
        return_ip,
        "the saved return address must be byte-for-byte unchanged"
    );
    assert_eq!(stack[frame_ip + 1], 0x07C0, "the saved CS must be unchanged");
}

#[test]
fn saved_flags_are_untouched_when_no_frame_is_outstanding() {
    // POST traps from ROM with nothing pushed but its own service id, so
    // there is no guest frame to patch. That must be a no-op rather than
    // a wild write.
    let mut fw = make_firmware();

    // A sentinel below the service id, standing in for whatever the ROM had
    // on its stack when it trapped.
    const SENTINEL: u16 = 0xBEEF;
    fw.machine.stack.push(SENTINEL);
    fw.machine.push_service(v86_firmware::rom::SERVICE_POST);

    firmware_service(&mut fw);

    assert_eq!(
        fw.machine.stack,
        vec![SENTINEL],
        "with no frame outstanding, the service id is popped and nothing else is written"
    );
}
