//! What the verifier needs from its environment. The ROM provides all of it on ESP32.

/// A flash read failed. Never a statement about the image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IoError;

/// Reads bytes at an absolute flash offset.
pub trait SlotRead {
    fn read(&mut self, offset: u32, out: &mut [u8]) -> Result<(), IoError>;
}

/// Streaming SHA-256.
pub trait Sha256Stream: Sized {
    fn new() -> Self;
    fn update(&mut self, data: &[u8]);
    fn finish(self) -> [u8; 32];

    fn digest(data: &[u8]) -> [u8; 32] {
        let mut hash = Self::new();
        hash.update(data);
        hash.finish()
    }
}

/// A byte buffer aligned for the ROM crypto routines, which read words.
#[repr(C, align(4))]
pub struct Aligned<const N: usize>(pub [u8; N]);
