//! Local trust policy: what this device accepts. The manifest never decides
//! any of it; it is only checked against the policy.

use crate::manifest::{Algorithm, ImageFormat, KeyId, Mcu, Product};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyState {
    Active,
    /// Pinned but refused: static revocation, decided when the firmware is built.
    Revoked,
}

/// One pinned public key, like an `authorized_keys` line: an identifier, the
/// single algorithm it may be used with, and its state.
#[derive(Debug, Clone, Copy)]
pub struct TrustedKey<'a> {
    pub id: KeyId,
    pub algorithm: Algorithm,
    pub public_key: &'a [u8],
    pub state: KeyState,
}

impl<'a> TrustedKey<'a> {
    /// Pins `public_key`; the identifier is derived from the key itself.
    pub fn pin(algorithm: Algorithm, public_key: &'a [u8]) -> Self {
        Self {
            id: KeyId::of(public_key),
            algorithm,
            public_key,
            state: KeyState::Active,
        }
    }

    pub fn revoked(mut self) -> Self {
        self.state = KeyState::Revoked;
        self
    }
}

/// Everything a signed profile enforces. There is deliberately no `Default`:
/// an unconfigured policy must not exist.
#[derive(Debug, Clone, Copy)]
pub struct TrustPolicy<'a> {
    pub product: Product,
    pub mcu: Mcu,
    pub image_format: ImageFormat,
    pub hw_variants: &'a [u16],
    /// Static floor; a persistent anti-rollback counter is a separate lot.
    pub min_security_version: u32,
    pub keys: &'a [TrustedKey<'a>],
}
