//! Boot-only binding to the existing portable confirmation gate.
//! The application arms TIMG0 after HAL/runtime init, before flash discovery.
//! This adapter exclusively borrows SystemBoot through boot reconciliation;
//! no activation/upload can borrow that owner during the gate.
#[cfg(any(test, feature = "esp32c3", feature = "esp32s3"))]
use iobewi_firmware_update::BackendOutcome;

/// Historical metadata classification: Pending takes precedence over Valid.
/// The adapter obtains these records together with the actual MMU observation.
#[cfg(any(test, feature = "esp32c3", feature = "esp32s3"))]
fn image_outcome(records: [iobewi_firmware_boot::Raw; 2]) -> BackendOutcome {
    use iobewi_firmware_boot::{has_entry_in_state, state};
    if has_entry_in_state(records, state::PENDING_VERIFY) {
        BackendOutcome::PendingConfirmation
    } else if has_entry_in_state(records, state::VALID) {
        BackendOutcome::Confirmed
    } else {
        BackendOutcome::Other
    }
}

#[cfg(any(test, feature = "esp32c3", feature = "esp32s3"))]
fn observed_outcome(
    records: [iobewi_firmware_boot::Raw; 2],
    running: u8,
) -> Option<BackendOutcome> {
    use iobewi_firmware_boot::{Decoded, decode, slot_of, state};
    let outcome = image_outcome(records);
    let expected = match outcome {
        BackendOutcome::PendingConfirmation => state::PENDING_VERIFY,
        BackendOutcome::Confirmed => state::VALID,
        _ => return None,
    };
    records.iter().any(|raw|matches!(decode(raw),Decoded::Ok(e) if e.state==expected && slot_of(e.seq,2)==running)).then_some(outcome)
}

#[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
mod esp {
    extern crate alloc;
    use crate::platform::{Error, SystemBoot};
    use alloc::string::String;
    use core::cell::Cell;
    use embassy_sync::{blocking_mutex::raw::NoopRawMutex, mutex::Mutex};
    use iobewi_esp_flash::SharedFlash;
    use iobewi_firmware_update::{
        BackendOutcome,
        service::boot::{BootSelfCheck, BootState, BootWatchdog},
    };

    /// Scoped boot gate, used on a single Embassy executor. The async mutex
    /// bridges the historical &self service traits to the exclusive &mut owner;
    /// it is not a second physical flash mutex. Self-check uses neither lock.
    pub struct BootRuntime<'a, C> {
        system: Mutex<NoopRawMutex, &'a mut SystemBoot>,
        flash: &'a SharedFlash,
        check: &'a C,
        slot: &'static str,
        outcome: Cell<BackendOutcome>,
    }
    impl<'a, C: BootSelfCheck> BootRuntime<'a, C> {
        /// Call with the watchdog already armed after HAL/TIMG0/runtime init.
        /// Snapshot is atomic under SharedFlash. Failure returns an error and
        /// leaves the watchdog armed; no unknown observation becomes Stable.
        /// While this adapter lives, use only its transitions on SystemBoot.
        pub async fn new(
            system: &'a mut SystemBoot,
            flash: &'a SharedFlash,
            check: &'a C,
        ) -> Result<Self, Error> {
            let (running, records) = system.snapshot(flash).await?;
            let outcome =
                super::observed_outcome(records, running.index()).ok_or(Error::Observation)?;
            let slot = if running.index() == 0 {
                crate::OTA0
            } else {
                crate::OTA1
            };
            Ok(Self {
                system: Mutex::new(system),
                flash,
                check,
                slot,
                outcome: Cell::new(outcome),
            })
        }
    }
    impl<C: BootSelfCheck> BootState for BootRuntime<'_, C> {
        async fn image_outcome(&self) -> BackendOutcome {
            self.outcome.get()
        }
        async fn booted_slot(&self) -> String {
            String::from(self.slot)
        }
        async fn confirm(&self) -> bool {
            let ok = self.system.lock().await.confirm(self.flash).await.is_ok();
            if ok {
                self.outcome.set(BackendOutcome::Confirmed);
            }
            ok
        }
        async fn reject(&self) -> bool {
            let ok = self.system.lock().await.reject(self.flash).await.is_ok();
            if ok {
                self.outcome.set(BackendOutcome::Other);
            }
            ok
        }
    }
    impl<C: BootSelfCheck> BootSelfCheck for BootRuntime<'_, C> {
        async fn self_check(&self) -> bool {
            self.check.self_check().await
        }
    }
    impl<C: BootSelfCheck> BootWatchdog for BootRuntime<'_, C> {
        fn watchdog_feed(&self) {
            iobewi_esp_watchdog::feed();
        }
        fn watchdog_disable(&self) {
            iobewi_esp_watchdog::disable();
        }
    }
}
#[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
pub use esp::BootRuntime;

#[cfg(test)]
mod tests {
    use super::*;
    use iobewi_firmware_boot::{BLANK, Entry, state};
    #[test]
    fn unknown_or_mismatched_boot_records_do_not_disarm_probation() {
        let valid = Entry::new(1, state::VALID).encode();
        let pending = Entry::new(2, state::PENDING_VERIFY).encode();
        assert_eq!(observed_outcome([BLANK; 2], 0), None);
        assert_eq!(observed_outcome([valid, BLANK], 1), None);
        assert_eq!(observed_outcome([valid, pending], 0), None);
        assert_eq!(
            observed_outcome([valid, pending], 1),
            Some(BackendOutcome::PendingConfirmation)
        );
        assert_eq!(
            observed_outcome([valid, Entry::new(2, state::ABORTED).encode()], 0),
            Some(BackendOutcome::Confirmed)
        );
    }
    #[test]
    fn classification_preserves_historical_precedence_and_reboot_outcome() {
        let valid = Entry::new(1, state::VALID).encode();
        let pending = Entry::new(2, state::PENDING_VERIFY).encode();
        assert_eq!(image_outcome([BLANK; 2]), BackendOutcome::Other);
        assert_eq!(image_outcome([valid, BLANK]), BackendOutcome::Confirmed);
        assert_eq!(
            image_outcome([valid, pending]),
            BackendOutcome::PendingConfirmation
        );
        assert_eq!(
            image_outcome([valid, Entry::new(2, state::ABORTED).encode()]),
            BackendOutcome::Confirmed
        );
        assert_eq!(
            image_outcome([valid, Entry::new(2, state::NEW).encode()]),
            BackendOutcome::Confirmed
        );
        assert_eq!(
            image_outcome([Entry::new(1, state::INVALID).encode(), BLANK]),
            BackendOutcome::Other
        );
    }
}
