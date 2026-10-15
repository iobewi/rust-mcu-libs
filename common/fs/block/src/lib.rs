#![no_std]
#![doc = include_str!("../README.md")]

/// Sector size of the read-only block contract, in bytes.
pub const SECTOR_SIZE: usize = 512;

/// Result of a non-blocking sector read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadStatus {
    /// The complete sector is available in the output buffer.
    Ready,
    /// Data may become available on a subsequent read.
    Pending,
    /// Data is no longer available, or the address is outside the medium.
    Expired,
}

/// A read-only medium with 512-byte sectors and explicit consumer sessions.
///
/// Reads must not wait for data. On `Pending` or `Expired`, implementations must
/// zero the entire output buffer. Read-only prohibits writes through this API;
/// it does not promise immutable content across reads or sessions.
///
/// The caller owns session timing and retry/timeout policy. The capacity must
/// remain stable for the lifetime of the device. `last_lba` is inclusive, so this
/// contract represents a nonempty medium.
pub trait ReadOnlyBlockDevice {
    fn last_lba(&self) -> u32;
    fn begin_session(&mut self) {}
    fn end_session(&mut self) {}
    fn read_sector(&mut self, lba: u32, out: &mut [u8; SECTOR_SIZE]) -> ReadStatus;
}
