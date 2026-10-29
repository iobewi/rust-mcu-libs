//! The abstract verifier and its ECDSA P-256 implementation.

use crate::manifest::Algorithm;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyError {
    BadSignature,
    /// Not in canonical form (zero or out-of-range scalar, high-S).
    NonCanonical,
    /// The pinned public key is not usable.
    BadKey,
}

/// Verifies one signature over `signed` (domain prefix included) with a pinned key.
pub trait SignatureVerifier {
    const ALGORITHM: Algorithm;
    fn verify(&self, public_key: &[u8], signed: &[u8], signature: &[u8])
    -> Result<(), VerifyError>;
}

/// ECDSA over P-256 with SHA-256, public key as an uncompressed SEC1 point,
/// signature as raw `r || s` with low `s` (one valid encoding per signature).
#[cfg(feature = "verify-p256")]
#[derive(Debug, Clone, Copy, Default)]
pub struct P256Verifier;

#[cfg(feature = "verify-p256")]
impl SignatureVerifier for P256Verifier {
    const ALGORITHM: Algorithm = Algorithm::EcdsaP256Sha256;

    fn verify(
        &self,
        public_key: &[u8],
        signed: &[u8],
        signature: &[u8],
    ) -> Result<(), VerifyError> {
        use p256::ecdsa::{Signature, VerifyingKey, signature::hazmat::PrehashVerifier};
        use sha2::{Digest as _, Sha256};
        if public_key.len() != Algorithm::EcdsaP256Sha256.public_key_len()
            || public_key.first() != Some(&0x04)
        {
            return Err(VerifyError::BadKey);
        }
        let key = VerifyingKey::from_sec1_bytes(public_key).map_err(|_| VerifyError::BadKey)?;
        let signature = Signature::from_slice(signature).map_err(|_| VerifyError::NonCanonical)?;
        if signature.normalize_s().is_some() {
            return Err(VerifyError::NonCanonical);
        }
        key.verify_prehash(&Sha256::digest(signed), &signature)
            .map_err(|_| VerifyError::BadSignature)
    }
}

/// Host-side signing, for the `ota_sign` tool and test vectors. Never linked
/// into firmware: the private key stays on the signing machine.
#[cfg(feature = "host-sign")]
pub mod host {
    use crate::manifest::{Manifest, signed_bytes};
    use p256::ecdsa::{Signature, SigningKey, signature::hazmat::PrehashSigner};
    use sha2::{Digest as _, Sha256};

    /// SEC1 uncompressed public key of `key`.
    pub fn public_key(key: &SigningKey) -> [u8; 65] {
        let point = key.verifying_key().to_encoded_point(false);
        let mut out = [0u8; 65];
        out.copy_from_slice(point.as_bytes());
        out
    }

    /// Deterministic (RFC 6979) low-S signature of `manifest`, raw `r || s`.
    pub fn sign(key: &SigningKey, manifest: &Manifest) -> [u8; 64] {
        let digest = Sha256::digest(signed_bytes(&manifest.encode()));
        let signature: Signature = key.sign_prehash(&digest).expect("prehash signing");
        let signature = signature.normalize_s().unwrap_or(signature);
        let mut out = [0u8; 64];
        out.copy_from_slice(&signature.to_bytes());
        out
    }

    pub use p256;
}
