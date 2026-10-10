//! ISO 9660 / El Torito bootable CD-ROM parsing.
//!
//! Implemented from the Phoenix/IBM "El Torito Bootable
//! CD-ROM Format Specification" and ECMA-119 (ISO 9660).
//! No firmware source code was consulted.

use crate::backend::BlockBackend;

/// Size of an ISO 9660 logical sector (and El Torito
/// catalog sector).
pub const ISO_SECTOR: usize = 2048;

/// El Torito media types (boot catalog default entry, byte 1).
pub const MEDIA_NO_EMULATION: u8 = 0;
pub const MEDIA_1200K_FLOPPY: u8 = 1;
pub const MEDIA_1440K_FLOPPY: u8 = 2;
pub const MEDIA_2880K_FLOPPY: u8 = 3;
pub const MEDIA_HARD_DISK: u8 = 4;
pub const MEDIA_NON_BOOTABLE: u8 = 5;

/// El Torito platform IDs (validation entry, byte 1).
pub const PLATFORM_X86: u8 = 0;

/// Errors returned while locating a bootable image.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ElToritoError {
    /// The image is not an ISO 9660 volume.
    NotIso,
    /// No boot record volume descriptor present.
    NoBootRecord,
    /// Boot record does not identify El Torito.
    NotElTorito,
    /// Boot catalog is missing or fails validation.
    InvalidCatalog,
    /// The catalog has no bootable default/section entry.
    NotBootable,
    /// I/O failure reading the image.
    IoError,
}

/// Parsed boot catalog validation entry (32 bytes).
#[derive(Copy, Clone, Debug)]
pub struct ValidationEntry {
    pub header_id: u8,
    pub platform_id: u8,
    pub id_string: [u8; 24],
}

/// Parsed boot catalog default (initial) entry (32 bytes).
#[derive(Copy, Clone, Debug)]
pub struct DefaultEntry {
    pub bootable: bool,
    pub media_type: u8,
    pub load_segment: u16,
    pub system_type: u8,
    /// Number of *virtual* 512-byte sectors to load.
    pub sector_count: u16,
    /// LBA (in 2048-byte CD sectors) of the boot image.
    pub load_rba: u32,
}

/// Parsed section entry (32 bytes) for multi-boot catalogs.
#[derive(Copy, Clone, Debug)]
pub struct SectionEntry {
    pub bootable: bool,
    pub media_type: u8,
    pub load_segment: u16,
    pub system_type: u8,
    pub sector_count: u16,
    pub load_rba: u32,
}

/// Complete El Torito boot information.
#[derive(Clone, Debug)]
pub struct BootInfo {
    /// LBA of the boot catalog sector.
    pub catalog_lba: u32,
    pub validation: ValidationEntry,
    pub default_entry: DefaultEntry,
    /// Bootable section entries (x86 platform).
    pub sections: Vec<SectionEntry>,
}

/// Read one 2048-byte sector from the image.
fn read_iso_sector(
    image: &mut dyn BlockBackend,
    lba: u32,
) -> Result<[u8; ISO_SECTOR], ElToritoError> {
    let mut buf = [0u8; ISO_SECTOR];
    image
        .read_sectors(lba as u64, &mut buf)
        .map_err(|_| ElToritoError::IoError)?;
    Ok(buf)
}

/// Check that a volume descriptor has the ISO 9660 signature.
///
/// "CD001" is five bytes at offsets 1..6. Slicing 1..7 compared six bytes
/// against five, which is never equal, so every volume descriptor was
/// rejected and `parse_boot_info` could only ever return `NotIso` -- El Torito
/// boot was dead code that looked alive.
fn is_volume_descriptor(sector: &[u8; ISO_SECTOR]) -> bool {
    &sector[1..6] == b"CD001"
}

/// Parse the El Torito boot information from an ISO image.
///
/// Steps (per the El Torito specification):
/// 1. Verify the Primary Volume Descriptor at LBA 16.
/// 2. Scan volume descriptors from LBA 17 for a Boot Record
///    (type 0x00) whose boot system identifier is
///    "EL TORITO SPECIFICATION".
/// 3. Read the Boot Catalog at the LBA stored at offset 0x47
///    of the boot record.
/// 4. Validate the catalog's Validation Entry (header ID 0x01,
///    checksum of all 16-bit words zero, key bytes 0x55 0xAA).
/// 5. Parse the Default Entry and any section entries.
pub fn parse_boot_info(
    image: &mut dyn BlockBackend,
) -> Result<BootInfo, ElToritoError> {
    // 1. Primary Volume Descriptor at LBA 16, type 0x01.
    let pvd = read_iso_sector(image, 16)?;
    if pvd[0] != 0x01 || !is_volume_descriptor(&pvd) {
        return Err(ElToritoError::NotIso);
    }

    // 2. Scan for the boot record volume descriptor.
    let mut boot_catalog_lba: Option<u32> = None;
    for lba in 17u32.. {
        let vd = read_iso_sector(image, lba)?;
        if vd[0] == 0xFF {
            break; // terminator
        }
        if !is_volume_descriptor(&vd) {
            continue;
        }
        if vd[0] != 0x00 {
            continue; // not a boot record
        }
        let boot_system_id = &vd[7..39];
        // "EL TORITO SPECIFICATION" is 23 bytes, so the remainder of the
        // 32-byte identifier field starts at 23 -- not at 21, which
        // overlapped the last two characters of the signature and rejected
        // every boot record that was otherwise perfectly good.
        let is_el_torito = boot_system_id
            .iter()
            .zip(b"EL TORITO SPECIFICATION".iter())
            .all(|(a, b)| *a == *b)
            && boot_system_id[23..].iter().all(|&c| c == 0 || c == b' ');
        if !is_el_torito {
            continue;
        }
        // Boot catalog LBA is at offset 0x47 (dword, LE).
        boot_catalog_lba = Some(u32::from_le_bytes([
            vd[0x47],
            vd[0x48],
            vd[0x49],
            vd[0x4A],
        ]));
        break;
    }

    let catalog_lba = match boot_catalog_lba {
        Some(lba) => lba,
        None => return Err(ElToritoError::NotElTorito),
    };

    // 3. Read and validate the boot catalog.
    let catalog = read_iso_sector(image, catalog_lba)?;
    let validation = parse_validation_entry(&catalog)?;

    // 4. Default entry at offset 32.
    let default_entry = parse_default_entry(&catalog, 32)?;

    // 5. Walk the remaining 32-byte entries until the end of
    //    the catalog sector. Section headers (0x90) and
    //    section entries (0x91) both occupy 32 bytes.
    let mut sections = Vec::new();
    let mut offset = 64usize;
    while offset + 32 <= ISO_SECTOR {
        let header_id = catalog[offset];
        if header_id == 0x00 {
            break; // end of catalog
        }
        if header_id == 0x91 && catalog[offset + 1] == 0x88 {
            let entry = parse_section_entry(&catalog, offset)?;
            sections.push(entry);
        }
        offset += 32;
    }

    Ok(BootInfo {
        catalog_lba,
        validation,
        default_entry,
        sections,
    })
}

/// Parse and checksum-validate a boot catalog validation
/// entry at offset 0 of a catalog sector.
fn parse_validation_entry(catalog: &[u8; ISO_SECTOR]) -> Result<ValidationEntry, ElToritoError> {
    if catalog[0] != 0x01 {
        return Err(ElToritoError::InvalidCatalog);
    }
    // Key bytes 0x55 0xAA at offset 30-31.
    if catalog[30] != 0x55 || catalog[31] != 0xAA {
        return Err(ElToritoError::InvalidCatalog);
    }
    // Checksum: the 16-bit sum of all 16 words must be zero.
    let mut sum: u32 = 0;
    for word in catalog[..32].chunks_exact(2) {
        sum += u16::from_le_bytes([word[0], word[1]]) as u32;
    }
    if (sum & 0xFFFF) != 0 {
        return Err(ElToritoError::InvalidCatalog);
    }
    let mut id_string = [0u8; 24];
    id_string.copy_from_slice(&catalog[4..28]);
    Ok(ValidationEntry {
        header_id: catalog[0],
        platform_id: catalog[1],
        id_string,
    })
}

/// Parse a default (initial) entry at the given catalog offset.
fn parse_default_entry(
    catalog: &[u8; ISO_SECTOR],
    offset: usize,
) -> Result<DefaultEntry, ElToritoError> {
    if offset + 32 > ISO_SECTOR {
        return Err(ElToritoError::InvalidCatalog);
    }
    Ok(DefaultEntry {
        bootable: catalog[offset] == 0x88,
        media_type: catalog[offset + 1],
        load_segment: u16::from_le_bytes([catalog[offset + 2], catalog[offset + 3]]),
        system_type: catalog[offset + 4],
        sector_count: u16::from_le_bytes([catalog[offset + 6], catalog[offset + 7]]),
        load_rba: u32::from_le_bytes([
            catalog[offset + 8],
            catalog[offset + 9],
            catalog[offset + 10],
            catalog[offset + 11],
        ]),
    })
}

/// Parse a section entry at the given catalog offset.
fn parse_section_entry(
    catalog: &[u8; ISO_SECTOR],
    offset: usize,
) -> Result<SectionEntry, ElToritoError> {
    if offset + 32 > ISO_SECTOR {
        return Err(ElToritoError::InvalidCatalog);
    }
    Ok(SectionEntry {
        bootable: catalog[offset + 1] == 0x88,
        media_type: catalog[offset + 2],
        load_segment: u16::from_le_bytes([catalog[offset + 3], catalog[offset + 4]]),
        system_type: catalog[offset + 5],
        sector_count: u16::from_le_bytes([catalog[offset + 7], catalog[offset + 8]]),
        load_rba: u32::from_le_bytes([
            catalog[offset + 9],
            catalog[offset + 10],
            catalog[offset + 11],
            catalog[offset + 12],
        ]),
    })
}

/// Read a full boot catalog sector (2048 bytes) from the image.
pub fn read_catalog_sector(
    image: &mut dyn BlockBackend,
    lba: u32,
) -> Result<[u8; ISO_SECTOR], ElToritoError> {
    read_iso_sector(image, lba)
}

/// Number of 512-byte virtual sectors in a standard El Torito
/// emulated media image.
pub fn emulated_media_sectors(media_type: u8) -> Option<u64> {
    match media_type {
        MEDIA_1200K_FLOPPY => Some(1232),
        MEDIA_1440K_FLOPPY => Some(2880),
        MEDIA_2880K_FLOPPY => Some(5760),
        MEDIA_HARD_DISK => None, // sized by the boot image itself
        _ => None,
    }
}

/// Geometry for an El Torito emulated hard disk image.
pub fn emulation_hd_geometry(image_sectors: u64) -> crate::backend::Geometry {
    crate::backend::hd_geometry(image_sectors)
}
