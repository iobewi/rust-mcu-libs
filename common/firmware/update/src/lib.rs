#![no_std]

//! Portable, transport- and hardware-independent firmware update mechanics.
//! Domain A and domain B are optional policies composed above this crate.
//! No flash driver, bootloader, HTTP, reboot, or activation authority lives here.

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod artifact;
pub mod bootstrap;
pub mod compat;
#[cfg(feature = "config-space")]
pub mod config_space;
pub mod error;
/// Historical OTA metadata API; reuse the sole OTM1 implementation.
pub mod metadata {
    pub use crate::compat::otm1::*;
}
pub mod service;
pub mod state;
pub mod storage;
pub mod transaction;

pub use artifact::{Committed, Digest, ResumePlan, WriteSession, is_complete, resume_plan};
pub use error::Error;
pub use state::{Action, BackendOutcome, TransactionState};
pub use storage::{ArtifactStorage, TransactionMetadata};
pub use transaction::{
    ArtifactRecord, TransactionRecord, activate, clear_stale, finish, reconcile,
};
