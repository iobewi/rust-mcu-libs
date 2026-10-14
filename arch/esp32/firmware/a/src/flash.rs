//! Bounded physical flash operations for firmware A image partitions.
//! Every operation acquires the process-wide SharedFlash lock exactly once.
//! The caller is responsible for selecting an inactive OTA partition.
use crate::Partition;
use embedded_storage::nor_flash::{NorFlash, ReadNorFlash};
use iobewi_esp_flash::SharedFlash;
use iobewi_firmware_layout::authorization::InactiveWriteTarget;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlashError {
    OutOfBounds,
    Misaligned,
    Driver,
}

/// Convert a relative range into a bounded absolute offset.
fn checked_offset(partition: Partition, offset: u32, len: u32) -> Result<u32, FlashError> {
    let end = offset.checked_add(len).ok_or(FlashError::OutOfBounds)?;
    if end > partition.size {
        return Err(FlashError::OutOfBounds);
    }
    partition
        .offset
        .checked_add(offset)
        .ok_or(FlashError::OutOfBounds)
}

/// Read only from the designated partition. May be used for verification.
pub async fn read(
    flash: &SharedFlash,
    partition: Partition,
    offset: u32,
    bytes: &mut [u8],
) -> Result<(), FlashError> {
    let len = u32::try_from(bytes.len()).map_err(|_| FlashError::OutOfBounds)?;
    let absolute = checked_offset(partition, offset, len)?;
    let mut guard = flash.lock().await;
    guard.read(absolute, bytes).map_err(|_| FlashError::Driver)
}

/// Erase an aligned portion of the authorized inactive firmware slot.
/// An arbitrary partition cannot be supplied by the caller.
pub async fn erase(
    flash: &SharedFlash,
    target: &mut InactiveWriteTarget,
    offset: u32,
    len: u32,
) -> Result<(), FlashError> {
    let region = target.region();
    let partition = Partition {
        offset: region.offset,
        size: u32::try_from(region.size).map_err(|_| FlashError::OutOfBounds)?,
    };
    let absolute = checked_offset(partition, offset, len)?;
    let end = absolute.checked_add(len).ok_or(FlashError::OutOfBounds)?;
    let unit = <iobewi_esp_flash::EspFlash as NorFlash>::ERASE_SIZE as u32;
    if absolute % unit != 0 || len % unit != 0 {
        return Err(FlashError::Misaligned);
    }
    if len == 0 {
        return Ok(());
    }
    let mut guard = flash.lock().await;
    guard.erase(absolute, end).map_err(|_| FlashError::Driver)
}

/// Program bytes into the authorized inactive slot. Does not erase or
/// publish metadata; the update transaction coordinates these operations.
pub async fn write(
    flash: &SharedFlash,
    target: &mut InactiveWriteTarget,
    offset: u32,
    bytes: &[u8],
) -> Result<(), FlashError> {
    let len = u32::try_from(bytes.len()).map_err(|_| FlashError::OutOfBounds)?;
    let region = target.region();
    let partition = Partition {
        offset: region.offset,
        size: u32::try_from(region.size).map_err(|_| FlashError::OutOfBounds)?,
    };
    let absolute = checked_offset(partition, offset, len)?;
    let unit = <iobewi_esp_flash::EspFlash as NorFlash>::WRITE_SIZE as u32;
    if absolute % unit != 0 || len % unit != 0 {
        return Err(FlashError::Misaligned);
    }
    if bytes.is_empty() {
        return Ok(());
    }
    let mut guard = flash.lock().await;
    guard.write(absolute, bytes).map_err(|_| FlashError::Driver)
}
