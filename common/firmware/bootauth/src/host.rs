//! Host-only RSA-PSS 3072 helpers: key generation, signing, a reference verifier and SHA-256.
//! They produce and check exactly what the ESP32 ROM verifies (Espressif's key and
//! signature-block encoding), so tests exercise real signatures and the packer tool
//! builds real proofs. Never linked into firmware.

use std::vec::Vec;

use iobewi_firmware_auth::manifest::signed_bytes;
use iobewi_firmware_auth::{Algorithm, Manifest, SignatureVerifier, VerifyError};
use rsa::pkcs8::{DecodePrivateKey, EncodePrivateKey, LineEnding};
use rsa::sha2::{Digest as _, Sha256};
use rsa::traits::PublicKeyParts;
use rsa::{BigUint, Pss, RsaPrivateKey, RsaPublicKey};

use crate::traits::Sha256Stream;

/// SHA-256 for tests and tools.
pub struct HostSha256(Sha256);

impl Sha256Stream for HostSha256 {
    fn new() -> Self {
        Self(Sha256::new())
    }
    fn update(&mut self, data: &[u8]) {
        self.0.update(data);
    }
    fn finish(self) -> [u8; 32] {
        self.0.finalize().into()
    }
}

/// `n` (384, LE) | `e` (u32 LE) | `rinv = 2^6144 mod n` (384, LE) | `mdash = -n^-1 mod 2^32` (u32 LE).
pub fn key_struct(public: &RsaPublicKey) -> [u8; 776] {
    let n = public.n();
    let mut out = [0u8; 776];
    let le = |value: &BigUint| {
        let mut bytes = value.to_bytes_le();
        bytes.resize(384, 0);
        bytes
    };
    out[..384].copy_from_slice(&le(n));
    let e = public.e().to_bytes_le();
    let mut e4 = [0u8; 4];
    e4[..e.len().min(4)].copy_from_slice(&e[..e.len().min(4)]);
    out[384..388].copy_from_slice(&e4);
    let rinv = (BigUint::from(1u8) << 6144usize) % n;
    out[388..772].copy_from_slice(&le(&rinv));
    // Newton iteration for the inverse of the lowest word modulo 2^32.
    let n0 = out[0] as u32 | (out[1] as u32) << 8 | (out[2] as u32) << 16 | (out[3] as u32) << 24;
    let mut inverse = n0;
    for _ in 0..5 {
        inverse = inverse.wrapping_mul(2u32.wrapping_sub(n0.wrapping_mul(inverse)));
    }
    out[772..776].copy_from_slice(&inverse.wrapping_neg().to_le_bytes());
    out
}

pub fn public_from_struct(key: &[u8]) -> Option<RsaPublicKey> {
    if key.len() != 776 {
        return None;
    }
    let n = BigUint::from_bytes_le(&key[..384]);
    let e = BigUint::from(u32::from_le_bytes(key[384..388].try_into().ok()?));
    RsaPublicKey::new(n, e).ok()
}

pub struct HostKey {
    pub private: RsaPrivateKey,
    pub public_struct: [u8; 776],
}

impl HostKey {
    pub fn generate() -> Self {
        let private = RsaPrivateKey::new(&mut rand_core::OsRng, 3072).expect("keygen");
        Self::from_private(private)
    }

    pub fn from_pem(pem: &str) -> Option<Self> {
        Some(Self::from_private(RsaPrivateKey::from_pkcs8_pem(pem).ok()?))
    }

    pub fn to_pem(&self) -> std::string::String {
        let pem = self.private.to_pkcs8_pem(LineEnding::LF).expect("pem");
        std::string::String::from(pem.as_str())
    }

    fn from_private(private: RsaPrivateKey) -> Self {
        let public_struct = key_struct(&private.to_public_key());
        Self {
            private,
            public_struct,
        }
    }

    /// SHA-256 of the key encoding: the value a bootloader pins.
    pub fn fingerprint(&self) -> [u8; 32] {
        Sha256::digest(self.public_struct).into()
    }

    /// Signature over the IAM1 signed bytes, little-endian, salt length 32.
    pub fn sign(&self, manifest: &Manifest) -> [u8; 384] {
        let digest = Sha256::digest(signed_bytes(&manifest.encode()));
        let be = self
            .private
            .sign_with_rng(
                &mut rand_core::OsRng,
                Pss::new_with_salt::<Sha256>(32),
                &digest,
            )
            .expect("sign");
        let mut out = [0u8; 384];
        for (i, byte) in be.iter().rev().enumerate() {
            out[i] = *byte;
        }
        out
    }

    /// `manifest || signature`: the IAM1 envelope.
    pub fn envelope(&self, manifest: &Manifest) -> Vec<u8> {
        let mut out = manifest.encode().to_vec();
        out.extend_from_slice(&self.sign(manifest));
        out
    }
}

/// Reference verifier: strict RSASSA-PSS, SHA-256, MGF1-SHA-256, salt length exactly 32.
#[derive(Debug, Clone, Copy, Default)]
pub struct HostRsaVerifier;

impl SignatureVerifier for HostRsaVerifier {
    const ALGORITHM: Algorithm = Algorithm::RsaPss3072Sha256;

    fn verify(&self, key: &[u8], signed: &[u8], signature: &[u8]) -> Result<(), VerifyError> {
        let public = public_from_struct(key).ok_or(VerifyError::BadKey)?;
        if signature.len() != 384 {
            return Err(VerifyError::NonCanonical);
        }
        let mut be = [0u8; 384];
        for (i, byte) in signature.iter().rev().enumerate() {
            be[i] = *byte;
        }
        public
            .verify(
                Pss::new_with_salt::<Sha256>(32),
                &Sha256::digest(signed),
                &be,
            )
            .map_err(|_| VerifyError::BadSignature)
    }
}
