//! The Workload A/B state machine over the OTM2 store.
//!
//! Historical OTM2 activation/recovery policy; usable for supervised domains.
//! It reuses the sole common WriteSession and owns only persistence policy.
//! Selection only: it records which Workload is active, which candidate is
//! staged and which one to return to. It does not load, run or watch a
//! Workload -- execution effects remain in the application/runtime.
//!
//! | state | event | next | |
//! |---|---|---|---|
//! | Empty / Valid / Staged | `commit_staged` (after `prepare` + verified write) | Staged | allowed (a Staged candidate is superseded) |
//! | Activating / PendingConfirmation / RollingBack | `prepare` | – | refused `Busy` |
//! | Staged | `begin_activation` (runtime API satisfied) | Activating → PendingConfirmation | allowed |
//! | Staged | `begin_activation` (runtime API not satisfied) | Staged | refused `IncompatibleRuntimeApi` |
//! | PendingConfirmation | `confirm` | Valid | allowed |
//! | Activating / PendingConfirmation / RollingBack | `begin_rollback` | RollingBack → Valid or Empty | allowed |
//! | any other | `activate` / `confirm` / `begin_rollback` | – | refused |

use crate::compat::otm2::{RuntimeApi, Side};
use crate::{ArtifactStorage, Committed, Digest, WriteSession};

use crate::compat::otm2::{FieldError, Record, SlotMeta, State, slot_index};
use crate::compat::otm2_store::{Loaded, MetadataBackend, MetadataStore};

/// Refusals of the supervised activation policy, not another streaming engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    Busy,
    IncompatibleRuntimeApi,
}

/// Where a restarted Agent stands (no loader involved).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recovery {
    /// No Workload and no candidate.
    NoWorkload,
    Valid(Side),
    Staged {
        active: Option<Side>,
        candidate: Side,
    },
    /// The candidate was running unconfirmed: resume the health window or roll back.
    PendingConfirmation(Side),
    /// An interrupted activation or rollback: `rollback` must be completed
    /// (`restore` is the Workload to return to, `None` = stay empty).
    RollbackRequired {
        restore: Option<Side>,
    },
    /// Neither metadata copy is valid; no state is invented.
    CorruptedMetadata,
}

#[derive(Debug, PartialEq, Eq)]
pub enum UpdateError<E> {
    Backend(E),
    Refused(Refusal),
    /// The state does not allow this operation.
    WrongState(Option<State>),
    /// Metadata is corrupted: refuse to act until recovered explicitly.
    Corrupted,
    TooLarge,
    Field(FieldError),
    /// `commit_staged` was given a write result that is not the prepared one.
    NotPrepared,
}

/// A slot reserved for an incoming Workload, with the metadata that will be
/// persisted once the write is verified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prepared {
    slot: Side,
    meta: SlotMeta,
    active_at_prepare: Option<Side>,
}

impl Prepared {
    pub const fn slot(&self) -> Side {
        self.slot
    }
    pub const fn metadata(&self) -> &SlotMeta {
        &self.meta
    }

    /// A streaming session for this artifact (shared engine: digest while
    /// writing, resume, `finish` verifies size and SHA-256).
    pub fn session(&self) -> WriteSession {
        WriteSession::begin(u64::from(self.meta.size), Digest(self.meta.digest))
    }
}

pub struct SupervisedUpdate<B> {
    store: MetadataStore<B>,
}

impl<B: MetadataBackend> SupervisedUpdate<B> {
    pub fn new(backend: B) -> Self {
        Self {
            store: MetadataStore::new(backend),
        }
    }

    pub fn store(&self) -> &MetadataStore<B> {
        &self.store
    }

    pub fn store_mut(&mut self) -> &mut MetadataStore<B> {
        &mut self.store
    }

    /// The current record, `None` when nothing was ever stored.
    fn current(&mut self) -> Result<Option<Record>, UpdateError<B::Error>> {
        match self.store.load().map_err(UpdateError::Backend)? {
            Loaded::Blank => Ok(None),
            Loaded::Record { record, .. } => Ok(Some(record)),
            Loaded::Corrupted => Err(UpdateError::Corrupted),
        }
    }

    fn base(current: &Option<Record>) -> Record {
        current.unwrap_or(Record::empty(0))
    }

    fn put(
        &mut self,
        mut record: Record,
        previous_sequence: u32,
    ) -> Result<(), UpdateError<B::Error>> {
        record.sequence = previous_sequence.wrapping_add(1);
        self.store.commit(&record).map_err(UpdateError::Backend)
    }

    pub fn state(&mut self) -> Result<Option<State>, UpdateError<B::Error>> {
        Ok(self.current()?.map(|r| r.state))
    }

    pub fn record(&mut self) -> Result<Option<Record>, UpdateError<B::Error>> {
        self.current()
    }

    /// Explicit recovery from [`Recovery::CorruptedMetadata`] (or an OTM2 factory
    /// reset): commits an `Empty` record with a fresh sequence. The Agent chooses
    /// to call it; nothing does so silently.
    pub fn format(&mut self) -> Result<(), B::Error> {
        let sequence = match self.store.load()? {
            Loaded::Record { record, .. } => record.sequence.wrapping_add(1),
            _ => 1,
        };
        self.store.commit(&Record::empty(sequence))
    }

    /// What the Agent must do after a restart.
    pub fn recover(&mut self) -> Result<Recovery, B::Error> {
        Ok(match self.store.load()? {
            Loaded::Blank => Recovery::NoWorkload,
            Loaded::Corrupted => Recovery::CorruptedMetadata,
            Loaded::Record { record, .. } => match record.state {
                State::Empty => Recovery::NoWorkload,
                State::Valid => record
                    .active
                    .map(Recovery::Valid)
                    .unwrap_or(Recovery::CorruptedMetadata),
                State::Staged => match record.candidate {
                    Some(candidate) => Recovery::Staged {
                        active: record.active,
                        candidate,
                    },
                    None => Recovery::CorruptedMetadata,
                },
                State::PendingConfirmation => record
                    .active
                    .map(Recovery::PendingConfirmation)
                    .unwrap_or(Recovery::CorruptedMetadata),
                State::Activating => Recovery::RollbackRequired {
                    restore: record.active,
                },
                State::RollingBack => Recovery::RollbackRequired {
                    restore: record.previous_valid,
                },
            },
        })
    }

    /// Selects the inactive slot for the typed artifact metadata. Refused while an activation,
    /// a pending confirmation or a rollback is in flight; a `Staged` candidate
    /// is superseded.
    pub fn prepare(
        &mut self,
        meta: SlotMeta,
        capacity: u64,
    ) -> Result<Prepared, UpdateError<B::Error>> {
        let current = self.current()?;
        if let Some(record) = current
            && matches!(
                record.state,
                State::Activating | State::PendingConfirmation | State::RollingBack
            )
        {
            return Err(UpdateError::Refused(Refusal::Busy));
        }
        if u64::from(meta.size) > capacity || meta.size == 0 {
            return Err(UpdateError::TooLarge);
        }
        let active = Self::base(&current).active;
        Ok(Prepared {
            slot: active.map_or(Side::A, Side::other),
            meta,
            active_at_prepare: active,
        })
    }

    /// Must be called before the first byte of a new artifact is written to the
    /// inactive slot. A `Staged` candidate lives in exactly that slot, so it stops
    /// existing here (state back to `Valid`/`Empty`): a new upload that is aborted,
    /// incomplete or has the wrong digest can then never leave a `Staged` record
    /// pointing at overwritten bytes. In `Activating`/`PendingConfirmation`/
    /// `RollingBack` the inactive slot is the rollback target: refused.
    pub fn begin_overwrite(&mut self) -> Result<(), UpdateError<B::Error>> {
        let Some(record) = self.current()? else {
            return Ok(());
        };
        match record.state {
            State::Activating | State::PendingConfirmation | State::RollingBack => {
                Err(UpdateError::Refused(Refusal::Busy))
            }
            State::Empty | State::Valid => Ok(()),
            State::Staged => {
                let mut cleared = record;
                if let Some(candidate) = record.candidate {
                    cleared.meta[slot_index(candidate)] = SlotMeta::EMPTY;
                }
                cleared.candidate = None;
                cleared.previous_valid = None;
                cleared.state = if cleared.active.is_some() {
                    State::Valid
                } else {
                    State::Empty
                };
                self.put(cleared, record.sequence)
            }
        }
    }

    /// Persists `Staged` once the artifact is fully written and its digest
    /// verified (`committed` comes from the shared `WriteSession::finish`).
    pub fn commit_staged(
        &mut self,
        prepared: &Prepared,
        committed: &Committed,
    ) -> Result<(), UpdateError<B::Error>> {
        if committed.digest.0 != prepared.meta.digest
            || committed.size != u64::from(prepared.meta.size)
        {
            return Err(UpdateError::NotPrepared);
        }
        let current = self.current()?;
        let mut record = Self::base(&current);
        if matches!(
            record.state,
            State::Activating | State::PendingConfirmation | State::RollingBack
        ) {
            return Err(UpdateError::Refused(Refusal::Busy));
        }
        if record.active != prepared.active_at_prepare || record.active == Some(prepared.slot) {
            return Err(UpdateError::NotPrepared);
        }
        record.state = State::Staged;
        record.candidate = Some(prepared.slot);
        record.previous_valid = None;
        record.meta[slot_index(prepared.slot)] = prepared.meta;
        self.put(record, Self::base(&current).sequence)
    }

    /// The Workload requirement of the active slot, for the Agent to verify
    /// at boot / after its own update.
    pub fn active_requirement(&mut self) -> Result<Option<RuntimeApi>, UpdateError<B::Error>> {
        Ok(self
            .current()?
            .and_then(|r| r.active.map(|s| r.meta[slot_index(s)].requires)))
    }

    /// Everything activation checks **before** it persists anything or calls a
    /// supervisor, with no side effect: the state must be `Staged` and the
    /// candidate's runtime API requirement must be met. Returns the record.
    /// A caller with no runtime to call uses this and stops there, leaving
    /// the state `Staged` -- it never pretends an activation happened.
    pub fn preflight_activate(
        &mut self,
        agent_api: RuntimeApi,
    ) -> Result<Record, UpdateError<B::Error>> {
        let current = self.current()?;
        let Some(record) = current.filter(|r| r.state == State::Staged) else {
            return Err(UpdateError::WrongState(current.map(|r| r.state)));
        };
        let candidate = record.candidate.ok_or(UpdateError::Corrupted)?;
        if !agent_api.satisfies(record.meta[slot_index(candidate)].requires) {
            return Err(UpdateError::Refused(Refusal::IncompatibleRuntimeApi));
        }
        Ok(record)
    }

    /// Step 1 of an activation: every `preflight_activate` check, then `Staged ->
    /// Activating` is persisted. Nothing has been started yet; if power is lost here
    /// recovery reports `RollbackRequired`, never "the candidate started".
    /// Returns the `Staged` record as it was before the transition.
    pub fn begin_activation(
        &mut self,
        agent_api: RuntimeApi,
    ) -> Result<Record, UpdateError<B::Error>> {
        let record = self.preflight_activate(agent_api)?;
        let mut activating = record;
        activating.state = State::Activating;
        self.put(activating, record.sequence)?;
        Ok(record)
    }

    /// Step 2: the candidate really runs: `Activating -> PendingConfirmation`. The
    /// previously active Workload (if any) becomes the way back.
    pub fn complete_activation(&mut self) -> Result<(), UpdateError<B::Error>> {
        let current = self.current()?;
        let Some(record) = current.filter(|r| r.state == State::Activating) else {
            return Err(UpdateError::WrongState(current.map(|r| r.state)));
        };
        let candidate = record.candidate.ok_or(UpdateError::Corrupted)?;
        let mut pending = record;
        pending.state = State::PendingConfirmation;
        pending.previous_valid = record.active;
        pending.active = Some(candidate);
        pending.candidate = None;
        self.put(pending, record.sequence)
    }

    /// The trigger (health) is a future capability; this only records it.
    pub fn confirm(&mut self) -> Result<(), UpdateError<B::Error>> {
        let current = self.current()?;
        let Some(record) = current.filter(|r| r.state == State::PendingConfirmation) else {
            return Err(UpdateError::WrongState(current.map(|r| r.state)));
        };
        let mut valid = record;
        valid.state = State::Valid;
        valid.previous_valid = None;
        self.put(valid, record.sequence)
    }

    /// Step 1 of a rollback: persist `RollingBack` (from `Activating` or
    /// `PendingConfirmation`; already `RollingBack` = resume after a restart) and say
    /// what must be stopped and what restored. The previous Workload's slot and
    /// metadata are not touched.
    pub fn begin_rollback(&mut self) -> Result<RollbackPlan, UpdateError<B::Error>> {
        let current = self.current()?;
        let Some(record) = current.filter(|r| {
            matches!(
                r.state,
                State::Activating | State::PendingConfirmation | State::RollingBack
            )
        }) else {
            return Err(UpdateError::WrongState(current.map(|r| r.state)));
        };
        let (failed, restore) = match record.state {
            State::Activating => (record.candidate, record.active),
            _ => (record.active, record.previous_valid),
        };
        if record.state != State::RollingBack {
            let mut rolling = record;
            rolling.state = State::RollingBack;
            rolling.active = failed;
            rolling.candidate = None;
            rolling.previous_valid = restore;
            self.put(rolling, record.sequence)?;
        }
        Ok(RollbackPlan { failed, restore })
    }

    /// Step 2: the previous Workload is running again (or there is none):
    /// `RollingBack -> Valid(previous)` / `Empty`.
    pub fn complete_rollback(&mut self) -> Result<(), UpdateError<B::Error>> {
        let current = self.current()?;
        let Some(record) = current.filter(|r| r.state == State::RollingBack) else {
            return Err(UpdateError::WrongState(current.map(|r| r.state)));
        };
        let restore = record.previous_valid;
        let mut done = record;
        done.candidate = None;
        done.previous_valid = None;
        match restore {
            Some(side) => {
                done.state = State::Valid;
                done.active = Some(side);
            }
            None => {
                done.state = State::Empty;
                done.active = None;
            }
        }
        self.put(done, record.sequence)
    }
}

/// What a rollback has to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RollbackPlan {
    /// The slot that failed (to stop), if any.
    pub failed: Option<Side>,
    /// The slot to restore, `None` = nothing to return to (stay empty).
    pub restore: Option<Side>,
}

/// Convenience for callers/tests: write `data` to a slot through the shared
/// streaming engine and return the verified result.
pub fn write_all<S: ArtifactStorage>(
    prepared: &Prepared,
    storage: &mut S,
    data: &[u8],
    chunk: usize,
) -> Result<Committed, crate::Error<S::Error>> {
    let mut session = prepared.session();
    for piece in data.chunks(chunk.max(1)) {
        session.append(storage, piece)?;
    }
    session.finish(storage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compat::otm2::RECORD_LEN;
    #[derive(Clone)]
    struct Memory {
        copies: [[u8; RECORD_LEN]; 2],
        cut: Option<usize>,
    }
    impl Default for Memory {
        fn default() -> Self {
            Self {
                copies: [[0xff; RECORD_LEN]; 2],
                cut: None,
            }
        }
    }
    impl MetadataBackend for Memory {
        type Error = ();
        fn read(&mut self, copy: usize) -> Result<[u8; RECORD_LEN], ()> {
            Ok(self.copies[copy])
        }
        fn write(&mut self, copy: usize, data: &[u8; RECORD_LEN]) -> Result<(), ()> {
            self.copies[copy].fill(0xff);
            let len = self.cut.unwrap_or(RECORD_LEN).min(RECORD_LEN);
            self.copies[copy][..len].copy_from_slice(&data[..len]);
            if len < RECORD_LEN { Err(()) } else { Ok(()) }
        }
    }
    fn meta(byte: u8) -> SlotMeta {
        SlotMeta::new("component", "1", [byte; 32], 4096, RuntimeApi::new(1, 0)).unwrap()
    }
    fn stage(update: &mut SupervisedUpdate<Memory>, byte: u8) -> Prepared {
        let prepared = update.prepare(meta(byte), 0x40000).unwrap();
        update.begin_overwrite().unwrap();
        update
            .commit_staged(
                &prepared,
                &Committed {
                    size: 4096,
                    digest: Digest([byte; 32]),
                },
            )
            .unwrap();
        prepared
    }
    fn confirm_first(update: &mut SupervisedUpdate<Memory>) {
        stage(update, 1);
        update.begin_activation(RuntimeApi::new(1, 0)).unwrap();
        update.complete_activation().unwrap();
        update.confirm().unwrap();
    }
    #[test]
    fn first_install_and_confirmed_replacement_use_shared_engine_and_policy() {
        let mut update = SupervisedUpdate::new(Memory::default());
        assert_eq!(update.recover(), Ok(Recovery::NoWorkload));
        let prepared = stage(&mut update, 1);
        assert_eq!(prepared.slot(), Side::A);
        assert_eq!(prepared.session().received(), 0);
        assert_eq!(
            update.recover(),
            Ok(Recovery::Staged {
                active: None,
                candidate: Side::A
            })
        );
        update.begin_activation(RuntimeApi::new(1, 0)).unwrap();
        assert_eq!(
            update.recover(),
            Ok(Recovery::RollbackRequired { restore: None })
        );
        update.complete_activation().unwrap();
        assert_eq!(update.recover(), Ok(Recovery::PendingConfirmation(Side::A)));
        update.confirm().unwrap();
        assert_eq!(update.recover(), Ok(Recovery::Valid(Side::A)));
        assert_eq!(stage(&mut update, 2).slot(), Side::B);
    }
    #[test]
    fn interrupted_activation_or_probation_restores_previous_without_system_reboot() {
        for complete in [false, true] {
            let mut update = SupervisedUpdate::new(Memory::default());
            confirm_first(&mut update);
            stage(&mut update, 2);
            update.begin_activation(RuntimeApi::new(1, 0)).unwrap();
            if complete {
                update.complete_activation().unwrap();
            }
            assert_eq!(
                update.prepare(meta(3), 0x40000),
                Err(UpdateError::Refused(Refusal::Busy))
            );
            assert_eq!(
                update.begin_overwrite(),
                Err(UpdateError::Refused(Refusal::Busy))
            );
            let plan = update.begin_rollback().unwrap();
            assert_eq!(plan.restore, Some(Side::A));
            assert_eq!(
                update.recover(),
                Ok(Recovery::RollbackRequired {
                    restore: Some(Side::A)
                })
            );
            // Runtime restores the previous image between begin and complete.
            update.complete_rollback().unwrap();
            assert_eq!(update.recover(), Ok(Recovery::Valid(Side::A)));
        }
    }
    #[test]
    fn supersession_invalidates_staged_before_overwrite_and_failed_first_install_returns_empty() {
        let mut update = SupervisedUpdate::new(Memory::default());
        stage(&mut update, 1);
        update.begin_overwrite().unwrap();
        assert_eq!(update.recover(), Ok(Recovery::NoWorkload));
        stage(&mut update, 2);
        update.begin_activation(RuntimeApi::new(1, 0)).unwrap();
        assert_eq!(update.begin_rollback().unwrap().restore, None);
        update.complete_rollback().unwrap();
        assert_eq!(update.recover(), Ok(Recovery::NoWorkload));
    }
    #[test]
    fn compatibility_and_verified_result_are_checked_before_publication() {
        let mut update = SupervisedUpdate::new(Memory::default());
        let prepared = update.prepare(meta(1), 0x40000).unwrap();
        assert_eq!(
            update.commit_staged(
                &prepared,
                &Committed {
                    size: 4096,
                    digest: Digest([2; 32])
                }
            ),
            Err(UpdateError::NotPrepared)
        );
        assert_eq!(update.recover(), Ok(Recovery::NoWorkload));
        stage(&mut update, 1);
        assert_eq!(
            update.begin_activation(RuntimeApi::new(2, 0)),
            Err(UpdateError::Refused(Refusal::IncompatibleRuntimeApi))
        );
        assert_eq!(update.state(), Ok(Some(State::Staged)));
    }
    #[test]
    fn torn_intent_preserves_last_confirmed_record_for_every_byte_prefix() {
        let mut base = SupervisedUpdate::new(Memory::default());
        confirm_first(&mut base);
        let backend = base.store().backend().clone();
        for cut in 0..RECORD_LEN {
            let mut update = SupervisedUpdate::new(backend.clone());
            let prepared = update.prepare(meta(2), 0x40000).unwrap();
            update.store_mut().backend_mut().cut = Some(cut);
            assert_eq!(
                update.commit_staged(
                    &prepared,
                    &Committed {
                        size: 4096,
                        digest: Digest([2; 32])
                    }
                ),
                Err(UpdateError::Backend(()))
            );
            assert_eq!(update.recover(), Ok(Recovery::Valid(Side::A)));
        }
    }
    #[test]
    fn corrupt_metadata_never_invents_an_active_component() {
        let mut memory = Memory::default();
        memory.copies[0].fill(0);
        let mut update = SupervisedUpdate::new(memory);
        assert_eq!(update.recover(), Ok(Recovery::CorruptedMetadata));
        assert_eq!(
            update.prepare(meta(1), 0x40000),
            Err(UpdateError::Corrupted)
        );
        assert_eq!(
            update.begin_activation(RuntimeApi::new(1, 0)),
            Err(UpdateError::Corrupted)
        );
    }
}
