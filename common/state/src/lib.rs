#![cfg_attr(not(test), no_std)]
#![doc = include_str!("../README.md")]

mod flags;
mod registry;
mod state;

pub use flags::Flags;
pub use registry::{Change, ChangeKind, Full, Observer, Registry, Snapshot, Source, SourceId};
pub use state::State;

#[cfg(test)]
mod tests;
