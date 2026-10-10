//! INT 13h status codes and other firmware result codes.

pub const SUCCESS: u8 = 0x00;
pub const INVALID_COMMAND: u8 = 0x01;
pub const ADDRESS_MARK_NOT_FOUND: u8 = 0x02;
pub const WRITE_PROTECTED: u8 = 0x03;
pub const SECTOR_NOT_FOUND: u8 = 0x04;
pub const RESET_FAILED: u8 = 0x05;
pub const MEDIA_CHANGED: u8 = 0x06;
pub const BAD_PARAMETER_TABLE: u8 = 0x07;
pub const DMA_OVERRUN: u8 = 0x08;
pub const DMA_64K_BOUNDARY: u8 = 0x09;
pub const BAD_SECTOR_FLAG: u8 = 0x0A;
pub const BAD_TRACK_FLAG: u8 = 0x0B;
pub const BAD_MEDIA: u8 = 0x0C;
pub const INVALID_SECTOR_COUNT: u8 = 0x0D;
pub const CONTROL_DATA_MARK: u8 = 0x0E;
pub const DMA_ERROR: u8 = 0x0F;
pub const CRC_ERROR: u8 = 0x10;
pub const ECC_CORRECTED: u8 = 0x11;
pub const CONTROLLER_FAILED: u8 = 0x20;
pub const SEEK_FAILED: u8 = 0x40;
pub const TIMEOUT: u8 = 0x80;
pub const DRIVE_NOT_READY: u8 = 0xAA;
pub const UNDEFINED_ERROR: u8 = 0xBB;
pub const WRITE_FAULT: u8 = 0xCC;
pub const STATUS_REGISTER_ERROR: u8 = 0xE0;
pub const SENSE_OPERATION_FAILED: u8 = 0xFF;

/// EDD "invalid function" (used by the EDD install check and for
/// unsupported functions in general).
pub const INVALID_FUNCTION: u8 = 0x01;

/// VBE return codes in AH (with CF clear).
pub const VBE_SUCCESS: u8 = 0x4F;
pub const VBE_FAILED: u8 = 0x00;
pub const VBE_FUNCTION_NOT_SUPPORTED: u8 = 0x01;
pub const VBE_HARDWARE_NOT_SUPPORTED: u8 = 0x02;
pub const VBE_MODE_NOT_SUPPORTED: u8 = 0x03;
pub const VBE_INVALID_INPUT: u8 = 0x04;
