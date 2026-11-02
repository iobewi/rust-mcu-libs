//! Portable software SHA-256: the fallback for a platform whose hardware backend is not available
//! or not proven. Safe Rust (RustCrypto `sha2`, `no_std`, no allocation), the same code on every
//! architecture. Slower than a hardware block and larger in the boot stage, which is why it is
//! not the first choice where an accelerator exists (`docs/firmware/verified-boot.md`).

use sha2::{Digest, Sha256};

use crate::traits::Sha256Stream;

pub struct SoftSha256(Sha256);

impl Sha256Stream for SoftSha256 {
    fn new() -> Self {
        Self(Sha256::new())
    }

    fn update(&mut self, data: &[u8]) {
        Digest::update(&mut self.0, data);
    }

    fn finish(self) -> [u8; 32] {
        self.0.finalize().into()
    }
}
