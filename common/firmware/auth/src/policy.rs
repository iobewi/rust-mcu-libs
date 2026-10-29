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

/// Domain separation for [`TrustPolicy::fingerprint`].
const POLICY_DOMAIN: &[u8; 21] = b"iobewi.ota.policy.v1\0";

impl TrustPolicy<'_> {
    /// SHA-256 over every element that decides what is accepted: the target
    /// constraints and, for each pinned key, its identifier, algorithm, state
    /// and public key bytes. A proof carries it, so it only ever authorizes
    /// under the exact policy that issued it (another key set, a revoked key,
    /// the same identifier with another public key, a raised floor...).
    /// Order matters on purpose: a differently ordered policy is a different one.
    pub(crate) fn fingerprint(&self) -> [u8; 32] {
        use sha2::{Digest as _, Sha256};
        let mut hash = Sha256::new();
        hash.update(POLICY_DOMAIN);
        hash.update([self.mcu.id(), self.image_format.id()]);
        hash.update(self.product.as_str().as_bytes());
        hash.update([0]);
        hash.update(self.min_security_version.to_le_bytes());
        hash.update((self.hw_variants.len() as u32).to_le_bytes());
        for variant in self.hw_variants {
            hash.update(variant.to_le_bytes());
        }
        hash.update((self.keys.len() as u32).to_le_bytes());
        for key in self.keys {
            hash.update(key.id.0);
            hash.update([
                key.algorithm.id(),
                match key.state {
                    KeyState::Active => 0,
                    KeyState::Revoked => 1,
                },
            ]);
            hash.update((key.public_key.len() as u32).to_le_bytes());
            hash.update(key.public_key);
        }
        hash.finalize().into()
    }
}
