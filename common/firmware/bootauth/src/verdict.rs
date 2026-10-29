//! The outcome of looking at one slot.

use iobewi_firmware_auth::{AuthError, DecodeError, KeyId};

/// Why a slot is not authentic. All of these are certain statements about the flash content;
/// a read error is [`Verdict::Io`] instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// No proof was ever written (sector erased).
    ProofAbsent,
    /// A proof write was interrupted before its commit word.
    ProofTruncated,
    /// The proof sector holds something that is not an exact, intact record.
    ProofCorrupt,
    /// The record is intact but its IAM1 envelope is not canonical.
    Malformed(DecodeError),
    /// The envelope or record names another algorithm than the one this bootloader verifies.
    AlgorithmNotAllowed,
    /// The key in the proof is not one of the pinned keys.
    UnknownKey,
    /// The pinned key does not have the shape of a key of its algorithm.
    InvalidKey,
    BadSignature,
    /// The signed manifest describes another product, MCU, variant, format or version.
    Target(AuthError),
    /// The signed size is zero or larger than the slot's usable capacity.
    BadSize,
    /// The bytes in the slot do not hash to the signed digest.
    DigestMismatch,
}

impl Refusal {
    /// Stable number for the bootloader's terse log (no `Debug` in 32 KiB).
    pub const fn code(self) -> u32 {
        match self {
            Self::ProofAbsent => 1,
            Self::ProofTruncated => 2,
            Self::ProofCorrupt => 3,
            Self::Malformed(_) => 4,
            Self::AlgorithmNotAllowed => 5,
            Self::UnknownKey => 6,
            Self::InvalidKey => 7,
            Self::BadSignature => 8,
            Self::Target(_) => 9,
            Self::BadSize => 10,
            Self::DigestMismatch => 11,
        }
    }
}

/// Proof that a slot was admitted by its boot profile. It cannot be built outside this
/// crate: the only ways to hold one are `VerifiedBoot::admit` and `UnsignedBoot::admit`.
///
/// ```compile_fail
/// use iobewi_firmware_bootauth::Authentic;
/// let forged = Authentic { size: None, key_id: None };
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Authentic {
    pub(crate) size: Option<u32>,
    pub(crate) key_id: Option<KeyId>,
}

impl Authentic {
    pub(crate) const fn unverified() -> Self {
        Self {
            size: None,
            key_id: None,
        }
    }

    /// Signed image size, if the profile verified one.
    pub const fn signed_size(&self) -> Option<u32> {
        self.size
    }

    pub const fn key_id(&self) -> Option<KeyId> {
        self.key_id
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Authentic(Authentic),
    Refused(Refusal),
    /// A flash read failed: nothing is known about the slot. Not a refusal.
    Io,
}
