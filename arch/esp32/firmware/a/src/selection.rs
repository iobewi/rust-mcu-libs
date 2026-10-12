//! Conservative target selection: never erase the currently executing firmware.
//! Flash metadata alone is insufficient when running a custom bootloader.
use crate::{Layout, Partition};
use iobewi_firmware_boot::{Raw, SECTOR_COUNT, update_target};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionError {
    UnknownRunningSlot,
    RunningSlotMismatch,
    NoTrustedImage,
    InvalidRunningSlot,
    InvalidTarget,
}

/// The selected target is always different from the **observed running slot**.
/// The observer must come from the execution/boot platform (not untrusted OTA
/// metadata). If it disagrees with the transactional boot metadata's newest trusted entry, refuse.
pub fn inactive(
    layout: &Layout,
    otadata: [Raw; SECTOR_COUNT],
    running_slot: Option<u8>,
) -> Result<Partition, SelectionError> {
    let running = running_slot.ok_or(SelectionError::UnknownRunningSlot)?;
    if running > 1 {
        return Err(SelectionError::InvalidRunningSlot);
    }
    let target = update_target(otadata, 2).ok_or(SelectionError::NoTrustedImage)?;
    if target > 1 || target == running {
        return Err(SelectionError::RunningSlotMismatch);
    }
    layout
        .images
        .get(usize::from(target))
        .copied()
        .ok_or(SelectionError::InvalidTarget)
}

#[cfg(test)]
mod tests {
    use super::*;
    use iobewi_firmware_boot::{Entry, state};
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
    fn never_return_running_slot() {
        let data = [
            Entry::new(1, state::VALID).encode(),
            iobewi_firmware_boot::BLANK,
        ];
        assert_eq!(inactive(&layout(), data, Some(0)), Ok(layout().images[1]));
        assert_eq!(
            inactive(&layout(), data, Some(1)),
            Err(SelectionError::RunningSlotMismatch)
        );
        assert_eq!(
            inactive(&layout(), data, None),
            Err(SelectionError::UnknownRunningSlot)
        );
        assert_eq!(
            inactive(&layout(), data, Some(2)),
            Err(SelectionError::InvalidRunningSlot)
        );
    }

    #[test]
    fn reject_untrusted_or_stale_metadata() {
        let blank = [iobewi_firmware_boot::BLANK; SECTOR_COUNT];
        assert_eq!(
            inactive(&layout(), blank, Some(0)),
            Err(SelectionError::NoTrustedImage)
        );
        let new_only = [
            Entry::new(2, state::NEW).encode(),
            iobewi_firmware_boot::BLANK,
        ];
        assert_eq!(
            inactive(&layout(), new_only, Some(1)),
            Err(SelectionError::NoTrustedImage)
        );
    }
}
