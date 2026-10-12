//! The staged-transaction record, and the two things this crate does with
//! it: reconcile it against what the backend reports after a restart
//! ([`reconcile`]), and drive it through activation/confirmation
//! ([`activate`], [`finish`]).
//!
//! Three type parameters, left generic on purpose (see the crate doc
//! comment), and deliberately not collapsed into one:
//!
//! * `TxId` names the *transaction* -- a deployment/release identifier, in
//!   a caller's own protocol vocabulary (a string, a UUID, ...). This is
//!   what [`activate`] checks against.
//! * `ArtifactId` names one artifact *within* a transaction (a kind:
//!   firmware, a workload, ...) -- never the same thing as `TxId`, even in
//!   v1 where a transaction holds exactly one artifact: a transaction
//!   naming itself "release-42" does not make its one firmware artifact
//!   *be* "release-42", and a future second artifact in that same
//!   transaction needs its own identity that isn't a duplicate of the
//!   transaction's.
//! * `Target` is whatever the backend needs to know where an artifact goes
//!   (a slot label, a partition index, ...).
//!
//! This crate compares `TxId`s (in `activate`) and otherwise never
//! interprets any of the three.

use alloc::vec::Vec;

use crate::artifact::Digest;
use crate::error::Error;
use crate::state::{Action, BackendOutcome, TransactionState};
use crate::storage::TransactionMetadata;

/// One artifact within a transaction: its own identity (never the
/// transaction's -- see the module doc comment), size and digest (checked
/// by [`crate::artifact::WriteSession::finish`] before this record is ever
/// built), and whatever the backend needs to place it (`target`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactRecord<ArtifactId, Target> {
    pub id: ArtifactId,
    pub size: u64,
    pub digest: Digest,
    pub target: Target,
}

/// The whole staged transaction: its own identity (`id`), and `artifacts`
/// -- a `Vec`, not a single field, specifically so a multi-artifact
/// transaction is a matter of pushing more entries, not a breaking change
/// to this type. Every function in this module that only handles one
/// artifact today says so in its own doc comment, not by the shape of this
/// struct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionRecord<TxId, ArtifactId, Target> {
    pub id: TxId,
    pub state: TransactionState,
    pub artifacts: Vec<ArtifactRecord<ArtifactId, Target>>,
}

impl<TxId: Clone, ArtifactId: Clone, Target: Clone> TransactionRecord<TxId, ArtifactId, Target> {
    /// A freshly staged transaction `id` of exactly one artifact -- the
    /// only shape this crate's v1 orchestration ([`activate`]) accepts; a
    /// future multi-artifact caller builds `artifacts` directly instead.
    pub fn staged(id: TxId, artifact: ArtifactRecord<ArtifactId, Target>) -> Self {
        TransactionRecord { id, state: TransactionState::Staged, artifacts: alloc::vec![artifact] }
    }

    /// The same transaction, with its state replaced -- e.g. for a caller
    /// reverting a returned `Activating` record back to `Staged` after its
    /// own backend switch fails (see [`activate`]'s doc comment). Every
    /// field here is already `pub`, so this is equivalent to a struct-update
    /// literal (`TransactionRecord { state: ..., ..record }`); it exists
    /// only for that call to read as intent rather than field-copying.
    pub fn with_state(&self, state: TransactionState) -> Self {
        TransactionRecord { id: self.id.clone(), state, artifacts: self.artifacts.clone() }
    }
}

/// The `on_boot`/reconnect-time decision table: given what's staged (`None`
/// if nothing is), what the backend reports, and whether the
/// currently-running/selected target is the one the staged transaction
/// names, what must happen.
///
/// Ported from `embewi-agent-esp`'s `ota_logic::boot_action`, unchanged in
/// shape -- only the vocabulary is generic now. `running_matches_staged` is
/// `None` when that comparison itself couldn't be made (the running
/// target couldn't be determined): nothing destructive is ever decided in
/// that case, on purpose -- a flaky read must never roll back a good image.
pub fn reconcile(staged: Option<TransactionState>, outcome: BackendOutcome, running_matches_staged: Option<bool>) -> Action {
    use Action::*;
    match (outcome, running_matches_staged) {
        (BackendOutcome::PendingConfirmation, None) => AwaitConfirmation,
        (BackendOutcome::PendingConfirmation, Some(true)) if staged == Some(TransactionState::Activating) => {
            AwaitConfirmation
        }
        (BackendOutcome::PendingConfirmation, _) => RollbackUnaccounted,
        (_, None) => Nothing,
        (_, Some(same)) => match staged {
            None => Nothing,
            Some(TransactionState::Staged) if same => ClearStale,
            Some(TransactionState::Staged) => KeepStaged,
            Some(TransactionState::Activating) if same && outcome == BackendOutcome::Confirmed => {
                FinishInterruptedActivation
            }
            Some(TransactionState::Activating) => ClearStale,
        },
    }
}

/// Records the intent to activate the staged transaction: checks it is
/// actually `Staged`, that `expected_id` names *the transaction* staged (a
/// caller doesn't get to activate a different deployment than the one it
/// asked to write -- this compares `TxId`, never an artifact's own id),
/// and durably commits the transition to `Activating` *before* returning.
///
/// This only records intent with the metadata store -- it does not touch
/// whatever backend actually switches the running target (that's outside
/// this crate; see the crate doc comment). The caller is expected to:
///
/// 1. call this first;
/// 2. only on `Ok`, tell its own backend to switch to the named target;
/// 3. on a backend failure, best-effort `metadata.commit(Some(&record))`
///    back to the returned record's pre-activation form (`with_state
///    (Staged)`) so a retry stays possible -- this crate does not do that
///    automatically, since "best effort" here means the caller's own
///    failure-logging/degraded-state policy, which this crate has no
///    opinion on.
pub fn activate<M, TxId, ArtifactId, Target>(
    metadata: &mut M,
    expected_id: &TxId,
) -> Result<TransactionRecord<TxId, ArtifactId, Target>, Error<M::Error>>
where
    M: TransactionMetadata<Record = TransactionRecord<TxId, ArtifactId, Target>>,
    TxId: Clone + PartialEq,
    ArtifactId: Clone,
    Target: Clone,
{
    let record = metadata.load().map_err(Error::Backend)?.ok_or(Error::NotStaged)?;
    if record.state != TransactionState::Staged {
        return Err(Error::NotStaged);
    }
    if record.artifacts.is_empty() {
        return Err(Error::NotStaged);
    }
    if &record.id != expected_id {
        return Err(Error::IdentityMismatch);
    }
    let activating = record.with_state(TransactionState::Activating);
    metadata.commit(Some(&activating)).map_err(Error::Backend)?;
    Ok(activating)
}

/// Clears the staged transaction once its activation is confirmed. Call
/// this only after the backend's own confirmation has *durably* succeeded
/// (outside this crate) -- never speculatively: a transaction cleared here
/// is gone, there is no undo.
pub fn finish<M, TxId, ArtifactId, Target>(metadata: &mut M) -> Result<(), Error<M::Error>>
where
    M: TransactionMetadata<Record = TransactionRecord<TxId, ArtifactId, Target>>,
{
    metadata.commit(None).map_err(Error::Backend)
}

/// Clears a stale staged transaction (an aborted activation, or a record
/// left over on the slot that's now confirmed running some other way) --
/// same effect as [`finish`], named separately because the caller's reason
/// for calling it (an [`Action::ClearStale`]) is not "we confirmed this".
pub fn clear_stale<M, TxId, ArtifactId, Target>(metadata: &mut M) -> Result<(), Error<M::Error>>
where
    M: TransactionMetadata<Record = TransactionRecord<TxId, ArtifactId, Target>>,
{
    metadata.commit(None).map_err(Error::Backend)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact::Digest;
    use alloc::string::String;
    use alloc::string::ToString;

    fn digest(byte: u8) -> Digest {
        Digest([byte; 32])
    }

    /// An artifact record, deliberately with an `id` ("firmware") that
    /// never looks like a transaction id ("dep-1", "dep-2", ...) below --
    /// proving `activate`'s identity check is against `TransactionRecord::id`,
    /// never against an artifact's own id.
    fn firmware() -> ArtifactRecord<String, String> {
        ArtifactRecord { id: "firmware".to_string(), size: 100, digest: digest(0xAB), target: "slot-b".to_string() }
    }

    fn staged(tx_id: &str) -> TransactionRecord<String, String, String> {
        TransactionRecord::staged(tx_id.to_string(), firmware())
    }

    struct MockMetadata {
        record: Option<TransactionRecord<String, String, String>>,
        fail_commit: bool,
    }

    impl TransactionMetadata for MockMetadata {
        type Error = &'static str;
        type Record = TransactionRecord<String, String, String>;

        fn load(&mut self) -> Result<Option<Self::Record>, Self::Error> {
            Ok(self.record.clone())
        }

        fn commit(&mut self, record: Option<&Self::Record>) -> Result<(), Self::Error> {
            if self.fail_commit {
                return Err("simulated backend failure");
            }
            self.record = record.cloned();
            Ok(())
        }
    }

    #[test]
    fn activate_refuses_with_nothing_staged() {
        let mut meta = MockMetadata { record: None, fail_commit: false };
        assert_eq!(activate(&mut meta, &"dep-1".to_string()), Err(Error::NotStaged));
    }

    #[test]
    fn activate_refuses_a_mismatched_identity_and_leaves_the_record_untouched() {
        let record = staged("dep-1");
        let mut meta = MockMetadata { record: Some(record.clone()), fail_commit: false };
        assert_eq!(activate(&mut meta, &"dep-2".to_string()), Err(Error::IdentityMismatch));
        // A refused activation must not have touched the record.
        assert_eq!(meta.load().unwrap(), Some(record));
    }

    #[test]
    fn activate_commits_the_transition_and_returns_it() {
        let mut meta = MockMetadata { record: Some(staged("dep-1")), fail_commit: false };
        let activating = activate(&mut meta, &"dep-1".to_string()).unwrap();
        assert_eq!(activating.state, TransactionState::Activating);
        assert_eq!(activating.id, "dep-1");
        assert_eq!(meta.load().unwrap().unwrap().state, TransactionState::Activating);
    }

    #[test]
    fn activate_refuses_an_already_activating_transaction() {
        let mut record = staged("dep-1");
        record.state = TransactionState::Activating;
        let mut meta = MockMetadata { record: Some(record), fail_commit: false };
        assert_eq!(activate(&mut meta, &"dep-1".to_string()), Err(Error::NotStaged));
    }

    #[test]
    fn activate_propagates_a_backend_commit_failure_without_changing_the_record() {
        let record = staged("dep-1");
        let mut meta = MockMetadata { record: Some(record.clone()), fail_commit: true };
        assert_eq!(activate(&mut meta, &"dep-1".to_string()), Err(Error::Backend("simulated backend failure")));
        assert_eq!(meta.load().unwrap(), Some(record));
    }

    #[test]
    fn caller_can_revert_activation_after_its_own_backend_switch_fails() {
        // Models what a real caller does: `activate` commits `Activating`,
        // the caller's own backend switch then fails, and the caller
        // reverts the metadata record to its pre-activation form so a retry
        // stays possible -- this crate provides the building block, not the
        // policy of when to use it.
        let record = staged("dep-1");
        let mut meta = MockMetadata { record: Some(record.clone()), fail_commit: false };
        let activating = activate(&mut meta, &"dep-1".to_string()).unwrap();
        assert_eq!(meta.load().unwrap().unwrap().state, TransactionState::Activating);

        let reverted = activating.with_state(TransactionState::Staged);
        meta.commit(Some(&reverted)).unwrap();
        assert_eq!(meta.load().unwrap(), Some(record));
    }

    #[test]
    fn finish_clears_the_record() {
        let mut record = staged("dep-1");
        record.state = TransactionState::Activating;
        let mut meta = MockMetadata { record: Some(record), fail_commit: false };
        finish(&mut meta).unwrap();
        assert_eq!(meta.load().unwrap(), None);
    }

    #[test]
    fn clear_stale_also_clears_the_record() {
        let mut meta = MockMetadata { record: Some(staged("dep-1")), fail_commit: false };
        clear_stale(&mut meta).unwrap();
        assert_eq!(meta.load().unwrap(), None);
    }

    use Action::*;
    use BackendOutcome as O;
    use TransactionState as S;

    #[test]
    fn staged_survives_a_reboot_on_a_different_target() {
        assert_eq!(reconcile(Some(S::Staged), O::Other, Some(false)), KeepStaged);
        assert_eq!(reconcile(Some(S::Staged), O::Confirmed, Some(false)), KeepStaged);
    }

    #[test]
    fn staged_on_the_now_running_target_is_stale() {
        assert_eq!(reconcile(Some(S::Staged), O::Confirmed, Some(true)), ClearStale);
    }

    #[test]
    fn activating_pending_on_the_same_target_awaits_confirmation() {
        assert_eq!(reconcile(Some(S::Activating), O::PendingConfirmation, Some(true)), AwaitConfirmation);
    }

    #[test]
    fn activating_confirmed_on_the_same_target_finishes_the_interrupted_activation() {
        assert_eq!(reconcile(Some(S::Activating), O::Confirmed, Some(true)), FinishInterruptedActivation);
    }

    #[test]
    fn activating_on_another_target_is_an_aborted_activation() {
        assert_eq!(reconcile(Some(S::Activating), O::Confirmed, Some(false)), ClearStale);
        assert_eq!(reconcile(Some(S::Activating), O::Other, Some(false)), ClearStale);
        assert_eq!(reconcile(Some(S::Activating), O::Other, Some(true)), ClearStale);
    }

    #[test]
    fn a_pending_candidate_nothing_staged_explains_is_rolled_back() {
        assert_eq!(reconcile(None, O::PendingConfirmation, Some(true)), RollbackUnaccounted);
        assert_eq!(reconcile(None, O::PendingConfirmation, Some(false)), RollbackUnaccounted);
        assert_eq!(reconcile(Some(S::Staged), O::PendingConfirmation, Some(true)), RollbackUnaccounted);
        assert_eq!(reconcile(Some(S::Activating), O::PendingConfirmation, Some(false)), RollbackUnaccounted);
    }

    #[test]
    fn an_undetermined_running_target_never_decides_anything_destructive() {
        assert_eq!(reconcile(Some(S::Activating), O::PendingConfirmation, None), AwaitConfirmation);
        assert_eq!(reconcile(None, O::PendingConfirmation, None), AwaitConfirmation);
        assert_eq!(reconcile(Some(S::Staged), O::Confirmed, None), Nothing);
        assert_eq!(reconcile(Some(S::Activating), O::Confirmed, None), Nothing);
    }

    #[test]
    fn nothing_staged_does_nothing() {
        assert_eq!(reconcile(None, O::Confirmed, Some(true)), Nothing);
        assert_eq!(reconcile(None, O::Other, Some(false)), Nothing);
    }
}
