//! Engine-level errors. Generic over `E`, the backend's own error type (from
//! [`crate::storage::ArtifactStorage::Error`] or
//! [`crate::storage::TransactionMetadata::Error`]) -- the engine never
//! invents its own storage error variants, it only adds the outcomes that
//! are its own to decide (a digest that doesn't match, a transition that
//! isn't legal from the current state, ...).

/// `#[non_exhaustive]`: this is not a stable API yet (see the crate's own
/// doc comment) and new outcomes are expected as the multi-artifact and
/// rollback paths grow.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error<E> {
    /// The backend (storage or metadata) reported a failure of its own.
    Backend(E),
    /// The declared size doesn't fit whatever bound the caller enforces
    /// (a partition, a slot, ...) -- checked by the caller, surfaced here
    /// only for callers that want one error type end to end.
    TooLarge,
    /// A write was attempted while another one -- or an activation -- was
    /// already in flight for this transaction.
    Busy,
    /// The digest computed while writing doesn't match the one the session
    /// was opened with. Never a post-hoc re-read: see [`crate::artifact`]'s
    /// doc comment for why that distinction is load-bearing. Carries the
    /// digest that *was* computed (the caller already has the expected
    /// one) -- a mismatch is exactly the failure a caller most needs to
    /// log both sides of to diagnose (corrupted transit vs. a wrong file).
    DigestMismatch(crate::artifact::Digest),
    /// The session (or the final flush) ended short of the declared size.
    /// Carries how many bytes were actually durable when that was
    /// detected -- the other half of the pair a "session ended short"
    /// diagnostic needs (the declared size is whatever the caller already
    /// opened the session with).
    Incomplete { durable: u64 },
    /// There is nothing staged to act on (activate/confirm/reject with an
    /// empty or already-resolved transaction).
    NotStaged,
    /// The caller's identity for this operation doesn't match the identity
    /// the staged artifact was written under.
    IdentityMismatch,
    /// The current [`crate::state::TransactionState`] doesn't allow the
    /// requested transition -- a boot-chain/transaction anomaly, never
    /// papered over.
    NoTransition,
    /// [`crate::storage::ArtifactStorage::write`] or `finish` returned a
    /// durability watermark this crate does not trust: one that moved
    /// backward, or that claims more bytes durable than it was just
    /// offered. `old_durable <= new_durable <= old_durable +
    /// pending.len()` is checked on every call -- a backend is never taken
    /// on faith past that bound, release build or not (this is a real
    /// check, not a `debug_assert`, specifically because it would
    /// otherwise vanish from exactly the build this crate ships in).
    InvalidDurabilityReport,
}
