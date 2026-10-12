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

/// The boot report must be independently verified by the platform adapter.
/// Missing, invalid, or contradicting observations fail closed.
pub fn select_inactive(
    layout: &Layout,
    metadata: [Raw; SECTOR_COUNT],
    boot_report: Option<RunningSlot>,
) -> Result<Partition, selection::SelectionError> {
    selection::inactive(layout, metadata, boot_report.map(RunningSlot::index))
}

#[cfg(test)]
mod tests {
    use super::*;
    use iobewi_firmware_boot::{BLANK, Entry, state};

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
