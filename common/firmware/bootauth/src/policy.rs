//! The two explicit boot profiles.

use core::marker::PhantomData;

use iobewi_firmware_auth::{
    DecodeError, KeyId, RawManifest, SignatureVerifier, Target, VerifyError,
    manifest::{MANIFEST_LEN, signed_bytes},
};
use iobewi_firmware_layout::domain_a::Partition;

use crate::iap1::{self, Header};
use crate::traits::{Aligned, Sha256Stream, SlotRead};
use crate::verdict::{Authentic, Refusal, Verdict};

mod sealed {
    pub trait Sealed {}
}

/// How a bootloader decides that a slot may be executed. Sealed: the profiles below are the
/// only ones, so no third kind of "boot anything" policy can be slipped in.
pub trait BootPolicy: sealed::Sealed {
    /// Printed once at boot, like the OTA profile label.
    const LABEL: &'static str;

    fn admit<R: SlotRead>(&self, reader: &mut R, slot: Partition) -> Verdict;
}

/// Structure and integrity checks only (done by the bootloader, as before): no proof is
/// read, no asymmetric cryptography is linked or run.
#[derive(Debug, Clone, Copy, Default)]
pub struct UnsignedBoot;

impl sealed::Sealed for UnsignedBoot {}

impl BootPolicy for UnsignedBoot {
    const LABEL: &'static str = "boot authentication: UNSIGNED";

    fn admit<R: SlotRead>(&self, _: &mut R, _: Partition) -> Verdict {
        Verdict::Authentic(Authentic::unverified())
    }
}

/// A verified-boot profile that cannot work and so must not exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigError {
    /// No pinned key: it would refuse every slot, which is a mistake, not a mode.
    NoPins,
    /// No hardware variant allowed.
    NoVariants,
    /// A pin is all zeros (an unset placeholder).
    ZeroPin,
}

/// Signature required, pinned keys, no unauthenticated fallback.
///
/// Keys are pinned by the SHA-256 of their complete encoding (for RSA, modulus, exponent and
/// both precomputed Montgomery values: a changed `rinv` must not be able to pass).
pub struct VerifiedBoot<'a, V: SignatureVerifier, H: Sha256Stream> {
    target: Target<'a>,
    pins: &'a [[u8; 32]],
    verifier: V,
    hash: PhantomData<H>,
}

impl<'a, V: SignatureVerifier, H: Sha256Stream> VerifiedBoot<'a, V, H> {
    pub fn new(target: Target<'a>, pins: &'a [[u8; 32]], verifier: V) -> Result<Self, ConfigError> {
        if pins.is_empty() {
            return Err(ConfigError::NoPins);
        }
        if pins.contains(&[0; 32]) {
            return Err(ConfigError::ZeroPin);
        }
        if target.hw_variants.is_empty() {
            return Err(ConfigError::NoVariants);
        }
        Ok(Self {
            target,
            pins,
            verifier,
            hash: PhantomData,
        })
    }

    fn verify_signature(
        &self,
        key: &[u8],
        manifest: &RawManifest<'_>,
        signature: &[u8],
    ) -> Result<(), Refusal> {
        let signed = signed_bytes(manifest.bytes());
        // A refusal is believed only if it repeats: one transient fault must not turn a good
        // slot into a rejected one (which `plan_boot` would then record in `otadata`).
        let mut last = Refusal::BadSignature;
        for _ in 0..2 {
            match self.verifier.verify(key, &signed, signature) {
                Ok(()) => return Ok(()),
                Err(VerifyError::BadSignature) => last = Refusal::BadSignature,
                Err(VerifyError::NonCanonical) => {
                    return Err(Refusal::Malformed(DecodeError::Signature));
                }
                Err(VerifyError::BadKey) => return Err(Refusal::InvalidKey),
            }
        }
        Err(last)
    }
}

impl<V: SignatureVerifier, H: Sha256Stream> sealed::Sealed for VerifiedBoot<'_, V, H> {}

impl<V: SignatureVerifier, H: Sha256Stream> BootPolicy for VerifiedBoot<'_, V, H> {
    const LABEL: &'static str = "boot authentication: VERIFIED";

    fn admit<R: SlotRead>(&self, reader: &mut R, slot: Partition) -> Verdict {
        macro_rules! io {
            ($e:expr) => {
                if $e.is_err() {
                    return Verdict::Io;
                }
            };
        }
        let base = slot.proof_offset();
        let mut head = Aligned([0u8; iap1::HEADER_LEN]);
        io!(reader.read(base, &mut head.0));
        let (algorithm, crc) = match iap1::classify(&head.0) {
            Header::Erased => return Verdict::Refused(Refusal::ProofAbsent),
            Header::Truncated => return Verdict::Refused(Refusal::ProofTruncated),
            Header::Corrupt => return Verdict::Refused(Refusal::ProofCorrupt),
            Header::Committed { algorithm, crc } => (algorithm, crc),
        };
        if algorithm != V::ALGORITHM {
            return Verdict::Refused(Refusal::AlgorithmNotAllowed);
        }
        let total = iap1::payload_len(algorithm);
        let mut payload = Aligned([0u8; iap1::MAX_PAYLOAD]);
        io!(reader.read(base + iap1::HEADER_LEN as u32, &mut payload.0[..total]));
        if iap1::crc(&head.0, &payload.0[..total]) != crc {
            return Verdict::Refused(Refusal::ProofCorrupt);
        }
        let split = iap1::envelope_len(algorithm);
        // The manifest is read in place (no enumeration decoding): the boot stage has very
        // little code space, and a field it does not understand is simply a mismatch.
        let manifest = match RawManifest::parse(&payload.0[..MANIFEST_LEN]) {
            Ok(manifest) => manifest,
            Err(error) => return Verdict::Refused(Refusal::Malformed(error)),
        };
        let signature = &payload.0[MANIFEST_LEN..split];
        let key = &payload.0[split..total];
        if manifest.algorithm_id() != algorithm.id() {
            return Verdict::Refused(Refusal::AlgorithmNotAllowed);
        }
        if !algorithm.key_shape_ok(key) {
            return Verdict::Refused(Refusal::InvalidKey);
        }
        let fingerprint = H::digest(key);
        if !self.pins.contains(&fingerprint) || manifest.key_id() != &fingerprint[..8] {
            return Verdict::Refused(Refusal::UnknownKey);
        }
        if let Err(refusal) = self.verify_signature(key, &manifest, signature) {
            return Verdict::Refused(refusal);
        }
        // Only a verified manifest is worth comparing with the target.
        if let Err(error) = self.target.check_raw(&manifest) {
            return Verdict::Refused(Refusal::Target(error));
        }
        let size = manifest.size();
        if size == 0 || size > slot.usable_size() {
            return Verdict::Refused(Refusal::BadSize);
        }
        let mut hash = H::new();
        let mut chunk = Aligned([0u8; 256]);
        let mut done = 0u32;
        while done < size {
            let n = (size - done).min(chunk.0.len() as u32) as usize;
            io!(reader.read(slot.offset + done, &mut chunk.0[..n]));
            hash.update(&chunk.0[..n]);
            done += n as u32;
        }
        if hash.finish() != *manifest.digest() {
            return Verdict::Refused(Refusal::DigestMismatch);
        }
        Verdict::Authentic(Authentic {
            size: Some(size),
            key_id: Some(KeyId(*manifest.key_id())),
        })
    }
}
