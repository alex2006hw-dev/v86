//! ROM installation, POST state, and the INT 19h bootstrap.
//!
//! Two different things happen at power-on and it is worth keeping them
//! apart:
//!
//! * [`install_roms`] runs once, from the host, before the CPU leaves
//!   reset. It copies the ROM images into guest memory and points the
//!   real IVT at the ROM stubs. The CPU is not executing yet, so this
//!   can be as direct as it likes.
//! * [`run_post`] runs from inside the ROM, on the ROM stub's own
//!   stack, after the CPU has entered at `F000:0000`. It builds the
//!   BIOS Data Area and the memory map, and installs the vectors whose
//!   targets are only known once the machine configuration is.
//!
//! Anything that has to happen before the first instruction executes
//! belongs in the first; anything that must observe real-mode CPU state
//! belongs in the second.

use crate::backend::{BlockBackend, DriveKind};
use crate::bda;
use crate::debug::tag;
use crate::eltorito::{self, BootInfo, ElToritoError};

use crate::ivt;
use crate::machine::{Machine, SegReg};
use crate::rom;
use crate::status;
use crate::Firmware;

/// Copy the ROM images into guest memory and point the IVT at them.
///
/// Runs from the host with the CPU held in reset, so it uses plain
/// memory writes rather than anything that depends on register state.
pub fn install_roms<M: Machine>(fw: &mut Firmware<M>) {
    let system = rom::build_system_rom();
    let vga = rom::build_vga_rom();

    for (i, b) in system.image.iter().enumerate() {
        fw.machine.write_u8(rom::SYSTEM_ROM_BASE + i as u32, *b);
    }
    for (i, b) in vga.image.iter().enumerate() {
        fw.machine.write_u8(rom::VGA_ROM_BASE + i as u32, *b);
    }

    // Point every vector that has a stub at its real ROM entry.
    for vector in 0u16..=0xFF {
        if let Some(off) = system.layout.stub(vector as u8) {
            ivt::write_vector(&mut fw.machine, vector as u8, rom::SYSTEM_ROM_SEG, off);
        }
    }
    // The video ROM owns INT 10h, so its stub wins over the system one.
    ivt::write_vector(&mut fw.machine, 0x10, rom::VGA_ROM_SEG, vga.layout.int10);
    ivt::write_vector(&mut fw.machine, 0x43, rom::VGA_ROM_SEG, vga.layout.int43);
    ivt::write_vector(&mut fw.machine, 0x1A, rom::VGA_ROM_SEG, vga.layout.int1a);

    // INT 1Ah AH=4Fh is the conventional way to find the VBE
    // information block, so the vector must reach the video ROM.
    bda::write_ivt_entry(&mut fw.machine, 0x43, rom::VGA_ROM_SEG, vga.layout.font);

    fw.roms = Some(system.layout);
    fw.vga_rom = Some(vga.layout);
}

/// Run POST from inside the ROM.
pub fn run_post<M: Machine>(fw: &mut Firmware<M>) {
    fw.trace(tag::POST, "POST begin");
    // 1. Zero the IVT. POST owns every vector it does not hand back, and
    //    the CPU may have been reset with stale ones in place. Vectors
    //    that survive are the exception timer (0x1C) and the ones the
    //    video ROM already published.
    let published = [
        (0x10u8, rom::VGA_ROM_SEG),
        (0x1Au8, rom::VGA_ROM_SEG),
        (0x43u8, rom::VGA_ROM_SEG),
    ];
    for v in 0u8..=0xFF {
        if !published.iter().any(|(vec, _)| *vec == v) {
            ivt::write_vector(&mut fw.machine, v, 0, 0);
        }
    }

    // 2. Re-install the vectors we own, now that the IVT is clean.
    if let Some(layout) = fw.roms.clone() {
        for v in 0u8..=0xFF {
            if let Some(off) = layout.stub(v) {
                // INT 10h/43h/1Ah belong to the video option ROM.
                if v == 0x10 || v == 0x1A || v == 0x43 {
                    continue;
                }
                ivt::write_vector(&mut fw.machine, v, rom::SYSTEM_ROM_SEG, off);
            }
        }
    }
    if let Some(vga) = fw.vga_rom {
        ivt::write_vector(&mut fw.machine, 0x10, rom::VGA_ROM_SEG, vga.int10);
        ivt::write_vector(&mut fw.machine, 0x43, rom::VGA_ROM_SEG, vga.font);
        ivt::write_vector(&mut fw.machine, 0x1A, rom::VGA_ROM_SEG, vga.int1a);
    }

    // 3. Initialize the BDA.
    init_bda(fw);

    // 4. Build the E820 memory map.
    build_e820(fw);

    // 5. Set up the video state. This has to happen before anything
    //    prints: the POST banner writes through the text page.
    fw.video.mode = 0x03; // 80x25 colour text
    fw.video.cols = 80;
    fw.video.rows = 25;
    fw.video.page = 0;
    bda::set_crt_mode(&mut fw.machine, 0x03);
    bda::set_crt_cols(&mut fw.machine, 80);

    // 6. Narrate the result. A BIOS that says nothing on the screen is
    //    very hard to debug, and this is what a user looks at first.
    banner(fw, "WorkAgent");
    banner(fw, "v86 permissive firmware");
    banner(fw, "");

    // 7. Report what POST found. These are the numbers a user can check
    // against the emulated hardware, so print them rather than only
    // tracing them.
    let mem = bda::memory_kib(&mut fw.machine);
    let equip = bda::equipment_word(&mut fw.machine);
    let drives = drive_summary(fw);
    banner(fw, &format!("Memory: {} KB", mem));
    banner(fw, &format!("Drives: {}", drives));
    fw.trace(
        tag::POST,
        &format!("POST done: {} KiB, equipment {:#06X}", mem, equip),
    );

    // 7. Run any option ROM the host registered. This has to come after
    //    the BDA and the drive table exist -- a ROM reads them -- and
    //    before the bootstrap, because the whole point of most ROMs is
    //    to be that bootstrap or to prepare it.
    //
    //    A ROM that does not return (the Linux boot stub ends in a far
    //    jump) means the rest of this function never runs.
    crate::option_rom::run_all(fw);

    // 8. Mark POST complete.
    fw.pending_boot = true;
    fw.trace(tag::POST, "POST complete");
}

/// Write one line to the screen through INT 10h AH=0Eh.
///
/// The firmware is its own video BIOS, so this is the same path a guest
/// would take; doing it through the service rather than poking 0xB8000
/// keeps the cursor and scroll bookkeeping correct.
fn banner<M: Machine>(fw: &mut Firmware<M>, line: &str)
{
    for ch in line.chars().take(78) {
        putc(fw, ch as u8);
    }
    putc(fw, b'\r');
    putc(fw, b'\n');
}

/// INT 10h AH=0Eh — write one character at the cursor.
fn putc<M: Machine>(fw: &mut Firmware<M>, ch: u8)
{
    // CRLF: the emulator's text page is not in raw mode.
    let (row, col) = bda::cursor_pos(&mut fw.machine, fw.video.page);
    if ch == b'\n'
    {
        let cols = fw.video.cols as u32;
        let rows = fw.video.rows as u32;
        let row = row as u32 + 1;

        if row >= rows
        {
            crate::int10::scroll_up_full(fw);
            bda::set_cursor_pos(&mut fw.machine, fw.video.page, (rows - 1) as u8, 0);
        }
        else
        {
            bda::set_cursor_pos(&mut fw.machine, fw.video.page, row as u8, 0);
        }
        let _ = (cols, col);
        return;
    }

    let cols = fw.video.cols as u32;
    let mut col = col as u32;
    let mut row = row as u32;

    if col >= cols
    {
        col = 0;
        row += 1;
    }
    if row >= fw.video.rows as u32
    {
        crate::int10::scroll_up_full(fw);
        row = fw.video.rows as u32 - 1;
    }

    let addr = crate::int10::VIDEO_COLOR_BASE + (row * cols + col) * 2;
    fw.machine.write_u8(addr, ch);
    fw.machine.write_u8(addr + 1, 0x07);
    bda::set_cursor_pos(&mut fw.machine, fw.video.page, row as u8, (col + 1) as u8);
}

/// One-line summary of the registered drives, e.g. "1 floppy, 1 hard".
fn drive_summary<M: Machine>(fw: &mut Firmware<M>) -> String {
    let floppies = fw.drives.count(DriveKind::Floppy);
    let hds = fw.drives.count(DriveKind::HardDisk);
    let cds = fw.drives.count(DriveKind::CdRom);

    let mut parts = Vec::new();

    if floppies > 0 {
        parts.push(format!("{} floppy", floppies));
    }
    if hds > 0 {
        parts.push(format!("{} hard", hds));
    }
    if cds > 0 {
        parts.push(format!("{} cd", cds));
    }
    if parts.is_empty() {
        "none".to_string()
    }
    else {
        parts.join(", ")
    }
}

/// Initialize the BIOS Data Area.
fn init_bda<M: Machine>(fw: &mut Firmware<M>) {
    // Equipment word.
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
    bda::set_equipment_word(&mut fw.machine, equip);

    // Memory size.
    bda::set_memory_kib(&mut fw.machine, fw.config.memory_kib);

    // Keyboard buffer.
    bda::kbd_clear(&mut fw.machine);
    fw.machine.write_u8(bda::KBD_SHIFT_FLAGS, 0);
    fw.machine.write_u8(bda::KBD_SHIFT_FLAGS + 1, 0);

    // CRT defaults.
    bda::set_crt_mode(&mut fw.machine, 0x03);
    bda::set_crt_cols(&mut fw.machine, 80);
    fw.machine.write_u8(bda::CRT_6845_BASE, 0xD4); // 0x3D4
    fw.machine.write_u8(bda::CRT_6845_BASE + 1, 0x03);

    // Reset flag: 0x0000 = cold boot.
    fw.machine.write_u16(bda::RESET_FLAG, 0x0000);

    // Timer ticks.
    fw.machine.write_u32(bda::TIMER_TICKS, 0);

    // Floppy motor status.
    fw.machine.write_u8(bda::FLOPPY_MOTOR, 0);
}

/// Build the E820 memory map.
fn build_e820<M: Machine>(fw: &mut Firmware<M>) {
    // The map describes *installed* memory, so the total and not the
    // conventional figure: a kernel reads this to decide where it may put
    // page tables, and a map that stops short of the machine it is on
    // makes it place them where the emulator has none.
    let total_bytes = u64::from(fw.config.total_memory_kib) * 1024;

    fw.e820.clear();

    // 0x00000000–0x0009FC00: usable (conventional memory).
    let conventional_end = 0x9FC00u64.min(total_bytes);
    if conventional_end > 0 {
        fw.e820.push(crate::dispatch::E820Entry {
            base: 0,
            length: conventional_end,
            kind: 1,
        });
    }

    // 0x0009FC00–0x00100000: reserved (BIOS area).
    if total_bytes > 0x9FC00 {
        fw.e820.push(crate::dispatch::E820Entry {
            base: 0x9FC00,
            length: 0x100000 - 0x9FC00,
            kind: 2,
        });
    }

    // 0x00100000+: usable extended memory.
    if total_bytes > 0x100000 {
        fw.e820.push(crate::dispatch::E820Entry {
            base: 0x100000,
            length: total_bytes - 0x100000,
            kind: 1,
        });
    }
}

/// Register a block backend as an INT 13h drive.
pub fn register_drive<M: Machine>(
    fw: &mut Firmware<M>,
    number: u8,
    backend: Box<dyn BlockBackend>,
) {
    fw.drives.add(number, backend);
}

/// Parse El Torito boot info from a CD-ROM backend and cache it.
pub fn parse_eltorito<M: Machine>(
    fw: &mut Firmware<M>,
    cd_number: u8,
) -> Result<BootInfo, ElToritoError> {
    let backend = fw
        .drives
        .get(cd_number)
        .ok_or(ElToritoError::IoError)?;
    let info = eltorito::parse_boot_info(backend.backend.as_mut())?;
    // Cache the boot catalog for INT 13h AH=4Dh.
    let catalog = eltorito::read_catalog_sector(backend.backend.as_mut(), info.catalog_lba)?;
    fw.cached_catalog = Some(catalog);
    fw.boot_info = Some(info.clone());
    Ok(info)
}

/// INT 19h — bootstrap loader.
///
/// Loads the boot sector from the first bootable device in the
/// configured boot order and jumps to it.
pub fn handle_int19<M: Machine>(fw: &mut Firmware<M>) -> bool {
    fw.trace(tag::BOOT, "INT 19h: bootstrap");
    let boot_order: Vec<&'static str> = fw.config.boot_order.clone();
    for device in boot_order {
        match device {
            "floppy" => {
                if boot_from_floppy(fw) {
                    return true;
                }
            }
            "hd" => {
                if boot_from_hd(fw) {
                    return true;
                }
            }
            "cd" => {
                if boot_from_cd(fw) {
                    return true;
                }
            }
            _ => {}
        }
    }
    // No bootable device found. Say so on the screen: a machine that
    // simply stops after POST is indistinguishable from a hang.
    fw.trace(tag::BOOT, "no bootable device");
    banner(fw, "No bootable device.");
    fw.set_error(status::DRIVE_NOT_READY);
    true
}

/// Attempt to boot from the first floppy drive.
fn boot_from_floppy<M: Machine>(fw: &mut Firmware<M>) -> bool {
    let number = match fw.drives.first_of_kind(DriveKind::Floppy) {
        Some(n) => n,
        None => return false,
    };
    let boot_sector = match read_boot_sector(fw, number) {
        Some(s) => s,
        None => return false,
    };
    if !is_valid_boot_sector(&boot_sector) {
        fw.trace(tag::BOOT, &format!("floppy 0x{:02X}: no 55AA signature", number));
        return false;
    }
    fw.trace(tag::BOOT, &format!("booting floppy 0x{:02X}", number));
    banner(fw, "Booting from floppy...");
    load_and_jump(fw, &boot_sector, number);
    true
}

/// Attempt to boot from the first hard disk.
fn boot_from_hd<M: Machine>(fw: &mut Firmware<M>) -> bool {
    let number = match fw.drives.first_of_kind(DriveKind::HardDisk) {
        Some(n) => n,
        None => return false,
    };
    let boot_sector = match read_boot_sector(fw, number) {
        Some(s) => s,
        None => return false,
    };
    if !is_valid_boot_sector(&boot_sector) {
        fw.trace(tag::BOOT, &format!("hard disk 0x{:02X}: no 55AA signature", number));
        return false;
    }
    fw.trace(tag::BOOT, &format!("booting hard disk 0x{:02X}", number));
    banner(fw, "Booting from hard disk...");
    load_and_jump(fw, &boot_sector, number);
    true
}

/// Attempt to boot from the first CD-ROM (El Torito).
fn boot_from_cd<M: Machine>(fw: &mut Firmware<M>) -> bool {
    let number = match fw.drives.first_of_kind(DriveKind::CdRom) {
        Some(n) => n,
        None => return false,
    };
    // Parse El Torito if not already done.
    if fw.boot_info.is_none() {
        if let Err(error) = parse_eltorito(fw, number) {
            fw.trace(
                tag::BOOT,
                &format!("CD 0x{:02X}: no El Torito boot record ({:?})", number, error),
            );
            return false;
        }
    }
    let info = fw.boot_info.as_ref().unwrap();
    let entry = &info.default_entry;

    match entry.media_type {
        eltorito::MEDIA_NO_EMULATION => {
            // Load virtual sectors to the load segment and jump.
            let load_seg = if entry.load_segment == 0 {
                0x07C0
            } else {
                entry.load_segment
            };
            let count = entry.sector_count as usize;
            let mut buf = vec![0u8; count * 512];
            let backend = fw.drives.get(number).unwrap();
            let rba = entry.load_rba as u64;
            // Read from the CD image (2048-byte sectors).
            let byte_offset = rba * 2048;
            let byte_len = (count * 512) as u64;
            let data = match read_cd_range(backend.backend.as_mut(), byte_offset, byte_len) {
                Some(d) => d,
                None => return false,
            };
            buf.copy_from_slice(&data[..count * 512]);

            // Write to guest memory at load_seg:0000.
            let phys_base = (load_seg as u32) << 4;
            for (i, b) in buf.iter().enumerate() {
                fw.machine.write_u8(phys_base + i as u32, *b);
            }

            // Set up registers and jump.
            fw.machine.write_seg(SegReg::Ds, load_seg);
            fw.machine.write_seg(SegReg::Es, load_seg);
            fw.machine.write_seg(SegReg::Ss, 0);
            fw.machine.write_reg(crate::machine::Reg::Esp, 0xFFFE);
            fw.machine.write_reg(crate::machine::Reg::Eax, 0);
            fw.machine.write_reg(crate::machine::Reg::Ebx, 0);
            fw.machine.write_reg(crate::machine::Reg::Ecx, 0);
            fw.machine.write_reg(crate::machine::Reg::Edx, number as u32);
            fw.machine.write_reg(crate::machine::Reg::Esi, 0);
            fw.machine.write_reg(crate::machine::Reg::Edi, 0);
            fw.machine.write_seg(SegReg::Cs, load_seg);
            fw.machine.write_ip(0);
            // As for the disk path: the bootstrap ran through a software
            // `int 0x19`, which clears IF. Hand over with them enabled.
            fw.machine.write_flag(crate::machine::Flag::If, true);
            fw.set_cf(false);
            true
        }
        eltorito::MEDIA_1200K_FLOPPY
        | eltorito::MEDIA_1440K_FLOPPY
        | eltorito::MEDIA_2880K_FLOPPY => {
            // Floppy emulation: register a virtual floppy drive.
            let emu_number = fw.drives.next_floppy();
            let geo = match entry.media_type {
                eltorito::MEDIA_1200K_FLOPPY => crate::backend::FLOPPY_1200K,
                eltorito::MEDIA_2880K_FLOPPY => crate::backend::FLOPPY_2880K,
                _ => crate::backend::FLOPPY_1440K,
            };
            let total = eltorito::emulated_media_sectors(entry.media_type).unwrap_or(0);
            let cd_backend = fw.drives.get(number).unwrap().backend.as_mut();
            let backend = crate::backend::CdEmulationBackend::new(
                cd_backend,
                entry.load_rba,
                total,
                geo,
                512,
            );
            fw.drives.add(emu_number, Box::new(backend));
            // Boot from the virtual floppy.
            let boot_sector = match read_boot_sector(fw, emu_number) {
                Some(s) => s,
                None => return false,
            };
            if !is_valid_boot_sector(&boot_sector) {
                return false;
            }
            load_and_jump(fw, &boot_sector, emu_number);
            true
        }
        eltorito::MEDIA_HARD_DISK => {
            // Hard disk emulation: register a virtual hard disk.
            let emu_number = fw.drives.next_hd();
            let image_sectors = (entry.sector_count as u64) * 4; // 512-byte sectors
            let geo = eltorito::emulation_hd_geometry(image_sectors);
            let backend = crate::backend::CdEmulationBackend::new(
                fw.drives.get(number).unwrap().backend.as_mut(),
                entry.load_rba,
                image_sectors,
                geo,
                512,
            );
            fw.drives.add(emu_number, Box::new(backend));
            let boot_sector = match read_boot_sector(fw, emu_number) {
                Some(s) => s,
                None => return false,
            };
            if !is_valid_boot_sector(&boot_sector) {
                return false;
            }
            load_and_jump(fw, &boot_sector, emu_number);
            true
        }
        _ => false,
    }
}

/// Read a range of bytes from a CD image (2048-byte sectors).
fn read_cd_range(
    backend: &mut dyn BlockBackend,
    byte_offset: u64,
    byte_len: u64,
) -> Option<Vec<u8>> {
    let lba = byte_offset / 2048;
    let offset_in_sector = (byte_offset % 2048) as usize;
    let end = byte_offset + byte_len;
    let last_lba = (end - 1) / 2048;
    let sector_count = (last_lba - lba + 1) as usize;
    let mut buf = vec![0u8; sector_count * 2048];
    backend.read_sectors(lba, &mut buf).ok()?;
    let result = buf[offset_in_sector..offset_in_sector + byte_len as usize].to_vec();
    Some(result)
}

/// Read the first sector (512 bytes) from a drive.
fn read_boot_sector<M: Machine>(fw: &mut Firmware<M>, number: u8) -> Option<[u8; 512]> {
    let mut buf = [0u8; 512];
    let backend = fw.drives.get(number)?;
    backend.backend.read_sectors(0, &mut buf).ok()?;
    Some(buf)
}

/// Check for the 0xAA55 boot signature.
fn is_valid_boot_sector(sector: &[u8; 512]) -> bool {
    sector[510] == 0x55 && sector[511] == 0xAA
}

/// Load a boot sector to 0x7C00 and jump to it.
fn load_and_jump<M: Machine>(fw: &mut Firmware<M>, sector: &[u8; 512], drive: u8) {
    let phys_base = 0x7C00u32;
    for (i, b) in sector.iter().enumerate() {
        fw.machine.write_u8(phys_base + i as u32, *b);
    }
    // Set up registers for the boot sector.
    fw.machine.write_seg(SegReg::Ds, 0);
    fw.machine.write_seg(SegReg::Es, 0);
    fw.machine.write_seg(SegReg::Ss, 0);
    fw.machine.write_reg(crate::machine::Reg::Esp, 0xFFFE);
    fw.machine.write_reg(crate::machine::Reg::Eax, 0);
    fw.machine.write_reg(crate::machine::Reg::Ebx, 0);
    fw.machine.write_reg(crate::machine::Reg::Ecx, 0);
    fw.machine.write_reg(crate::machine::Reg::Edx, drive as u32);
    fw.machine.write_reg(crate::machine::Reg::Esi, 0);
    fw.machine.write_reg(crate::machine::Reg::Edi, 0);
    // CS first: the instruction pointer is relative to the code segment,
    // so writing IP before CS would compute the offset against the old one.
    fw.machine.write_seg(SegReg::Cs, 0x07C0);
    fw.machine.write_ip(0);
    // Hand over with interrupts enabled. The bootstrap is reached through
    // `int 0x19`, and a software interrupt clears IF in the CPU core, so
    // without this the loader starts with interrupts off and stays off.
    fw.machine.write_flag(crate::machine::Flag::If, true);
    fw.set_cf(false);
}
