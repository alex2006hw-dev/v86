//! POST (Power-On Self Test) and INT 19h bootstrap.
//!
//! POST initializes the BDA, registers drives, parses El Torito
//! boot catalogs, installs firmware IVT markers, and then runs
//! the INT 19h bootstrap loader.

use crate::backend::{BlockBackend, DriveKind};
use crate::bda;
use crate::eltorito::{self, BootInfo, ElToritoError};

use crate::ivt;
use crate::machine::{Machine, SegReg};
use crate::status;
use crate::Firmware;

/// Run POST: initialize the machine firmware state.
pub fn run_post<M: Machine>(fw: &mut Firmware<M>) {
    // 1. Zero the IVT (the CPU starts with garbage vectors).
    ivt::clear_ivt(&mut fw.machine);

    // 2. Initialize the BDA.
    init_bda(fw);

    // 3. Build the E820 memory map.
    build_e820(fw);

    // 4. Install firmware IVT markers for all service vectors.
    install_firmware_vectors(fw);

    // 5. Set up the video state.
    fw.video.mode = 0x03; // 80x25 color text
    fw.video.cols = 80;
    fw.video.rows = 25;
    fw.video.page = 0;
    bda::set_crt_mode(&mut fw.machine, 0x03);
    bda::set_crt_cols(&mut fw.machine, 80);
    bda::write_ivt_entry(&mut fw.machine, 0x43, 0xF000, 0x0000); // font

    // 6. Mark POST complete.
    fw.pending_boot = true;
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
    let mem_kib = fw.config.memory_kib as u64;
    let total_bytes = (mem_kib as u64) * 1024;

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

/// Install firmware IVT markers for all BIOS service vectors.
fn install_firmware_vectors<M: Machine>(fw: &mut Firmware<M>) {
    for vector in [
        0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x19, 0x1A,
    ] {
        ivt::install_firmware_vector(&mut fw.machine, vector);
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
    // No bootable device found.
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
        return false;
    }
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
        return false;
    }
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
        if parse_eltorito(fw, number).is_err() {
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
            fw.machine.write_ip(0);
            fw.machine.write_seg(SegReg::Cs, load_seg);
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
    fw.machine.write_ip(0);
    fw.machine.write_seg(SegReg::Cs, 0x07C0);
    fw.set_cf(false);
}
