//! INT 13h — low-level disk services.
//!
//! Implements the classic CHS functions (AH=00h–18h) and
//! the INT 13h Extensions (EDD: AH=41h–49h) from the T10
//! Enhanced Disk Drive specification, plus the El Torito
//! disk-emulation control functions (AH=4Ah/4Bh/4Ch/4Dh).

use crate::backend::{
    BlockBackend, BlockInfo, DriveKind, Geometry, SECTOR_SIZE,
};
use crate::machine::Machine;
use crate::backend::Int13Status;
use crate::status::{self, INVALID_FUNCTION, SUCCESS};
use crate::{Firmware};

/// One registered INT 13h drive.
pub struct Drive {
    pub number: u8,
    pub backend: Box<dyn BlockBackend>,
    pub last_status: Int13Status,
    pub locked: bool,
}

impl Drive {
    pub fn new(number: u8, backend: Box<dyn BlockBackend>) -> Drive {
        Drive {
            number,
            backend,
            last_status: SUCCESS,
            locked: false,
        }
    }

    pub fn info(&self) -> BlockInfo {
        self.backend.info()
    }

    pub fn set_status(&mut self, st: Int13Status) {
        self.last_status = st;
    }
}

/// The INT 13h drive table.
pub struct DriveTable {
    pub drives: Vec<Drive>,
}

impl DriveTable {
    pub fn new() -> DriveTable {
        DriveTable { drives: Vec::new() }
    }

    /// Register a drive with an explicit INT 13h number.
    pub fn add(&mut self, number: u8, backend: Box<dyn BlockBackend>) {
        self.drives.push(Drive::new(number, backend));
    }

    pub fn get(&mut self, number: u8) -> Option<&mut Drive> {
        self.drives
            .iter_mut()
            .find(|d| d.number == number)
    }

    pub fn count(&self, kind: DriveKind) -> usize {
        self.drives
            .iter()
            .filter(|d| d.backend.info().kind == kind)
            .count()
    }

    /// Next free floppy drive number (0x00–0x7F).
    pub fn next_floppy(&self) -> u8 {
        for n in 0x00u8..=0x7F {
            if !self.drives.iter().any(|d| d.number == n) {
                return n;
            }
        }
        0x7F
    }

    /// Next free hard disk drive number (0x80–0xFF).
    pub fn next_hd(&self) -> u8 {
        for n in 0x80u8..=0xFF {
            if !self.drives.iter().any(|d| d.number == n) {
                return n;
            }
        }
        0xFF
    }

    /// Next free CD-ROM drive number. Per the El Torito
    /// specification, CD-ROM drives are assigned numbers
    /// in the range 0x00–0x0F; numbers already claimed by
    /// floppy drives are skipped.
    pub fn next_cd(&self) -> u8 {
        for n in 0x00u8..=0x0F {
            let claimed = self.drives.iter().any(|d| {
                d.number == n
                    && d.backend.info().kind != DriveKind::CdRom
            });
            if !claimed {
                return n;
            }
        }
        0x0F
    }

    /// First drive of a kind, if any.
    pub fn first_of_kind(&self, kind: DriveKind) -> Option<u8> {
        self.drives
            .iter()
            .find(|d| d.backend.info().kind == kind)
            .map(|d| d.number)
    }
}

impl<M: Machine> Firmware<M> {
    // INT 13h-specific helpers (register accessors are in dispatch.rs).

    fn int13_ok(&mut self, sectors: u8) {
        self.set_cf(false);
        self.set_ah(SUCCESS);
        self.set_al(sectors);
    }

    fn int13_error(&mut self, drive: u8, st: Int13Status, sectors_done: u8) {
        self.set_cf(true);
        self.set_ah(st);
        self.set_al(sectors_done);
        if let Some(d) = self.drives.get(drive) {
            d.set_status(st);
        }
    }

    fn int13_error_nodrive(&mut self, st: Int13Status) {
        self.set_cf(true);
        self.set_ah(st);
    }

    /// Look up the drive in DL, or report "drive not
    /// ready".
    fn drive_in_dl(&mut self) -> Option<u8> {
        let dl = self.dl();
        if self.drives.get(dl).is_some() {
            Some(dl)
        } else {
            self.int13_error_nodrive(status::DRIVE_NOT_READY);
            None
        }
    }
}

// ----------------------------------------------------------------------
// INT 13h dispatch
// ----------------------------------------------------------------------

/// Handle an INT 13h call. `ah` is the function number;
//  all register access goes through the firmware state.
pub fn handle_int13<M: Machine>(fw: &mut Firmware<M>) {
    let ah = fw.ah();
    fw.trace(
        crate::debug::tag::DISK,
        format!("INT 13h AH={:02X}h DL={:02X}h", ah, fw.dl()),
    );
    match ah {
        0x00 => reset_disk_system(fw),
        0x01 => get_last_status(fw),
        0x02 => read_sectors_chs(fw),
        0x03 => write_sectors_chs(fw),
        0x04 => verify_sectors_chs(fw),
        0x05 => format_track(fw),
        0x08 => get_drive_parameters(fw),
        0x0C => seek_cylinder(fw),
        0x0D => reset_hard_disks(fw),
        0x10 => check_drive_ready(fw),
        0x11 => recalibrate(fw),
        0x15 => get_disk_type(fw),
        0x16 => floppy_detect_change(fw),
        0x17 => set_disk_type_for_format(fw),
        0x18 => set_media_type_for_format(fw),
        0x41 => edd_install_check(fw),
        0x42 => edd_read_sectors(fw),
        0x43 => edd_write_sectors(fw),
        0x44 => edd_verify_sectors(fw),
        0x45 => edd_lock_unlock(fw),
        0x46 => edd_eject(fw),
        0x47 => edd_seek(fw),
        0x48 => edd_get_drive_parameters(fw),
        0x49 => edd_media_change(fw),
        0x4A | 0x4C => eltorito_initiate_emulation(fw),
        0x4B => eltorito_emulation_control(fw),
        0x4D => eltorito_return_boot_catalog(fw),
        _ => {
            // Includes AX=5001h (EDD 3.0 send packet
            // command / ATAPI passthrough), which is
            // not implemented yet.
            fw.int13_error_nodrive(INVALID_FUNCTION);
        }
    }
}

// ----------------------------------------------------------------------
// Classic CHS functions
// ----------------------------------------------------------------------

/// AH=00h: reset the disk system.
fn reset_disk_system<M: Machine>(fw: &mut Firmware<M>) {
    // In this emulation there is no controller state to
    // reset; acknowledge and clear the media-change flag.
    if let Some(number) = fw.drive_in_dl() {
        if let Some(d) = fw.drives.get(number) {
            let _ = d.backend.media_changed();
        }
        fw.int13_ok(0);
    }
}

/// AH=01h: get status of the last operation.
fn get_last_status<M: Machine>(fw: &mut Firmware<M>) {
    let dl = fw.dl();
    let st = fw
        .drives
        .get(dl)
        .map(|d| d.last_status)
        .unwrap_or(SUCCESS);
    fw.set_cf(false);
    fw.set_ah(st);
}

/// CHS register block -> (cylinder, head, sector).
fn chs_from_regs<M: Machine>(fw: &Firmware<M>) -> (u32, u32, u32) {
    let ch = fw.ch() as u32;
    let cl = fw.cl() as u32;
    let dh = fw.dh() as u32;
    let cylinder = ch | ((cl & 0xC0) << 2);
    let sector = cl & 0x3F;
    (cylinder, dh, sector)
}

/// AH=02h: read sector(s) into memory at ES:BX.
fn read_sectors_chs<M: Machine>(fw: &mut Firmware<M>) {
    transfer_chs(fw, false)
}

/// AH=03h: write sector(s) from memory at ES:BX.
fn write_sectors_chs<M: Machine>(fw: &mut Firmware<M>) {
    transfer_chs(fw, true)
}

/// AH=04h: verify sector(s).
fn verify_sectors_chs<M: Machine>(fw: &mut Firmware<M>) {
    let number = match fw.drive_in_dl() {
        Some(n) => n,
        None => return,
    };
    let (cylinder, head, sector) = chs_from_regs(fw);
    let geo = {
        let d = fw.drives.get(number).unwrap();
        d.info().geometry
    };
    if geo.chs_to_lba(cylinder, head, sector).is_none() {
        fw.int13_error(number, status::SECTOR_NOT_FOUND, 0);
        return;
    }
    fw.int13_ok(0);
}

/// Shared implementation of AH=02h/03h.
fn transfer_chs<M: Machine>(fw: &mut Firmware<M>, write: bool) {
    let number = match fw.drive_in_dl() {
        Some(n) => n,
        None => return,
    };
    // AL = sector count; the historical encoding of 0
    // means 256 sectors.
    let mut count = fw.al() as u32;
    if count == 0 {
        count = 256;
    }
    let (cylinder, head, sector) = chs_from_regs(fw);
    let buf_seg = fw.es();
    let buf_off = fw.bx() as u32;

    let (geo, sector_size, total_sectors) = {
        let d = fw.drives.get(number).unwrap();
        let info = d.info();
        (info.geometry, info.sector_size, info.total_sectors)
    };

    let lba = match geo.chs_to_lba(cylinder, head, sector) {
        Some(lba) => lba,
        None => {
            fw.int13_error(number, status::SECTOR_NOT_FOUND, 0);
            return;
        }
    };

    // Capacity check in the device's native sector
    // units.
    if lba + count as u64 > total_sectors {
        fw.int13_error(number, status::SECTOR_NOT_FOUND, 0);
        return;
    }

    let ss = sector_size as usize;
    let chunk_cap = 127usize.min(16 * 1024 / ss.max(1));
    let mut scratch = vec![0u8; chunk_cap * ss];
    let mut transferred: u32 = 0;

    while transferred < count {
        let this = ((count - transferred) as usize).min(scratch.len() / ss);
        let lba_i = lba + transferred as u64;
        let dst = crate::machine::phys(buf_seg, buf_off + transferred * ss as u32);

        let result: Result<(), Int13Status> = {
            let d = fw.drives.get(number).unwrap();
            let slice = &mut scratch[..this * ss];
            if write {
                // fill the scratch from guest memory
                for i in 0..slice.len() {
                    slice[i] = fw.machine.read_u8(dst + i as u32);
                }
                d.backend.write_sectors(lba_i, slice)
            } else {
                d.backend.read_sectors(lba_i, slice).map(|()| {
                    for i in 0..slice.len() {
                        fw.machine.write_u8(dst + i as u32, slice[i]);
                    }
                })
            }
        };

        match result {
            Ok(()) => {
                transferred += this as u32;
            }
            Err(st) => {
                fw.int13_error(number, st, transferred as u8);
                return;
            }
        }
    }

    fw.int13_ok(transferred as u8);
}

/// AH=05h: format track. The emulator works with
/// pre-formatted images, so this is a no-op that
/// reports success.
fn format_track<M: Machine>(fw: &mut Firmware<M>) {
    if fw.drive_in_dl().is_some() {
        fw.int13_ok(0);
    }
}

/// AH=08h: get drive parameters.
fn get_drive_parameters<M: Machine>(fw: &mut Firmware<M>) {
    let number = match fw.drive_in_dl() {
        Some(n) => n,
        None => return,
    };
    let (kind, geo, sector_size) = {
        let d = fw.drives.get(number).unwrap();
        let info = d.info();
        (info.kind, info.geometry, info.sector_size)
    };

    // CX = cylinders - 1 (bits 0-7 in CH, bits 8-9 in
    // CL 6-7); DH = heads - 1; DL = drive count.
    let cylinders = geo.cylinders.saturating_sub(1).min(0x3FF);
    let heads = geo.heads.saturating_sub(1);
    let spt = geo.sectors_per_track.min(0x3F);
    fw.set_ch((cylinders & 0xFF) as u8);
    fw.set_cl((((cylinders >> 8) & 0x03) << 6) as u8 | (spt as u8 & 0x3F));
    fw.set_dh(heads as u8);

    match kind {
        DriveKind::Floppy => {
            // BL = media type; DL = total floppy drives.
            let total = fw.drives.count(DriveKind::Floppy) as u8;
            fw.set_dl(total);
            let media = media_type_code(geo, sector_size);
            fw.set_bl(media);
        }
        DriveKind::HardDisk => {
            // DL = total hard disks.
            let total = fw.drives.count(DriveKind::HardDisk) as u8;
            fw.set_dl(total);
        }
        DriveKind::CdRom => {
            // CD-ROM drives are not part of the legacy
            // CHS model; report the drive count in DL.
            let total = fw.drives.count(DriveKind::CdRom) as u8;
            fw.set_dl(total);
        }
    }

    // ES:DI -> 10-byte disk parameter table.
    let addr = crate::machine::phys(fw.es(), fw.di() as u32);
    fw.machine.write_u8(addr, spt as u8); // max sector number
    fw.machine.write_u8(addr + 1, 0);
    fw.machine.write_u16(addr + 2, cylinders as u16);
    fw.machine.write_u16(addr + 4, heads as u16);
    fw.machine.write_u16(addr + 6, 0);
    fw.machine.write_u16(addr + 8, 0);

    fw.int13_ok(0);
}

/// Legacy floppy media type code (BL of AH=08h).
fn media_type_code(geo: Geometry, sector_size: u16) -> u8 {
    let total = geo.total_sectors();
    if sector_size != SECTOR_SIZE {
        return 0x00;
    }
    match total {
        720 => 0x03,  // 360K 3.5"
        1232 => 0x02, // 1.2M 5.25"
        1440 => 0x04, // 1.44M 3.5"
        2880 => 0x05, // 2.88M 3.5"
        _ => 0x00,
    }
}

/// AH=0Ch: seek cylinder (no-op).
fn seek_cylinder<M: Machine>(fw: &mut Firmware<M>) {
    if fw.drive_in_dl().is_some() {
        fw.int13_ok(0);
    }
}

/// AH=0Dh: reset hard disks (no-op).
fn reset_hard_disks<M: Machine>(fw: &mut Firmware<M>) {
    fw.int13_ok(0);
}

/// AH=10h: check drive ready.
fn check_drive_ready<M: Machine>(fw: &mut Firmware<M>) {
    if fw.drive_in_dl().is_some() {
        fw.int13_ok(0);
    }
}

/// AH=11h: recalibrate drive (no-op).
fn recalibrate<M: Machine>(fw: &mut Firmware<M>) {
    if fw.drive_in_dl().is_some() {
        fw.int13_ok(0);
    }
}

/// AH=15h: get disk type.
fn get_disk_type<M: Machine>(fw: &mut Firmware<M>) {
    let number = match fw.drive_in_dl() {
        Some(n) => n,
        None => return,
    };
    let (kind, total_sectors) = {
        let d = fw.drives.get(number).unwrap();
        let info = d.info();
        (info.kind, info.total_sectors)
    };
    match kind {
        DriveKind::Floppy => {
            // AH=01: floppy, no change line support.
            fw.set_cf(false);
            fw.set_ah(0x01);
        }
        DriveKind::HardDisk => {
            // AH=02: hard disk with change line; DX:CX =
            // sector count.
            fw.set_cf(false);
            fw.set_ah(0x02);
            fw.set_dx((total_sectors >> 16) as u16);
            fw.set_cx(total_sectors as u16);
        }
        DriveKind::CdRom => {
            // AH=03: removable media with change line.
            fw.set_cf(false);
            fw.set_ah(0x03);
        }
    }
}

/// AH=16h: floppy media change detection.
fn floppy_detect_change<M: Machine>(fw: &mut Firmware<M>) {
    let number = match fw.drive_in_dl() {
        Some(n) => n,
        None => return,
    };
    let changed = fw
        .drives
        .get(number)
        .map(|d| d.backend.media_changed())
        .unwrap_or(false);
    fw.set_cf(false);
    fw.set_ah(if changed { 0x06 } else { 0x00 });
}

/// AH=17h: set disk type for format (accepted, no-op).
fn set_disk_type_for_format<M: Machine>(fw: &mut Firmware<M>) {
    if fw.drive_in_dl().is_some() {
        fw.int13_ok(0);
    }
}

/// AH=18h: set media type for format (accepted, no-op).
fn set_media_type_for_format<M: Machine>(fw: &mut Firmware<M>) {
    if fw.drive_in_dl().is_some() {
        fw.int13_ok(0);
    }
}

// ----------------------------------------------------------------------
// INT 13h Extensions (EDD)
// ----------------------------------------------------------------------

/// EDD install check capability flags (CX).
pub const EDD_CAP_EXTENDED_ACCESS: u16 = 1 << 0; // AH=42h-44h,47h,48h
pub const EDD_CAP_CONTROL: u16 = 1 << 1; // AH=45h-47h,48h
pub const EDD_CAP_64BIT: u16 = 1 << 2; // 64-bit LBA addressing

/// EDD version reported by AH=41h, as major in AH and minor in DH.
///
/// EDD 3.0 is the right number to claim: it is the version that added the
/// 64-bit LBA field to the disk address packet, which is exactly what
/// `EDD_CAP_64BIT` below promises.
pub const EDD_VERSION_MAJOR: u8 = 0x03;
pub const EDD_VERSION_MINOR: u8 = 0x00;

/// AH=41h: EDD installation check. BX must be 0x55AAh.
fn edd_install_check<M: Machine>(fw: &mut Firmware<M>) {
    if fw.bx() != 0x55AA {
        // BX != 55AAh: not a valid installation check.
        fw.trace(
            crate::debug::tag::DISK,
            format!("EDD check rejected: BX={:04X}", fw.bx()),
        );
        fw.int13_error_nodrive(INVALID_FUNCTION);
        return;
    }
    if fw.drive_in_dl().is_none() {
        fw.trace(crate::debug::tag::DISK, "EDD check: no such drive");
        return;
    }
    // Report EDD 3.0 with extended access, control functions and 64-bit
    // addressing.
    //
    // AH is the *major version of the Extensions spec*, not the classic
    // INT 13h success code. Writing SUCCESS (0) there tells a caller the
    // BIOS implements version 0, which is not a version that exists.
    // DL is the number of drives, not the drive that was asked about, so it
    // has to be counted rather than echoed back.
    let drive_count = fw.drives.drives.len().min(0xFF) as u8;
    fw.set_cf(false);
    fw.set_ah(EDD_VERSION_MAJOR);
    fw.set_bx(0xAA55);
    fw.set_cx(EDD_CAP_EXTENDED_ACCESS | EDD_CAP_CONTROL | EDD_CAP_64BIT);
    fw.set_dh(EDD_VERSION_MINOR);
    fw.set_dl(drive_count);
    fw.trace(
        crate::debug::tag::DISK,
        format!(
            "EDD ok: AH={:02X}h BX={:04X}h CX={:04X}h DX={:04X}h",
            fw.ah(),
            fw.bx(),
            fw.cx(),
            fw.dx()
        ),
    );
}

/// Disk Address Packet (DAP) at DS:SI.
struct DiskAddressPacket {
    size: u8,
    count: u16,
    buffer_seg: u16,
    buffer_off: u16,
    lba: u64,
}

/// Read a DAP from guest memory. The 64-bit LBA field
/// is at offset 8 (EDD 3.0); EDD 1.0 callers only
/// supply the low dword.
fn read_dap<M: Machine>(fw: &mut Firmware<M>) -> DiskAddressPacket {
    let addr = crate::machine::phys(fw.ds(), fw.si() as u32);
    let size = fw.machine.read_u8(addr);
    let count = fw.machine.read_u16(addr + 2);
    let buffer_off = fw.machine.read_u16(addr + 4);
    let buffer_seg = fw.machine.read_u16(addr + 6);
    let lba_lo = fw.machine.read_u32(addr + 8) as u64;
    let lba_hi = fw.machine.read_u32(addr + 12) as u64;
    DiskAddressPacket {
        size,
        count,
        buffer_seg,
        buffer_off,
        lba: lba_lo | (lba_hi << 32),
    }
}

/// Store the number of sectors actually transferred
/// back into the DAP's count field.
fn write_dap_count<M: Machine>(fw: &mut Firmware<M>, count: u16) {
    let addr = crate::machine::phys(fw.ds(), fw.si() as u32);
    fw.machine.write_u16(addr + 2, count);
}

/// AH=42h: extended read.
fn edd_read_sectors<M: Machine>(fw: &mut Firmware<M>) {
    edd_transfer(fw, false)
}

/// AH=43h: extended write.
fn edd_write_sectors<M: Machine>(fw: &mut Firmware<M>) {
    edd_transfer(fw, true)
}

/// Shared implementation of AH=42h/43h.
fn edd_transfer<M: Machine>(fw: &mut Firmware<M>, write: bool) {
    let number = match fw.drive_in_dl() {
        Some(n) => n,
        None => return,
    };
    let dap = read_dap(fw);
    if dap.size < 16 {
        fw.int13_error(number, status::BAD_PARAMETER_TABLE, 0);
        return;
    }
    if dap.count == 0 {
        fw.int13_error(number, status::INVALID_SECTOR_COUNT, 0);
        return;
    }

    let (sector_size, total_sectors) = {
        let d = fw.drives.get(number).unwrap();
        let info = d.info();
        (info.sector_size, info.total_sectors)
    };

    // LBA bounds check against the device's native
    // sector capacity.
    if dap.lba + dap.count as u64 > total_sectors {
        write_dap_count(fw, 0);
        fw.int13_error(number, status::SECTOR_NOT_FOUND, 0);
        return;
    }

    let ss = sector_size as usize;
    let chunk_cap = 127usize.min(16 * 1024 / ss.max(1));
    let mut scratch = vec![0u8; chunk_cap * ss];
    let mut transferred: u32 = 0;

    while transferred < dap.count as u32 {
        let this = ((dap.count as u32 - transferred) as usize).min(scratch.len() / ss);
        let lba_i = dap.lba + transferred as u64;
        let dst = crate::machine::phys(
            dap.buffer_seg,
            dap.buffer_off as u32 + transferred * ss as u32,
        );

        let result: Result<(), Int13Status> = {
            let d = fw.drives.get(number).unwrap();
            let slice = &mut scratch[..this * ss];
            if write {
                for i in 0..slice.len() {
                    slice[i] = fw.machine.read_u8(dst + i as u32);
                }
                d.backend.write_sectors(lba_i, slice)
            } else {
                let r = d.backend.read_sectors(lba_i, slice);
                if r.is_ok() {
                    for i in 0..slice.len() {
                        fw.machine.write_u8(dst + i as u32, slice[i]);
                    }
                }
                r
            }
        };

        match result {
            Ok(()) => transferred += this as u32,
            Err(st) => {
                write_dap_count(fw, transferred as u16);
                fw.int13_error(number, st, transferred as u8);
                return;
            }
        }
    }

    write_dap_count(fw, transferred as u16);
    fw.int13_ok(transferred as u8);
}

/// AH=44h: verify sectors.
fn edd_verify_sectors<M: Machine>(fw: &mut Firmware<M>) {
    let number = match fw.drive_in_dl() {
        Some(n) => n,
        None => return,
    };
    let dap = read_dap(fw);
    if dap.size < 16 {
        fw.int13_error(number, status::BAD_PARAMETER_TABLE, 0);
        return;
    }
    let total_sectors = {
        let d = fw.drives.get(number).unwrap();
        d.info().total_sectors
    };
    if dap.lba + dap.count as u64 > total_sectors {
        write_dap_count(fw, 0);
        fw.int13_error(number, status::SECTOR_NOT_FOUND, 0);
        return;
    }
    write_dap_count(fw, dap.count);
    fw.int13_ok(dap.count as u8);
}

/// AH=45h: lock/unlock drive. AL=0 lock, AL=1 unlock.
fn edd_lock_unlock<M: Machine>(fw: &mut Firmware<M>) {
    let number = match fw.drive_in_dl() {
        Some(n) => n,
        None => return,
    };
    let lock = fw.al() == 0;
    let result = fw
        .drives
        .get(number)
        .map(|d| {
            let r = d.backend.lock(lock);
            d.locked = lock;
            r
        })
        .unwrap_or(Ok(()));
    match result {
        Ok(()) => fw.int13_ok(0),
        Err(st) => fw.int13_error(number, st, 0),
    }
}

/// AH=46h: eject media.
fn edd_eject<M: Machine>(fw: &mut Firmware<M>) {
    let number = match fw.drive_in_dl() {
        Some(n) => n,
        None => return,
    };
    let result = fw
        .drives
        .get(number)
        .map(|d| d.backend.eject())
        .unwrap_or(Err(status::INVALID_COMMAND));
    match result {
        Ok(()) => fw.int13_ok(0),
        Err(st) => fw.int13_error(number, st, 0),
    }
}

/// AH=47h: extended seek (no-op, success).
fn edd_seek<M: Machine>(fw: &mut Firmware<M>) {
    if fw.drive_in_dl().is_some() {
        fw.int13_ok(0);
    }
}

/// EDD device parameters structure size (EDD 3.0).
pub const EDD_PARAMS_SIZE: u16 = 30;

/// Information flags for the EDD parameters structure.
pub const EDD_INFO_CHS_VALID: u16 = 1 << 0;
pub const EDD_INFO_LBA_VALID: u16 = 1 << 1;
pub const EDD_INFO_LBA_ASSIST_VALID: u16 = 1 << 2;

/// AH=48h: get drive parameters (EDD device parameters
/// structure at ES:DI).
fn edd_get_drive_parameters<M: Machine>(fw: &mut Firmware<M>) {
    let number = match fw.drive_in_dl() {
        Some(n) => n,
        None => return,
    };
    let (geo, sector_size, total_sectors) = {
        let d = fw.drives.get(number).unwrap();
        let info = d.info();
        (info.geometry, info.sector_size, info.total_sectors)
    };

    let addr = crate::machine::phys(fw.es(), fw.di() as u32);
    // The caller stores the buffer size in the first
    // word; honor it.
    let buf_size = fw.machine.read_u16(addr).max(EDD_PARAMS_SIZE);

    let info_flags =
        EDD_INFO_CHS_VALID | EDD_INFO_LBA_VALID | EDD_INFO_LBA_ASSIST_VALID;

    let mut params = [0u8; EDD_PARAMS_SIZE as usize];
    params[0..2].copy_from_slice(&EDD_PARAMS_SIZE.to_le_bytes());
    params[2..4].copy_from_slice(&info_flags.to_le_bytes());
    params[4..8].copy_from_slice(&geo.cylinders.to_le_bytes());
    params[8..12].copy_from_slice(&geo.heads.to_le_bytes());
    params[12..16].copy_from_slice(&geo.sectors_per_track.to_le_bytes());
    // Total sectors: 64-bit at offset 0x10 (EDD 3.0;
    // EDD 1.0 callers read the low dword).
    params[16..24].copy_from_slice(&total_sectors.to_le_bytes());
    params[24..26].copy_from_slice(&sector_size.to_le_bytes());
    // Host protected area size (offset 0x1A): zero.
    params[26..30].copy_from_slice(&0u32.to_le_bytes());

    let n = buf_size.min(EDD_PARAMS_SIZE) as usize;
    for i in 0..n {
        fw.machine.write_u8(addr + i as u32, params[i]);
    }

    fw.int13_ok(0);
}

/// AH=49h: extended media change.
fn edd_media_change<M: Machine>(fw: &mut Firmware<M>) {
    let number = match fw.drive_in_dl() {
        Some(n) => n,
        None => return,
    };
    let changed = fw
        .drives
        .get(number)
        .map(|d| d.backend.media_changed())
        .unwrap_or(false);
    fw.set_cf(false);
    fw.set_ah(if changed { 0x06 } else { 0x00 });
}

// ----------------------------------------------------------------------
// El Torito disk emulation control
// ----------------------------------------------------------------------

/// AH=4Ah / AH=4Ch: initiate disk emulation (and boot).
/// The firmware configures El Torito emulation during
/// POST, so this acknowledges the request.
fn eltorito_initiate_emulation<M: Machine>(fw: &mut Firmware<M>) {
    fw.int13_ok(0);
}

/// AX=4B00h: terminate disk emulation; AX=4B01h: get
/// emulation status (AL = 0 while emulation is active).
fn eltorito_emulation_control<M: Machine>(fw: &mut Firmware<M>) {
    match fw.al() {
        0x00 => {
            // Terminate: accepted as a no-op; the
            // emulator keeps the image attached.
            fw.int13_ok(0);
        }
        0x01 => {
            // Get status: AL=0 means emulation is
            // active.
            fw.set_cf(false);
            fw.set_ah(SUCCESS);
            fw.set_al(0x00);
        }
        _ => fw.int13_error_nodrive(INVALID_FUNCTION),
    }
}

/// AX=4D00h: return the boot catalog at ES:DI.
fn eltorito_return_boot_catalog<M: Machine>(fw: &mut Firmware<M>) {
    // The firmware caches the catalog of the attached
    // El Torito image; copy it out when available.
    let catalog = match fw.cached_catalog.as_ref() {
        Some(c) => c,
        None => {
            fw.int13_error_nodrive(status::DRIVE_NOT_READY);
            return;
        }
    };
    let addr = crate::machine::phys(fw.es(), fw.di() as u32);
    for (i, b) in catalog.iter().enumerate() {
        fw.machine.write_u8(addr + i as u32, *b);
    }
    fw.int13_ok(0);
}
