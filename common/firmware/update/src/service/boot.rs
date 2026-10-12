//! Boot reconciliation and confirmation gate independent of any SoC.

use alloc::string::String;

use embassy_futures::select::{Either, select};
use embassy_time::{Duration, Timer};

use crate::{Action, BackendOutcome};
use crate::metadata::{
    Metadata, MetadataStore, PendingCheckAction, boot_action, clear_staged,
    finish_validation, load_metadata, pending_check_action,
};

/// What the platform can tell and do about the boot chain. `BootOps` has
/// always bundled three distinct capabilities; they are named separately so
/// each can be classified and implemented where it belongs, and the bundle
/// stays a blanket implementation (the service functions keep one bound):
///
/// * [`BootState`] -- physical boot status and the durable EWBT writes
///   (`firmware/boot` logic executed by a platform storage adapter);
/// * [`BootSelfCheck`] -- the storage self-check at boot;
/// * [`BootWatchdog`] -- the pending-verify watchdog (primitive in a
///   watchdog driver; the service decides when to feed or disable it).
///
/// The service decides when to use them. The caller decides when its own
/// application is ready to validate.
#[allow(async_fn_in_trait)]
pub trait BootState {
    async fn image_outcome(&self) -> BackendOutcome;
    async fn booted_slot(&self) -> String;
    async fn confirm(&self) -> bool;
    async fn reject(&self) -> bool;
}

#[allow(async_fn_in_trait)]
pub trait BootSelfCheck {
    async fn self_check(&self) -> bool;
}

pub trait BootWatchdog {
    fn watchdog_feed(&self);
    fn watchdog_disable(&self);
}

/// Everything the boot gate needs from a platform: the three capabilities above.
pub trait BootOps: BootState + BootSelfCheck + BootWatchdog {}

impl<T: BootState + BootSelfCheck + BootWatchdog> BootOps for T {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootStatus { Stable, PendingVerify, Degraded, Rollback }

pub struct BootResult {
    pub status: BootStatus,
    pub image: BackendOutcome,
    pub slot: String,
    pub action: Action,
    pub storage_healthy: Option<bool>,
}

/// Reconcile the durable transaction with the image actually booted. A
/// rollback request rejects the candidate before asking the caller to reset.
pub async fn on_boot<S: MetadataStore, B: BootOps>(store: &S, boot: &B) -> BootResult {
    let staged = load_metadata(store).await.unwrap_or_else(|_| Metadata::default()).staged;
    let image = boot.image_outcome().await;
    let slot = boot.booted_slot().await;
    let action = boot_action(&staged, image, (!slot.is_empty()).then_some(slot.as_str()));
    let status = match action {
        Action::AwaitConfirmation => {
            boot.watchdog_feed();
            BootStatus::PendingVerify
        }
        Action::RollbackUnaccounted => {
            let _ = boot.reject().await;
            BootStatus::Rollback
        }
        Action::ClearStale => {
            let _ = clear_staged(store).await;
            boot.watchdog_disable();
            BootStatus::Stable
        }
        Action::FinishInterruptedActivation => {
            let outcome = finish_validation(store, &staged).await;
            boot.watchdog_disable();
            if outcome.is_err() { BootStatus::Degraded } else { BootStatus::Stable }
        }
        _ => {
            boot.watchdog_disable();
            BootStatus::Stable
        }
    };
    // A failed metadata commit after confirmation is retried on the next
    // boot. Do not claim full health or run another self-check in that case.
    let storage_healthy = if status == BootStatus::Stable {
        Some(boot.self_check().await)
    } else { None };
    BootResult { status, image, slot, action, storage_healthy }
}

/// Race the storage check against a bounded deadline. A timeout leaves the
/// boot entry Pending so that a reset causes the bootloader to roll back.
pub async fn pending_check<B: BootOps>(boot: &B, timeout: Duration) -> PendingCheckAction {
    let result = match select(boot.self_check(), Timer::after(timeout)).await {
        Either::First(passed) => Some(passed),
        Either::Second(()) => None,
    };
    pending_check_action(result)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confirmation { Valid, Degraded, ResetRequired }

/// Call only after `pending_check` returned `Confirm`. The watchdog remains
/// armed until the boot entry has been durably confirmed by the adapter.
pub async fn confirm_pending<S: MetadataStore, B: BootOps>(store: &S, boot: &B) -> Confirmation {
    let staged = load_metadata(store).await.unwrap_or_else(|_| Metadata::default()).staged;
    if !boot.confirm().await {
        let _ = boot.reject().await;
        return Confirmation::ResetRequired;
    }
    boot.watchdog_disable();
    if finish_validation(store, &staged).await.is_err() {
        Confirmation::Degraded
    } else {
        Confirmation::Valid
    }
}

pub async fn reject_pending<B: BootOps>(boot: &B) {
    let _ = boot.reject().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::{MetadataStore, Stage, Staged, save_metadata};
    use alloc::vec::Vec;
    use core::cell::{Cell, RefCell};
    use core::future::Future;
    use core::task::{Context, Poll, Waker};

    fn block_on<F: Future>(future: F) -> F::Output {
        let mut future = core::pin::pin!(future);
        let mut cx = Context::from_waker(Waker::noop());
        loop {
            if let Poll::Ready(v) = future.as_mut().poll(&mut cx) {
                return v;
            }
            std::thread::yield_now();
        }
    }

    struct MemoryStore(RefCell<Option<Vec<u8>>>);

    impl MetadataStore for MemoryStore {
        type Error = ();
        async fn load_raw(&self) -> Result<Option<Vec<u8>>, ()> { Ok(self.0.borrow().clone()) }
        async fn commit_raw(&self, bytes: &[u8]) -> Result<(), ()> {
            *self.0.borrow_mut() = Some(bytes.to_vec());
            Ok(())
        }
    }

    /// Scripted platform: what the boot chain reports, and a record of what the
    /// gate asked it to do. Implements the three capabilities separately --
    /// `BootOps` comes from the blanket impl.
    struct FakeBoot {
        outcome: BackendOutcome,
        slot: &'static str,
        confirm_ok: bool,
        self_check: Option<bool>, // None = never completes (a hung check)
        confirms: Cell<u32>,
        rejects: Cell<u32>,
        feeds: Cell<u32>,
        disables: Cell<u32>,
        self_checks: Cell<u32>,
    }

    impl FakeBoot {
        fn new(outcome: BackendOutcome, slot: &'static str) -> Self {
            Self {
                outcome, slot, confirm_ok: true, self_check: Some(true),
                confirms: Cell::new(0), rejects: Cell::new(0), feeds: Cell::new(0),
                disables: Cell::new(0), self_checks: Cell::new(0),
            }
        }
    }

    impl BootState for FakeBoot {
        async fn image_outcome(&self) -> BackendOutcome { self.outcome }
        async fn booted_slot(&self) -> String { String::from(self.slot) }
        async fn confirm(&self) -> bool { self.confirms.set(self.confirms.get() + 1); self.confirm_ok }
        async fn reject(&self) -> bool { self.rejects.set(self.rejects.get() + 1); true }
    }

    impl BootSelfCheck for FakeBoot {
        async fn self_check(&self) -> bool {
            self.self_checks.set(self.self_checks.get() + 1);
            match self.self_check {
                Some(v) => v,
                None => core::future::pending().await,
            }
        }
    }

    impl BootWatchdog for FakeBoot {
        fn watchdog_feed(&self) { self.feeds.set(self.feeds.get() + 1); }
        fn watchdog_disable(&self) { self.disables.set(self.disables.get() + 1); }
    }

    fn store_with(stage: Stage, slot: &str) -> MemoryStore {
        let store = MemoryStore(RefCell::new(None));
        let mut metadata = Metadata::default();
        metadata.set_staged(Staged {
            stage, slot: String::from(slot), digest: String::from("sha256:aa"), deployment_id: String::from("d1"), size: 1,
        });
        block_on(save_metadata(&store, &metadata)).unwrap();
        store
    }

    #[test]
    fn nothing_staged_and_confirmed_image_is_stable_and_self_checked() {
        let store = MemoryStore(RefCell::new(None));
        let boot = FakeBoot::new(BackendOutcome::Confirmed, "ota_0");
        let result = block_on(on_boot(&store, &boot));
        assert_eq!((result.status, result.action), (BootStatus::Stable, Action::Nothing));
        assert_eq!(result.storage_healthy, Some(true));
        assert_eq!((boot.disables.get(), boot.feeds.get(), boot.rejects.get()), (1, 0, 0));
    }

    #[test]
    fn pending_candidate_named_by_the_activating_record_awaits_confirmation_with_the_watchdog_fed() {
        let store = store_with(Stage::Activating, "ota_1");
        let boot = FakeBoot::new(BackendOutcome::PendingConfirmation, "ota_1");
        let result = block_on(on_boot(&store, &boot));
        assert_eq!((result.status, result.action), (BootStatus::PendingVerify, Action::AwaitConfirmation));
        assert_eq!(result.storage_healthy, None, "no self-check before the candidate is confirmed");
        assert_eq!((boot.feeds.get(), boot.disables.get(), boot.rejects.get(), boot.self_checks.get()), (1, 0, 0, 0));
    }

    #[test]
    fn unaccounted_pending_candidate_is_rejected_for_rollback() {
        // Pending image booted from a slot nothing staged explains.
        let store = store_with(Stage::Activating, "ota_0");
        let boot = FakeBoot::new(BackendOutcome::PendingConfirmation, "ota_1");
        let result = block_on(on_boot(&store, &boot));
        assert_eq!((result.status, result.action), (BootStatus::Rollback, Action::RollbackUnaccounted));
        assert_eq!(boot.rejects.get(), 1);
        assert_eq!(boot.confirms.get(), 0, "an unexplained candidate is never confirmed");

        let none = MemoryStore(RefCell::new(None));
        let boot = FakeBoot::new(BackendOutcome::PendingConfirmation, "ota_0");
        assert_eq!(block_on(on_boot(&none, &boot)).status, BootStatus::Rollback);
    }

    #[test]
    fn confirmed_image_on_the_staged_slot_finishes_the_interrupted_activation_exactly_once() {
        let store = store_with(Stage::Activating, "ota_1");
        let boot = FakeBoot::new(BackendOutcome::Confirmed, "ota_1");
        let result = block_on(on_boot(&store, &boot));
        assert_eq!((result.status, result.action), (BootStatus::Stable, Action::FinishInterruptedActivation));
        // The staged record is cleared and the staged identity became active.
        let metadata = block_on(load_metadata(&store)).unwrap();
        assert_eq!(metadata.staged.stage.as_str(), "none");
        assert_eq!(metadata.active_deployment_id, "d1");
        // A second boot with the same facts has nothing left to finish.
        let again = block_on(on_boot(&store, &boot));
        assert_eq!(again.action, Action::Nothing);
    }

    #[test]
    fn staged_record_for_the_running_slot_is_cleared_and_a_foreign_one_is_kept() {
        let store = store_with(Stage::Written, "ota_1");
        let boot = FakeBoot::new(BackendOutcome::Confirmed, "ota_1");
        assert_eq!(block_on(on_boot(&store, &boot)).action, Action::ClearStale);
        assert_eq!(block_on(load_metadata(&store)).unwrap().staged.stage.as_str(), "none");

        let store = store_with(Stage::Written, "ota_1");
        let boot = FakeBoot::new(BackendOutcome::Confirmed, "ota_0");
        assert_eq!(block_on(on_boot(&store, &boot)).action, Action::KeepStaged);
        assert_eq!(block_on(load_metadata(&store)).unwrap().staged.stage.as_str(), "written");
    }

    #[test]
    fn an_unknown_booted_slot_never_triggers_a_destructive_decision() {
        let store = store_with(Stage::Activating, "ota_1");
        let boot = FakeBoot::new(BackendOutcome::Confirmed, "");
        let result = block_on(on_boot(&store, &boot));
        assert_eq!(result.action, Action::Nothing);
        assert_eq!(boot.rejects.get(), 0);
        assert_eq!(block_on(load_metadata(&store)).unwrap().staged.stage.as_str(), "activating");
    }

    #[test]
    fn pending_check_confirms_rejects_or_times_out_into_a_reset() {
        let mut boot = FakeBoot::new(BackendOutcome::PendingConfirmation, "ota_1");
        assert_eq!(block_on(pending_check(&boot, Duration::from_millis(500))), PendingCheckAction::Confirm);
        boot.self_check = Some(false);
        assert_eq!(block_on(pending_check(&boot, Duration::from_millis(500))), PendingCheckAction::Reject);
        boot.self_check = None; // a hung check must not hold the candidate forever
        assert_eq!(block_on(pending_check(&boot, Duration::from_millis(50))), PendingCheckAction::Reset);
        assert_eq!(boot.confirms.get() + boot.rejects.get(), 0, "the check itself never writes boot state");
    }

    #[test]
    fn confirmation_commits_the_candidate_or_demands_a_reset_after_rejecting() {
        let store = store_with(Stage::Activating, "ota_1");
        let boot = FakeBoot::new(BackendOutcome::PendingConfirmation, "ota_1");
        assert_eq!(block_on(confirm_pending(&store, &boot)), Confirmation::Valid);
        assert_eq!(boot.confirms.get(), 1);
        assert_eq!(block_on(load_metadata(&store)).unwrap().staged.stage.as_str(), "none");
        assert_eq!(boot.disables.get(), 1, "the watchdog is released only after the durable confirm");

        let store = store_with(Stage::Activating, "ota_1");
        let mut boot = FakeBoot::new(BackendOutcome::PendingConfirmation, "ota_1");
        boot.confirm_ok = false;
        assert_eq!(block_on(confirm_pending(&store, &boot)), Confirmation::ResetRequired);
        assert_eq!((boot.confirms.get(), boot.rejects.get(), boot.disables.get()), (1, 1, 0));
        assert_eq!(block_on(load_metadata(&store)).unwrap().staged.stage.as_str(), "activating", "metadata untouched when the confirm failed");
    }

    #[test]
    fn reject_pending_only_rejects() {
        let boot = FakeBoot::new(BackendOutcome::PendingConfirmation, "ota_1");
        block_on(reject_pending(&boot));
        assert_eq!((boot.rejects.get(), boot.confirms.get()), (1, 0));
    }
}
