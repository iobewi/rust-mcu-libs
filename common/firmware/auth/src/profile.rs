//! The two authentication profiles of the single OTA engine.
//!
//! A profile is a type. The engine (`iobewi_firmware_update::service::OtaService`)
//! is generic over it, and each profile fixes its own kind of proof:
//!
//! * [`UnsignedAllowed`] yields [`Unsigned`]: integrity and compatibility only;
//! * [`SignatureRequired`] yields [`Admitted`]: a signed IAM1 manifest accepted
//!   by the local trust policy and bound to one image.
//!
//! Proofs have private fields and no public constructor, so neither can be
//! built outside this crate, and a service of one profile does not type-check
//! with the other profile's proof.
//!
//! ```compile_fail
//! // Cannot be fabricated: private fields.
//! let _ = iobewi_firmware_auth::Admitted { manifest: todo!() };
//! ```
//!
//! ```compile_fail
//! // Cannot be fabricated: no public constructor.
//! let _ = iobewi_firmware_auth::Unsigned(());
//! ```

use crate::error::{AuthError, PolicyError};
use crate::manifest::{Envelope, KeyId, Manifest, signed_bytes};
use crate::policy::{KeyState, TrustPolicy};
use crate::verify::{SignatureVerifier, VerifyError};
use iobewi_firmware_image::Digest;

mod sealed {
    pub trait Sealed {}
}

/// What an uploader claims about the image it is about to send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UploadClaim {
    pub size: u64,
    pub digest: Digest,
}

/// What the transaction record says is staged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StagedClaim {
    pub size: u64,
    pub digest: Digest,
}

/// How an OTA service decides that an image may be published and activated.
/// Sealed: only the two profiles of this crate exist.
pub trait AuthProfile: sealed::Sealed {
    type Proof;
    /// Whether the bytes present in the slot must be hashed again before activation.
    const REHASH_SLOT_ON_ACTIVATION: bool;

    /// Before any erase: authorizes the claimed image.
    fn admit_upload(
        &self,
        presented: Option<&[u8]>,
        claim: &UploadClaim,
    ) -> Result<Self::Proof, AuthError>;

    /// At activation, under the current policy: authorizes the staged image
    /// from what the client presents again.
    fn admit_activation(
        &self,
        presented: Option<&[u8]>,
        staged: &StagedClaim,
    ) -> Result<Self::Proof, AuthError>;

    /// The proof describes exactly this image.
    fn check_binding(
        &self,
        proof: &Self::Proof,
        size: u64,
        digest: &Digest,
    ) -> Result<(), AuthError>;
}

/// Proof of the unsigned profile: nothing was authenticated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unsigned(());

/// Proof of the signed profile: a manifest accepted by the trust policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Admitted {
    manifest: Manifest,
}

impl Admitted {
    pub fn size(&self) -> u64 {
        u64::from(self.manifest.size)
    }
    pub fn digest(&self) -> Digest {
        self.manifest.digest
    }
    pub fn key_id(&self) -> KeyId {
        self.manifest.key_id
    }
    pub fn security_version(&self) -> u32 {
        self.manifest.security_version
    }
}

/// Integrity and compatibility only; the signature is not required.
#[derive(Debug, Clone, Copy, Default)]
pub struct UnsignedAllowed;

impl sealed::Sealed for UnsignedAllowed {}

impl AuthProfile for UnsignedAllowed {
    type Proof = Unsigned;
    const REHASH_SLOT_ON_ACTIVATION: bool = false;

    fn admit_upload(&self, _: Option<&[u8]>, _: &UploadClaim) -> Result<Unsigned, AuthError> {
        Ok(Unsigned(()))
    }
    fn admit_activation(&self, _: Option<&[u8]>, _: &StagedClaim) -> Result<Unsigned, AuthError> {
        Ok(Unsigned(()))
    }
    fn check_binding(&self, _: &Unsigned, _: u64, _: &Digest) -> Result<(), AuthError> {
        Ok(())
    }
}

/// A signed IAM1 manifest accepted by the local trust policy is mandatory.
/// There is no way to build it without keys, and no fallback to unsigned.
pub struct SignatureRequired<'a, V: SignatureVerifier> {
    policy: TrustPolicy<'a>,
    verifier: V,
}

impl<V: SignatureVerifier> sealed::Sealed for SignatureRequired<'_, V> {}

impl<'a, V: SignatureVerifier> SignatureRequired<'a, V> {
    pub fn new(policy: TrustPolicy<'a>, verifier: V) -> Result<Self, PolicyError> {
        if policy.keys.is_empty() {
            return Err(PolicyError::NoKeys);
        }
        if !policy.keys.iter().any(|k| k.state == KeyState::Active) {
            return Err(PolicyError::NoActiveKey);
        }
        if policy.hw_variants.is_empty() {
            return Err(PolicyError::NoVariants);
        }
        for (i, key) in policy.keys.iter().enumerate() {
            if key.algorithm != V::ALGORITHM {
                return Err(PolicyError::AlgorithmMismatch);
            }
            if key.public_key.len() != key.algorithm.public_key_len()
                || key.public_key.first() != Some(&0x04)
            {
                return Err(PolicyError::BadKey);
            }
            if policy.keys[..i].iter().any(|other| other.id == key.id) {
                return Err(PolicyError::DuplicateKeyId);
            }
        }
        Ok(Self { policy, verifier })
    }

    pub fn policy(&self) -> &TrustPolicy<'a> {
        &self.policy
    }

    fn authorize(
        &self,
        presented: Option<&[u8]>,
        size: u64,
        digest: &Digest,
    ) -> Result<Admitted, AuthError> {
        let envelope = Envelope::decode(presented.ok_or(AuthError::EnvelopeMissing)?)?;
        let manifest = envelope.manifest;
        let key = self
            .policy
            .keys
            .iter()
            .find(|key| key.id == manifest.key_id)
            .ok_or(AuthError::UnknownKey)?;
        if key.state == KeyState::Revoked {
            return Err(AuthError::RevokedKey);
        }
        // The manifest names an algorithm; the policy pins the one this key
        // may be used with. A mismatch is a refusal, never a negotiation.
        if manifest.algorithm != key.algorithm || key.algorithm != V::ALGORITHM {
            return Err(AuthError::AlgorithmNotAllowed);
        }
        self.verifier
            .verify(
                key.public_key,
                &signed_bytes(&envelope.manifest_bytes),
                envelope.signature,
            )
            .map_err(|error| match error {
                VerifyError::BadSignature => AuthError::BadSignature,
                VerifyError::NonCanonical => {
                    AuthError::Malformed(crate::error::DecodeError::Signature)
                }
                VerifyError::BadKey => AuthError::InvalidKey,
            })?;
        // Only now is the manifest content trusted enough to be compared.
        if manifest.image_format != self.policy.image_format {
            return Err(AuthError::WrongImageFormat);
        }
        if manifest.mcu != self.policy.mcu {
            return Err(AuthError::WrongMcu);
        }
        if manifest.product != self.policy.product {
            return Err(AuthError::WrongProduct);
        }
        if !self.policy.hw_variants.contains(&manifest.hw_variant) {
            return Err(AuthError::WrongVariant);
        }
        if manifest.security_version < self.policy.min_security_version {
            return Err(AuthError::SecurityVersionTooLow);
        }
        if u64::from(manifest.size) != size {
            return Err(AuthError::SizeMismatch);
        }
        if manifest.digest != *digest {
            return Err(AuthError::DigestMismatch);
        }
        Ok(Admitted { manifest })
    }
}

impl<V: SignatureVerifier> AuthProfile for SignatureRequired<'_, V> {
    type Proof = Admitted;
    const REHASH_SLOT_ON_ACTIVATION: bool = true;

    fn admit_upload(
        &self,
        presented: Option<&[u8]>,
        claim: &UploadClaim,
    ) -> Result<Admitted, AuthError> {
        self.authorize(presented, claim.size, &claim.digest)
    }

    fn admit_activation(
        &self,
        presented: Option<&[u8]>,
        staged: &StagedClaim,
    ) -> Result<Admitted, AuthError> {
        self.authorize(presented, staged.size, &staged.digest)
    }

    fn check_binding(&self, proof: &Admitted, size: u64, digest: &Digest) -> Result<(), AuthError> {
        if proof.size() == size && proof.digest() == *digest {
            Ok(())
        } else {
            Err(AuthError::ProofMismatch)
        }
    }
}
