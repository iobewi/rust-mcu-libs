#![no_std]

//! ESP32 legacy A-only firmware layout mapping.
//! Pure validation, independent of flash access. The actual flash reader,
//! writer and custom transactional-boot-metadata-compatible bootloader are separate integration gates.

#[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
pub mod flash;
#[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
pub mod otadata_esp;
#[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
pub mod platform;

pub mod otadata;
pub mod running;
pub mod selection;

// The production adapter and host tests use the same implementation.
pub use iobewi_firmware_layout::domain_a::*;
