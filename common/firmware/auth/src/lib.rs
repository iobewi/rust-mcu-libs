#![no_std]
//! Authenticity of firmware images, distinct from their integrity.
//!
//! The SHA-256 of an image proves that bytes arrived intact; it says nothing
//! about who produced them. This crate adds a signed manifest (IAM1), a local
//! trust policy made of pinned public keys, and two authentication profiles
//! for the one OTA engine. It does not verify anything at boot and it is not
//! Secure Boot: it authorizes what is *published and activated by OTA*.
//!
//! Layout: [`manifest`] (canonical encoding), [`policy`] (what the device
//! accepts), [`verify`] (abstract verifier, ECDSA P-256 behind `verify-p256`),
//! [`profile`] (the two profiles and their proofs), [`error`].

pub mod error;
pub mod manifest;
pub mod policy;
pub mod profile;
pub mod verify;

pub use error::{AuthError, DecodeError, PolicyError};
pub use manifest::{
    Algorithm, Envelope, ImageFormat, KeyId, MAX_ENVELOPE, MAX_ENVELOPE_TEXT, Manifest, Mcu,
    Product, RSA_PSS_3072_PUBLIC_KEY_LEN, RSA_PSS_3072_SIGNATURE_LEN, RawManifest, decode_b64url,
    encode_b64url,
};
pub use policy::{KeyState, Target, TrustPolicy, TrustedKey};
pub use profile::{
    Admitted, AuthProfile, SignatureRequired, StagedClaim, Unsigned, UnsignedAllowed, UploadClaim,
};
#[cfg(feature = "verify-p256")]
pub use verify::P256Verifier;
pub use verify::{SignatureVerifier, VerifyError};

#[cfg(test)]
extern crate std;
