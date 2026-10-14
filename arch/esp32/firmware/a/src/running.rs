//! Firmware A target selection using a platform-observed running slot.
//! The caller must obtain this identity from the running image/bootloader,
//! never infer it from the boot metadata alone.
use crate::{Layout, Partition, selection};
use iobewi_firmware_boot::{Raw, SECTOR_COUNT};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunningSlot {
    Zero,
    One,
}

impl RunningSlot {
    pub const fn from_boot_report(index: u8) -> Option<Self> {
        match index {
            0 => Some(Self::Zero),
            1 => Some(Self::One),
            _ => None,
        }
    }
    pub const fn index(self) -> u8 {
        match self {
            Self::Zero => 0,
            Self::One => 1,
        }
    }
}

/// Resolve a bootloader-reported image partition against the validated layout.
/// The observation itself must originate from the running platform, not
/// from otadata or the proposed update target.
pub fn from_running_partition(layout: &Layout, observed: Partition) -> Option<RunningSlot> {
    if layout.images[0] == layout.images[1] {
        return None;
    }
    match (observed == layout.images[0], observed == layout.images[1]) {
        (true, false) => Some(RunningSlot::Zero),
        (false, true) => Some(RunningSlot::One),
        _ => None,
    }
}

/// The boot report must be independently verified by the platform adapter.
/// Missing, invalid, or contradicting observations fail closed.
pub fn select_inactive(
    layout: &Layout,
    metadata: [Raw; SECTOR_COUNT],
    boot_report: Option<RunningSlot>,
) -> Result<Partition, selection::SelectionError> {
    selection::inactive(layout, metadata, boot_report.map(RunningSlot::index))
}

/// Reserve A only after the historical target selection agrees with the
/// observed mapped application. Never create an unowned stand-alone permit.
pub fn reserve_inactive<'a>(
    state: &'a mut iobewi_firmware_layout::session::DomainState,
    layout: &Layout,
    metadata: [Raw; SECTOR_COUNT],
    observed: Option<Partition>,
    flash_size: u32,
) -> Result<iobewi_firmware_layout::session::WriteGuard<'a>, ReservationError> {
    use iobewi_firmware_layout::{authorization::SlotLayout, session::ExecutionState};
    // The historical selector trusts PendingVerify for slot identity. An upload
    // during probation would target the confirmed fallback; reserve must refuse.
    if iobewi_firmware_boot::has_entry_in_state(
        metadata,
        iobewi_firmware_boot::state::PENDING_VERIFY,
    ) {
        return Err(ReservationError::PendingConfirmation);
    }
    use iobewi_firmware_slots::Slot;
    let running = observed.and_then(|p| from_running_partition(layout, p));
    let chosen = select_inactive(layout, metadata, running).map_err(ReservationError::Selection)?;
    let active =
        running
            .and_then(|r| Slot::from_index(r.index()))
            .ok_or(ReservationError::Selection(
                selection::SelectionError::UnknownRunningSlot,
            ))?;
    let target = active.other();
    if chosen != layout.images[usize::from(target.index())] {
        return Err(ReservationError::Selection(
            selection::SelectionError::InvalidTarget,
        ));
    }
    let checked =
        SlotLayout::for_a(layout, flash_size).map_err(|_| ReservationError::InvalidLayout)?;
    state.observe(ExecutionState::Running(active));
    state
        .reserve(&checked, target)
        .map_err(ReservationError::Session)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReservationError {
    PendingConfirmation,
    Selection(selection::SelectionError),
    InvalidLayout,
    Session(iobewi_firmware_layout::session::SessionError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use iobewi_firmware_boot::{BLANK, Entry, state};
    use iobewi_firmware_layout::session::DomainState;
    use iobewi_firmware_slots::Domain;
    fn layout() -> Layout {
        Layout {
            images: [
                Partition {
                    offset: 0x20000,
                    size: 0x180000,
                },
                Partition {
                    offset: 0x1a0000,
                    size: 0x180000,
                },
            ],
            otadata: Partition {
                offset: 0xd000,
                size: 0x2000,
            },
        }
    }
    #[test]
    fn only_the_exact_observed_image_unlocks_historical_target() {
        let layout = layout();
        let records = [Entry::new(1, state::VALID).encode(), BLANK];
        let mut owner = DomainState::new(Domain::A);
        for observed in [
            None,
            Some(layout.otadata),
            Some(layout.images[1]),
            Some(Partition {
                offset: layout.images[0].offset,
                size: 0x1000,
            }),
        ] {
            assert!(reserve_inactive(&mut owner, &layout, records, observed, 0x400000).is_err());
        }
        let mut guard = reserve_inactive(
            &mut owner,
            &layout,
            records,
            Some(layout.images[0]),
            0x400000,
        )
        .unwrap();
        assert_eq!(guard.permit().region().offset, layout.images[1].offset);
    }
    #[test]
    fn historical_bootloader_rollback_and_confirmation_select_the_other_image() {
        let layout = layout();
        let mut owner = DomainState::new(Domain::A);
        for (records, observed, target) in [
            (
                [
                    Entry::new(1, state::VALID).encode(),
                    Entry::new(2, state::VALID).encode(),
                ],
                1,
                0,
            ),
            (
                [
                    Entry::new(1, state::VALID).encode(),
                    Entry::new(2, state::ABORTED).encode(),
                ],
                0,
                1,
            ),
            (
                [
                    Entry::new(1, state::VALID).encode(),
                    Entry::new(2, state::INVALID).encode(),
                ],
                0,
                1,
            ),
        ] {
            let mut guard = reserve_inactive(
                &mut owner,
                &layout,
                records,
                Some(layout.images[observed]),
                0x400000,
            )
            .unwrap();
            assert_eq!(guard.permit().region().offset, layout.images[target].offset);
        }
    }
    #[test]
    fn blank_metadata_and_new_without_promotion_cannot_erase_running_image() {
        let layout = layout();
        let mut owner = DomainState::new(Domain::A);
        assert!(
            reserve_inactive(
                &mut owner,
                &layout,
                [BLANK; 2],
                Some(layout.images[0]),
                0x400000
            )
            .is_err()
        );
        assert!(
            reserve_inactive(
                &mut owner,
                &layout,
                [
                    Entry::new(1, state::VALID).encode(),
                    Entry::new(2, state::NEW).encode()
                ],
                Some(layout.images[1]),
                0x400000
            )
            .is_err()
        );
    }
    #[test]
    fn probation_never_authorizes_overwriting_the_confirmed_fallback() {
        let layout = layout();
        let mut owner = DomainState::new(Domain::A);
        let records = [
            Entry::new(1, state::VALID).encode(),
            Entry::new(2, state::PENDING_VERIFY).encode(),
        ];
        // Preserve the historical selector, but gate destructive operations.
        assert_eq!(
            select_inactive(&layout, records, Some(RunningSlot::One)),
            Ok(layout.images[0])
        );
        assert!(matches!(
            reserve_inactive(
                &mut owner,
                &layout,
                records,
                Some(layout.images[1]),
                0x400000
            ),
            Err(ReservationError::PendingConfirmation)
        ));
    }
}
