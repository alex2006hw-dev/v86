//! Abstract block-device backends and disk geometry.

pub const SECTOR_SIZE: u16 = 512;
pub const CD_SECTOR_SIZE: u16 = 2048;

/// INT 13h error/status codes as returned in AH.
pub type Int13Status = u8;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DriveKind {
    Floppy,
    HardDisk,
    CdRom,
}

/// CHS geometry used for INT 13h translation.
#[derive(Copy, Clone, Debug)]
pub struct Geometry {
    pub cylinders: u32,
    pub heads: u32,
    pub sectors_per_track: u32,
}

impl Geometry {
    /// CHS → LBA. Sector numbers are 1-based in the register
    /// encoding; `sector` here is already the 1-based value.
    /// Returns None when the CHS tuple does not address a real
    /// sector (sector == 0, head >= heads, cylinder >= cylinders,
    /// or out of capacity).
    pub fn chs_to_lba(&self, cylinder: u32, head: u32, sector: u32) -> Option<u64> {
        if sector == 0 || sector > self.sectors_per_track || head >= self.heads || cylinder >= self.cylinders {
            return None;
        }
        let lba = (cylinder as u64) * (self.heads as u64) * (self.sectors_per_track as u64)
            + (head as u64) * (self.sectors_per_track as u64)
            + (sector as u64 - 1);
        Some(lba)
    }

    /// LBA → CHS. Returns (cylinder, head, 1-based sector).
    pub fn lba_to_chs(&self, lba: u64) -> (u32, u32, u32) {
        let spt = self.sectors_per_track as u64;
        let heads = self.heads as u64;
        let sector = (lba % spt) as u32 + 1;
        let head = ((lba / spt) % heads) as u32;
        let cylinder = (lba / (spt * heads)) as u32;
        (cylinder, head, sector)
    }

    pub fn total_sectors(&self) -> u64 {
        (self.cylinders as u64) * (self.heads as u64) * (self.sectors_per_track as u64)
    }
}

/// Standard 1.44M 3.5" floppy geometry.
pub const FLOPPY_1440K: Geometry = Geometry {
    cylinders: 80,
    heads: 2,
    sectors_per_track: 18,
};

/// Standard 1.2M 5.25" floppy geometry.
pub const FLOPPY_1200K: Geometry = Geometry {
    cylinders: 80,
    heads: 2,
    sectors_per_track: 15,
};

/// Standard 2.88M 3.5" floppy geometry.
pub const FLOPPY_2880K: Geometry = Geometry {
    cylinders: 80,
    heads: 2,
    sectors_per_track: 36,
};

/// Geometry for an emulated hard disk image: 16 heads, 63 sectors
/// per track, cylinders derived from the image size (clamped to
/// 1024 cylinders for the CHS register encoding, with LBA-assist
/// via EDD for the full capacity).
pub fn hd_geometry(total_sectors: u64) -> Geometry {
    const HEADS: u64 = 16;
    const SPT: u64 = 63;
    let cylinders = (total_sectors + HEADS * SPT - 1) / (HEADS * SPT);
    let cylinders = cylinders.min(1024).max(1) as u32;
    Geometry {
        cylinders,
        heads: HEADS as u32,
        sectors_per_track: SPT as u32,
    }
}

/// Geometry for a CD-ROM drive (2048-byte sectors): the CHS model
/// is not meaningful for CDs; we expose one sector per track so
/// that CHS math degenerates to LBA.
pub const CD_ROM_GEOMETRY: Geometry = Geometry {
    cylinders: 1_000_000,
    heads: 1,
    sectors_per_track: 1,
};

/// Static device description.
#[derive(Copy, Clone, Debug)]
pub struct BlockInfo {
    pub kind: DriveKind,
    pub geometry: Geometry,
    pub sector_size: u16,
    /// Total number of addressable sectors of `sector_size` bytes.
    pub total_sectors: u64,
    pub removable: bool,
}

/// Abstract block device. All operations are expressed in LBA with
/// the device's native sector size.
pub trait BlockBackend {
    fn info(&self) -> BlockInfo;

    /// Read `buf.len()` bytes starting at byte offset
    /// `lba * sector_size`. `buf.len()` is always a multiple of
    /// the sector size.
    fn read_sectors(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), Int13Status>;

    fn write_sectors(&mut self, lba: u64, buf: &[u8]) -> Result<(), Int13Status>;

    fn flush(&mut self) -> Result<(), Int13Status> {
        Ok(())
    }

    /// True when the media was changed since the last call.
    fn media_changed(&mut self) -> bool {
        false
    }

    fn eject(&mut self) -> Result<(), Int13Status> {
        Err(super::status::INVALID_COMMAND)
    }

    fn lock(&mut self, _lock: bool) -> Result<(), Int13Status> {
        Ok(())
    }

    fn seek(&mut self, _lba: u64) -> Result<(), Int13Status> {
        Ok(())
    }
}

/// In-memory block backend (used by tests and small images).
pub struct RamDisk {
    pub info: BlockInfo,
    pub data: Vec<u8>,
    pub changed: bool,
}

impl RamDisk {
    pub fn new(info: BlockInfo, data: Vec<u8>) -> RamDisk {
        RamDisk {
            info,
            data,
            changed: false,
        }
    }
}

impl BlockBackend for RamDisk {
    fn info(&self) -> BlockInfo {
        self.info
    }

    fn read_sectors(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), Int13Status> {
        let sector_size = self.info.sector_size as u64;
        let start = lba
            .checked_mul(sector_size)
            .ok_or(super::status::SECTOR_NOT_FOUND)?;
        let end = start
            .checked_add(buf.len() as u64)
            .ok_or(super::status::SECTOR_NOT_FOUND)?;
        if end > self.data.len() as u64 {
            return Err(super::status::SECTOR_NOT_FOUND);
        }
        buf.copy_from_slice(&self.data[start as usize..end as usize]);
        Ok(())
    }

    fn write_sectors(&mut self, lba: u64, buf: &[u8]) -> Result<(), Int13Status> {
        let sector_size = self.info.sector_size as u64;
        let start = lba
            .checked_mul(sector_size)
            .ok_or(super::status::SECTOR_NOT_FOUND)?;
        let end = start
            .checked_add(buf.len() as u64)
            .ok_or(super::status::SECTOR_NOT_FOUND)?;
        if end > self.data.len() as u64 {
            return Err(super::status::SECTOR_NOT_FOUND);
        }
        self.data[start as usize..end as usize].copy_from_slice(buf);
        self.changed = true;
        Ok(())
    }

    fn media_changed(&mut self) -> bool {
        let c = self.changed;
        self.changed = false;
        c
    }
}

/// El Torito CD emulation backend: serves a portion of a CD image
/// as a virtual floppy or hard disk with 512-byte sectors.
///
/// This backend reads the boot image from the CD at creation time
/// and serves it as a virtual disk.
pub struct CdEmulationBackend {
    pub data: Vec<u8>,
    pub start_lba: u64,
    pub total_sectors: u64,
    pub geometry: Geometry,
    pub sector_size: u16,
}

impl CdEmulationBackend {
    pub fn new(
        cd_backend: &mut dyn BlockBackend,
        start_lba: u32,
        total_sectors: u64,
        geometry: Geometry,
        sector_size: u16,
    ) -> CdEmulationBackend {
        // Read the boot image from the CD.
        let start_byte = (start_lba as u64) * 2048;
        let total_bytes = total_sectors * sector_size as u64;
        let first_sector = (start_byte / 2048) as u64;
        let last_sector = ((start_byte + total_bytes - 1) / 2048) as u64;
        let sector_count = (last_sector - first_sector + 1) as usize;
        let mut scratch = vec![0u8; sector_count * 2048];
        let _ = cd_backend.read_sectors(first_sector, &mut scratch);
        let offset = (start_byte % 2048) as usize;
        let data = scratch[offset..offset + total_bytes as usize].to_vec();

        CdEmulationBackend {
            data,
            start_lba: start_lba as u64,
            total_sectors,
            geometry,
            sector_size,
        }
    }
}

impl BlockBackend for CdEmulationBackend {
    fn info(&self) -> BlockInfo {
        BlockInfo {
            kind: DriveKind::HardDisk,
            geometry: self.geometry,
            sector_size: self.sector_size,
            total_sectors: self.total_sectors,
            removable: false,
        }
    }

    fn read_sectors(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), Int13Status> {
        let sector_size = self.sector_size as u64;
        let start = lba * sector_size;
        let end = start + buf.len() as u64;
        if end > self.data.len() as u64 {
            return Err(super::status::SECTOR_NOT_FOUND);
        }
        buf.copy_from_slice(&self.data[start as usize..end as usize]);
        Ok(())
    }

    fn write_sectors(&mut self, _lba: u64, _buf: &[u8]) -> Result<(), Int13Status> {
        Err(super::status::WRITE_PROTECTED)
    }
}
