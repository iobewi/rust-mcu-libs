//! The native Workload image format (**IWNI**, format version 1).
//!
//! A Workload is a binary compiled for one target. This crate defines the *container*
//! the OTA stores and the runtime loads, and the gate that decides, **before any byte is
//! copied to executable memory or any jump is made**, whether an image may run.
//!
//! * It is not ELF. An ELF may exist as an intermediate build artefact; the packer
//!   (`iobewi-workload-pack`) converts it to this compact format.
//! * It is not a Rust ABI. The image says which target it is for, which `RuntimeApi` it
//!   needs and which `WorkloadContext` ABI version its entry point expects; everything
//!   else is explicit little-endian integers.
//! * It does not repeat what OTM2 already records (id, version, digest, size): the
//!   SHA-256 of the whole file is the one OTM2 verifies.
//! * No pointer is stored as a Rust pointer: addresses are 32-bit link addresses of a
//!   fixed-link target layout ([`TargetLayout`]), checked for equality by the loader.
//!
//! Three different versions, three different meanings (see `docs/workload-runtime-api.md`):
//! * `format_version`: this container.
//! * `abi_version`: the layout of `WorkloadContext` the entry point receives.
//! * `RuntimeApi`: the functional contract Agent <-> Workload (global, from OTM2 too).
#![no_std]

use iobewi_firmware_update::compat::otm2::RuntimeApi;

pub const MAGIC: [u8; 4] = *b"IWNI";
pub const FORMAT_VERSION: u16 = 1;
pub const HEADER_LEN: usize = 64;

/// Target classes. The numeric values are part of the format and never reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// Xtensa LX7 (ESP32-S3), 32-bit pointers, little-endian.
    Esp32s3,
    /// RISC-V RV32IMC (ESP32-C3), 32-bit pointers, little-endian. Representable; no
    /// loader exists for it yet.
    Esp32c3,
}

impl Target {
    pub const fn code(self) -> u16 {
        match self {
            Target::Esp32s3 => 1,
            Target::Esp32c3 => 2,
        }
    }

    pub const fn from_code(code: u16) -> Option<Self> {
        match code {
            1 => Some(Target::Esp32s3),
            2 => Some(Target::Esp32c3),
            _ => None,
        }
    }

    /// Required alignment of the entry point, in bytes.
    pub const fn entry_align(self) -> u32 {
        4
    }
}

/// Bit assigned to flags; none are defined in v1, any set bit is rejected.
pub const FLAGS_KNOWN: u32 = 0;

/// What the image says about itself (decoded header).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageHeader {
    pub target: u16,
    pub abi_version: u16,
    pub requires: RuntimeApi,
    pub flags: u32,
    pub image_size: u32,
    pub entry: u32,
    pub code_addr: u32,
    pub code_offset: u32,
    pub code_size: u32,
    pub data_addr: u32,
    pub data_offset: u32,
    pub data_size: u32,
    pub bss_size: u32,
}

/// Fixed-link layout of the loader's executable region on one target: the image is linked
/// for these addresses (no relocation), and the loader refuses any other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TargetLayout {
    pub target: Target,
    /// Instruction-bus address of the code area and its capacity.
    pub code_addr: u32,
    pub code_capacity: u32,
    /// Data-bus address of the data+bss area and its capacity.
    pub data_addr: u32,
    pub data_capacity: u32,
    /// `WorkloadContext` ABI version the loader provides.
    pub abi_version: u16,
    /// The `RuntimeApi` the Agent provides.
    pub provides: RuntimeApi,
}

/// The fixed-link layout of the ESP32-S3 Workload region. **Single source** for the loader,
/// the packer and the host tests; `workload/sdk/ld/esp32s3-v1.ld` repeats the same numbers
/// (a test keeps them equal).
///
/// Physical region = tail of the reclaimed `dram2` area, `0x3FCE3700..0x3FCEB700` on the data
/// bus. The code is *executed* through the instruction-bus alias (`+0xF0000`), the rest is
/// *accessed* through the data bus.
pub const ESP32S3_CODE_ADDR: u32 = 0x403D_3700;
pub const ESP32S3_CODE_CAPACITY: u32 = 0x5000;
pub const ESP32S3_DATA_ADDR: u32 = 0x3FCE_8700;
pub const ESP32S3_DATA_CAPACITY: u32 = 0x3000;
/// Data-bus address of the start of the whole Workload region (where the code bytes are
/// *written*), and its total size.
pub const ESP32S3_REGION_DBUS: u32 = 0x3FCE_3700;
pub const ESP32S3_REGION_SIZE: u32 = ESP32S3_CODE_CAPACITY + ESP32S3_DATA_CAPACITY;
/// Instruction-bus address minus data-bus address for SRAM1 on the ESP32-S3.
pub const ESP32S3_IBUS_MINUS_DBUS: u32 = 0x006F_0000;

/// The ESP32-S3 layout for an Agent providing `provides`.
pub const fn esp32s3_layout(provides: RuntimeApi, abi_version: u16) -> TargetLayout {
    TargetLayout {
        target: Target::Esp32s3,
        code_addr: ESP32S3_CODE_ADDR,
        code_capacity: ESP32S3_CODE_CAPACITY,
        data_addr: ESP32S3_DATA_ADDR,
        data_capacity: ESP32S3_DATA_CAPACITY,
        abi_version,
        provides,
    }
}

/// Why an image cannot run. Each variant refuses *activation* (the candidate stays
/// Staged); none of them has executed or copied anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageError {
    Truncated,
    BadMagic,
    BadFormatVersion(u16),
    BadHeaderLen(u16),
    UnknownTarget(u16),
    TargetMismatch {
        image: u16,
        device: u16,
    },
    AbiMismatch {
        image: u16,
        device: u16,
    },
    RuntimeApiMismatch {
        required: RuntimeApi,
        provided: RuntimeApi,
    },
    UnknownFlags(u32),
    ReservedNotZero,
    /// The size announced in the header differs from the artifact the OTA stored.
    SizeMismatch {
        header: u32,
        artifact: u64,
    },
    BadBounds,
    EmptyCode,
    AddrMismatch,
    TooLarge,
    BadEntry,
}

impl ImageError {
    /// Stable machine-readable reason (used in HTTP and logs).
    pub const fn reason(&self) -> &'static str {
        match self {
            ImageError::Truncated => "truncated",
            ImageError::BadMagic => "bad_magic",
            ImageError::BadFormatVersion(_) => "bad_format_version",
            ImageError::BadHeaderLen(_) => "bad_header_len",
            ImageError::UnknownTarget(_) => "unknown_target",
            ImageError::TargetMismatch { .. } => "target_mismatch",
            ImageError::AbiMismatch { .. } => "abi_mismatch",
            ImageError::RuntimeApiMismatch { .. } => "runtime_api_mismatch",
            ImageError::UnknownFlags(_) => "unknown_flags",
            ImageError::ReservedNotZero => "reserved_not_zero",
            ImageError::SizeMismatch { .. } => "size_mismatch",
            ImageError::BadBounds => "bad_bounds",
            ImageError::EmptyCode => "empty_code",
            ImageError::AddrMismatch => "addr_mismatch",
            ImageError::TooLarge => "too_large",
            ImageError::BadEntry => "bad_entry",
        }
    }
}

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

impl ImageHeader {
    /// Encode (used by the packer and by tests).
    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let mut b = [0u8; HEADER_LEN];
        b[0..4].copy_from_slice(&MAGIC);
        b[4..6].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
        b[6..8].copy_from_slice(&(HEADER_LEN as u16).to_le_bytes());
        b[8..10].copy_from_slice(&self.target.to_le_bytes());
        b[10..12].copy_from_slice(&self.abi_version.to_le_bytes());
        b[12..14].copy_from_slice(&self.requires.major.to_le_bytes());
        b[14..16].copy_from_slice(&self.requires.minor.to_le_bytes());
        b[16..20].copy_from_slice(&self.flags.to_le_bytes());
        b[20..24].copy_from_slice(&self.image_size.to_le_bytes());
        b[24..28].copy_from_slice(&self.entry.to_le_bytes());
        b[28..32].copy_from_slice(&self.code_addr.to_le_bytes());
        b[32..36].copy_from_slice(&self.code_offset.to_le_bytes());
        b[36..40].copy_from_slice(&self.code_size.to_le_bytes());
        b[40..44].copy_from_slice(&self.data_addr.to_le_bytes());
        b[44..48].copy_from_slice(&self.data_offset.to_le_bytes());
        b[48..52].copy_from_slice(&self.data_size.to_le_bytes());
        b[52..56].copy_from_slice(&self.bss_size.to_le_bytes());
        // 56..64 reserved, zero.
        b
    }

    /// Structural decode only (no policy): magic, version, header length, reserved, flags.
    pub fn decode(bytes: &[u8]) -> Result<Self, ImageError> {
        if bytes.len() < HEADER_LEN {
            return Err(ImageError::Truncated);
        }
        if bytes[0..4] != MAGIC {
            return Err(ImageError::BadMagic);
        }
        let version = u16_at(bytes, 4);
        if version != FORMAT_VERSION {
            return Err(ImageError::BadFormatVersion(version));
        }
        let header_len = u16_at(bytes, 6);
        if header_len as usize != HEADER_LEN {
            return Err(ImageError::BadHeaderLen(header_len));
        }
        if bytes[56..64].iter().any(|&b| b != 0) {
            return Err(ImageError::ReservedNotZero);
        }
        let flags = u32_at(bytes, 16);
        if flags & !FLAGS_KNOWN != 0 {
            return Err(ImageError::UnknownFlags(flags));
        }
        Ok(Self {
            target: u16_at(bytes, 8),
            abi_version: u16_at(bytes, 10),
            requires: RuntimeApi::new(u16_at(bytes, 12), u16_at(bytes, 14)),
            flags,
            image_size: u32_at(bytes, 20),
            entry: u32_at(bytes, 24),
            code_addr: u32_at(bytes, 28),
            code_offset: u32_at(bytes, 32),
            code_size: u32_at(bytes, 36),
            data_addr: u32_at(bytes, 40),
            data_offset: u32_at(bytes, 44),
            data_size: u32_at(bytes, 48),
            bss_size: u32_at(bytes, 52),
        })
    }

    /// The whole gate: structure + compatibility + bounds, in this order, with checked
    /// arithmetic. `artifact_size` is the size OTM2 recorded for the stored artifact.
    /// Success means the loader may copy `code`/`data` and jump to `entry`; failure means
    /// nothing was touched.
    pub fn validate(&self, layout: &TargetLayout, artifact_size: u64) -> Result<(), ImageError> {
        if Target::from_code(self.target).is_none() {
            return Err(ImageError::UnknownTarget(self.target));
        }
        if self.target != layout.target.code() {
            return Err(ImageError::TargetMismatch {
                image: self.target,
                device: layout.target.code(),
            });
        }
        if self.abi_version != layout.abi_version {
            return Err(ImageError::AbiMismatch {
                image: self.abi_version,
                device: layout.abi_version,
            });
        }
        if !layout.provides.satisfies(self.requires) {
            return Err(ImageError::RuntimeApiMismatch {
                required: self.requires,
                provided: layout.provides,
            });
        }
        if u64::from(self.image_size) != artifact_size {
            return Err(ImageError::SizeMismatch {
                header: self.image_size,
                artifact: artifact_size,
            });
        }
        if self.code_size == 0 {
            return Err(ImageError::EmptyCode);
        }
        // File bounds: header | code | data, in order, no overlap, inside the image.
        let code_end = self
            .code_offset
            .checked_add(self.code_size)
            .ok_or(ImageError::BadBounds)?;
        let data_end = self
            .data_offset
            .checked_add(self.data_size)
            .ok_or(ImageError::BadBounds)?;
        if (self.code_offset as usize) < HEADER_LEN || code_end > self.image_size {
            return Err(ImageError::BadBounds);
        }
        if self.data_size == 0 {
            if self.data_offset != 0 && self.data_offset < code_end {
                return Err(ImageError::BadBounds);
            }
        } else if self.data_offset < code_end || data_end > self.image_size {
            return Err(ImageError::BadBounds);
        }
        // Fixed-link addresses must be exactly the loader's.
        if self.code_addr != layout.code_addr || self.data_addr != layout.data_addr {
            return Err(ImageError::AddrMismatch);
        }
        if self.code_size > layout.code_capacity {
            return Err(ImageError::TooLarge);
        }
        let data_total = self
            .data_size
            .checked_add(self.bss_size)
            .ok_or(ImageError::BadBounds)?;
        if data_total > layout.data_capacity {
            return Err(ImageError::TooLarge);
        }
        // Entry: inside the *code* area actually provided by the image, aligned.
        let code_limit = self
            .code_addr
            .checked_add(self.code_size)
            .ok_or(ImageError::BadBounds)?;
        let align = Target::from_code(self.target).map_or(4, Target::entry_align);
        if self.entry < self.code_addr
            || self.entry >= code_limit
            || !self.entry.is_multiple_of(align)
        {
            return Err(ImageError::BadEntry);
        }
        Ok(())
    }

    /// Total footprint of the loaded image in the Workload RAM, for reporting.
    pub fn ram_footprint(&self) -> u32 {
        self.code_size
            .saturating_add(self.data_size)
            .saturating_add(self.bss_size)
    }
}

#[cfg(test)]
extern crate std;
#[cfg(test)]
mod tests;
