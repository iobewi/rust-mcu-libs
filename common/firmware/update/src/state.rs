//! The vocabulary [`crate::transaction::reconcile`] reasons over. Kept
//! separate from `transaction.rs`'s data (the record itself) and orchestration
//! (the functions) so the decision table can be read -- and tested -- as
//! pure enum arithmetic.

/// Where a staged transaction stands. `None` (as returned by
/// [`crate::storage::TransactionMetadata::load`]) means no transaction is
/// staged at all -- that's not a variant here on purpose: a caller matching
/// on `Option<TransactionState>` can't forget to handle "nothing staged" as
/// a distinct case the way it could forget one arm of a 4-variant enum.
///
/// `#[non_exhaustive]`: this API is not stable yet (see the crate doc
/// comment), and a backend that wants to keep a resolved transaction's
/// record around instead of clearing it (say, to retain the last confirmed
/// artifact's identity) will need a state this enum doesn't have yet.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionState {
    /// Every artifact is written and digest-verified. The backend has not
    /// been asked to activate this transaction yet.
    Staged,
    /// Activation has been recorded with the metadata store *and* handed to
    /// the backend (in that order -- see [`crate::transaction::activate`]).
    /// Not yet confirmed: a restart here must find its way back to a
    /// deterministic outcome via [`crate::transaction::reconcile`].
    Activating,
}

/// What the backend reports about whatever it is currently running or
/// about to run, independent of how it encodes that internally (a commit
/// word, a superblock, ...). This is deliberately coarser than a full A/B
/// slot-trust model: `reconcile` only needs to know whether there is an
/// unconfirmed candidate in play, and whether the currently-trusted state
/// is confirmed.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendOutcome {
    /// The backend is running (or about to run) an unconfirmed candidate --
    /// booted/loaded at most once, waiting on this engine to confirm or
    /// reject it.
    PendingConfirmation,
    /// The backend's current, trusted state is confirmed.
    Confirmed,
    /// Neither of the above (nothing pending, nothing this engine staged is
    /// currently relevant).
    Other,
}

/// What the caller of [`crate::transaction::reconcile`] must do.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Nothing staged, nothing to reconcile.
    Nothing,
    /// The backend has an unconfirmed candidate this engine's own staged
    /// transaction accounts for: run whatever confirmation procedure decides
    /// if it stands.
    AwaitConfirmation,
    /// The backend has an unconfirmed candidate that nothing staged here
    /// explains: never trust it, reject/roll it back.
    RollbackUnaccounted,
    /// The backend's state is confirmed, on the slot/target this engine's
    /// `Activating` transaction named, but the metadata commit that should
    /// have followed confirmation never completed (or never even started).
    /// Finish it: re-run whatever [`crate::transaction::finish`] does.
    FinishInterruptedActivation,
    /// The staged record no longer describes anything real (an activation
    /// that was aborted, or the backend fell back to a different target):
    /// forget it.
    ClearStale,
    /// `Staged` and waiting on an external call to activate: must survive
    /// as-is.
    KeepStaged,
}
