#![no_std]

//! What a firmware image *is*, independent of how it is transported, staged
//! or booted:
//!
//! * [`digest`]: the SHA-256 digest type and its `sha256:<hex>` text form
//!   (transport integrity of an artifact, computed while it is written);
//! * [`esp`]: bootability of an ESP application image sitting in flash
//!   (header, segment table, checksum, memory-map constraints) -- the
//!   bootloader's check, pure and host-testable.
//!
//! No flash access, no partition table, no OTA state: a caller supplies a
//! reader. Logical firmware domains/slots and boot state belong to separate crates.

pub mod digest;
pub mod esp;

#[cfg(feature = "alloc")]
pub use digest::format_digest;
pub use digest::{parse_digest, Digest};

#[cfg(test)]
extern crate alloc;
#[cfg(test)]
extern crate std;
#[cfg(test)]
mod esp_tests;
