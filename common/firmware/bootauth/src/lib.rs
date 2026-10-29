#![no_std]
//! Boot-time authenticity of application slots (issue #91).
//!
//! A signature checked at upload is not a Secure Boot: bytes in a slot can change afterwards.
//! This crate decides, **before a slot is executed**, whether it still carries an authentic
//! image, and keeps that decision away from the A/B selection logic: `iobewi-firmware-boot`
//! stays a pure decision engine and only ever sees one more reason for `image_ok` to say no.
//!
//! * [`iap1`] is the proof stored in each slot's last sector (see
//!   `iobewi_firmware_layout::domain_a::PROOF_SECTOR_SIZE`).
//! * [`UnsignedBoot`] and [`VerifiedBoot`] are the two explicit boot profiles. The profile is
//!   chosen when a bootloader is built; `VerifiedBoot` has no path that boots a slot without
//!   a valid proof, and no fallback to the unsigned behaviour.
//! * [`select::plan`] runs `plan_boot` with the profile's decision and keeps a flash read
//!   error apart from an image that is genuinely refused (an I/O error never reaches
//!   `otadata` as `Invalid`).
//! * [`writer`] writes a proof: erase, body, read back, commit word last.
//! * [`provision`] keeps a slot's proof in step with its image across an OTA: invalidate
//!   before the image is rewritten, publish and re-check once it is complete, gate activation.
//!
//! What this does **not** do: it does not protect a bootloader that an attacker can rewrite
//! (that needs the ROM to authenticate the second stage, a separate hardware profile), it
//! has no anti-rollback counter (a validly signed older image still verifies), and the
//! signature algorithm is whatever the adapter provides (the ROM on ESP32).

pub mod iap1;
pub mod policy;
pub mod provision;
pub mod select;
pub mod traits;
pub mod verdict;
pub mod writer;

#[cfg(feature = "host-sign")]
extern crate std;
#[cfg(feature = "host-sign")]
pub mod host;

pub use policy::{BootPolicy, ConfigError, UnsignedBoot, VerifiedBoot};
pub use traits::{Aligned, IoError, Sha256Stream, SlotRead};
pub use verdict::{Authentic, Refusal, Verdict};
