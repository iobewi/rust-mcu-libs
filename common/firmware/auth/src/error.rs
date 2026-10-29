//! Typed refusals. Callers map them to one generic transport answer and log
//! the detail; nothing here is meant to be shown to the uploader.

/// The presented bytes are not a canonical IAM1 envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// Wrong length for the declared algorithm, or trailing/missing bytes.
    Length,
    Magic,
    Version,
    /// Algorithm identifier that is neither implemented nor reserved.
    Algorithm,
    /// Reserved algorithm identifier (Ed25519, RSA-PSS): not implemented.
    UnsupportedAlgorithm,
    ImageFormat,
    Mcu,
    /// Reserved bytes are not zero.
    Reserved,
    /// Product tag is empty, not printable ASCII, or not zero padded.
    Product,
    /// Signature is not in canonical form (zero scalar, high-S, bad range).
    Signature,
    /// Header text is not strict unpadded base64url.
    Base64,
}

/// Why an image was not authorized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthError {
    /// A signed profile received no envelope.
    EnvelopeMissing,
    Malformed(DecodeError),
    /// The manifest names an algorithm other than the one pinned for its key.
    AlgorithmNotAllowed,
    UnknownKey,
    RevokedKey,
    /// The pinned public key is unusable (not a point of the curve).
    InvalidKey,
    BadSignature,
    WrongImageFormat,
    WrongMcu,
    WrongProduct,
    WrongVariant,
    SecurityVersionTooLow,
    /// Manifest size differs from the size being uploaded or staged.
    SizeMismatch,
    /// Manifest digest differs from the digest being uploaded or staged.
    DigestMismatch,
    /// The proof does not describe this image (different size or digest).
    ProofMismatch,
    /// The proof was issued under another trust policy (other keys, state or
    /// target constraints) than the one of the service asked to use it.
    PolicyMismatch,
    /// The bytes present in the slot do not hash to the authorized digest.
    SlotDigestMismatch,
}

impl From<DecodeError> for AuthError {
    fn from(error: DecodeError) -> Self {
        Self::Malformed(error)
    }
}

/// A trust policy that cannot be used to build a signed profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyError {
    /// No pinned key at all: a signed profile would accept nothing, which is
    /// a configuration mistake, never a reason to fall back to unsigned.
    NoKeys,
    /// No key is active (all revoked).
    NoActiveKey,
    DuplicateKeyId,
    /// A pinned key has the wrong length or encoding for its algorithm.
    BadKey,
    /// The product tag is not a valid IAM1 product tag.
    BadProduct,
    /// No hardware variant is allowed: the policy would accept nothing.
    NoVariants,
    /// A key is pinned for an algorithm the verifier does not implement.
    AlgorithmMismatch,
}
