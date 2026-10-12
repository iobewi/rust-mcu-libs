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
pub fn from_running_partition(
    layout: &Layout,
    observed: Partition,
) -> Option<RunningSlot> {
    if layout.images[0] == layout.images[1] {
        return None;
    }
    match (
        observed == layout.images[0],
        observed == layout.images[1],
    ) {
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

/// Combine A's historical transactional selection with the common write gate.
/// The boot report MUST be independently obtained; this adapter does not
/// observe running hardware. Until such an observer exists, callers must fail
/// closed rather than deriving a report from otadata.
pub fn authorize_inactive(
    layout: &Layout,
    metadata: [Raw; SECTOR_COUNT],
    boot_report: Option<RunningSlot>,
) -> Result<iobewi_firmware_layout::authorization::InactiveWriteTarget, AuthorizedSelectionError> {
    use iobewi_firmware_layout::authorization::{AuthorizationError, authorize_a};
    use iobewi_firmware_slots::{Domain, FirmwareSlot, Slot};

    let chosen = select_inactive(layout, metadata, boot_report)
        .map_err(AuthorizedSelectionError::Selection)?;
    let running = boot_report.ok_or(AuthorizedSelectionError::Authorization(
        AuthorizationError::UnknownRunning,
    ))?;
    let current = match running {
        RunningSlot::Zero => Slot::Zero,
        RunningSlot::One => Slot::One,
    };
    let target = current.other();
    let validated = iobewi_firmware_layout::domain_a::Layout {
        images: layout
            .images
            .map(|p| iobewi_firmware_layout::domain_a::Partition {
                offset: p.offset,
                size: p.size,
            }),
        otadata: iobewi_firmware_layout::domain_a::Partition {
            offset: layout.otadata.offset,
            size: layout.otadata.size,
        },
    };
    let permission = authorize_a(
        &validated,
        Some(FirmwareSlot::new(Domain::A, current)),
        FirmwareSlot::new(Domain::A, target),
    )
    .map_err(AuthorizedSelectionError::Authorization)?;
    if permission.region().offset != chosen.offset
        || permission.region().size != chosen.size as usize
    {
        return Err(AuthorizedSelectionError::Selection(
            selection::SelectionError::InvalidTarget,
        ));
    }
    Ok(permission)
}

#[derive(Debug, PartialEq, Eq)]
pub enum AuthorizedSelectionError {
    Selection(selection::SelectionError),
    Authorization(iobewi_firmware_layout::authorization::AuthorizationError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use iobewi_firmware_boot::{BLANK, Entry, state};

    #[test]
    fn reported_partition_must_match_exactly_one_slot() {
        let layout = Layout {
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
        };
        assert_eq!(
            from_running_partition(&layout, layout.images[0]),
            Some(RunningSlot::Zero)
        );
        assert_eq!(
            from_running_partition(&layout, layout.images[1]),
            Some(RunningSlot::One)
        );
        assert_eq!(
            from_running_partition(
                &layout,
                Partition {
                    offset: 0x20000,
                    size: 0x1000,
                },
            ),
            None
        );
        let mut ambiguous = layout;
        ambiguous.images[1] = ambiguous.images[0];
        assert_eq!(
            from_running_partition(&ambiguous, ambiguous.images[0]),
            None
        );
    }

    #[test]
    fn authorization_requires_observed_report_and_matching_metadata() {
        let layout = Layout {
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
        };
        let records = [Entry::new(1, state::VALID).encode(), BLANK];
        assert_eq!(
            authorize_inactive(&layout, records, None),
            Err(AuthorizedSelectionError::Selection(
                selection::SelectionError::UnknownRunningSlot
            ))
        );
        assert_eq!(
            authorize_inactive(&layout, records, Some(RunningSlot::One)),
            Err(AuthorizedSelectionError::Selection(
                selection::SelectionError::RunningSlotMismatch
            ))
        );
        let permit = authorize_inactive(&layout, records, Some(RunningSlot::Zero)).unwrap();
        assert_eq!(permit.region().offset, layout.images[1].offset);
    }

    #[test]
    fn only_a_verified_report_can_unlock_an_inactive_target() {
        let layout = Layout {
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
                size: 8192,
            },
        };
        let records = [Entry::new(1, state::VALID).encode(), BLANK];
        assert_eq!(
            select_inactive(&layout, records, None),
            Err(selection::SelectionError::UnknownRunningSlot)
        );
        assert_eq!(
            select_inactive(&layout, records, Some(RunningSlot::One)),
            Err(selection::SelectionError::RunningSlotMismatch)
        );
        assert_eq!(
            select_inactive(&layout, records, Some(RunningSlot::Zero)),
            Ok(layout.images[1])
        );
        assert_eq!(RunningSlot::from_boot_report(2), None);
    }
}
