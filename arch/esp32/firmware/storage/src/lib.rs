#![no_std]

//! Firmware artifact storage over an already selected ESP flash partition.
//! Ported from iobewi_old/firmware/esp32/src/lib.rs, excluding A-specific
//! partition selection and boot decisions. Safe for independent domains A/B.
//! The caller must enforce inactive-slot selection before writing.

use embedded_storage::nor_flash::NorFlash;
use iobewi_esp_partitions::{PartitionRange, erase_range as erase_raw_partition_range};
use iobewi_firmware_update::ArtifactStorage;
pub mod partitions;
#[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
pub mod shared_flash;

/// Failure while committing artifact bytes to ESP NOR flash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlashWriteError {
    /// The caller-provided scratch area is smaller than one erase block.
    ScratchTooSmall,
    /// Flash address arithmetic overflowed or the erase block would exceed the partition.
    AddressOverflow,
    /// The requested logical block is not aligned to the NOR erase geometry.
    Unaligned,
    /// The underlying NOR erase/program operation failed.
    Flash,
}

/// Erases one already-selected logical range inside an OTA partition.
///
/// The range is relative to the partition and must be aligned to the
/// underlying NOR erase size. Passing a larger aligned range lets concrete
/// ESP flash drivers use block erase commands instead of one 4 KiB sector
/// erase per call.
///
/// This has no staging semantics of its own: callers must only erase a target
/// their transaction/boot policy has already declared safe.
pub fn erase_partition_range<F>(
    flash: &mut F,
    partition: PartitionRange,
    logical_from: u64,
    logical_to: u64,
) -> Result<(), FlashWriteError>
where
    F: NorFlash,
{
    erase_raw_partition_range(flash, partition, logical_from, logical_to).map_err(|e| match e {
        iobewi_esp_partitions::PartitionError::AddressOverflow => FlashWriteError::AddressOverflow,
        iobewi_esp_partitions::PartitionError::Unaligned => FlashWriteError::Unaligned,
        _ => FlashWriteError::Flash,
    })
}

/// Sector-aware [`ArtifactStorage`] over an already-selected ESP app
/// partition.
///
/// The caller owns the flash object. The common ESP hardware layer is
/// the ESP implementation crates; IOBEWI OTA only layers artifact semantics over the
/// already-selected partition.
pub struct EspArtifactStorage<'a, F> {
    flash: &'a mut F,
    partition_offset: u32,
    partition_size: usize,
    scratch: &'a mut [u8],
    erase_before_write: bool,
}

impl<'a, F> EspArtifactStorage<'a, F>
where
    F: NorFlash,
{
    pub fn new(
        flash: &'a mut F,
        partition: PartitionRange,
        scratch: &'a mut [u8],
    ) -> Result<Self, FlashWriteError> {
        Self::new_with_erase_mode(flash, partition, scratch, true)
    }

    /// Constructs a backend for a range the caller has already erased.
    ///
    /// This preserves sector-sized durability while skipping the per-sector
    /// erase. Pair it with `erase_partition_range` to erase large ESP flash
    /// blocks efficiently and still report durability every erase sector.
    pub fn new_pre_erased(
        flash: &'a mut F,
        partition: PartitionRange,
        scratch: &'a mut [u8],
    ) -> Result<Self, FlashWriteError> {
        Self::new_with_erase_mode(flash, partition, scratch, false)
    }

    fn new_with_erase_mode(
        flash: &'a mut F,
        partition: PartitionRange,
        scratch: &'a mut [u8],
        erase_before_write: bool,
    ) -> Result<Self, FlashWriteError> {
        if scratch.len() < F::ERASE_SIZE {
            return Err(FlashWriteError::ScratchTooSmall);
        }
        Ok(Self {
            flash,
            partition_offset: partition.offset,
            partition_size: partition.size,
            scratch,
            erase_before_write,
        })
    }

    pub fn erase_size(&self) -> usize {
        F::ERASE_SIZE
    }

    fn flush_block(&mut self, logical_offset: u64, bytes: &[u8]) -> Result<(), FlashWriteError> {
        if bytes.len() > F::ERASE_SIZE {
            return Err(FlashWriteError::Flash);
        }

        let logical_offset =
            usize::try_from(logical_offset).map_err(|_| FlashWriteError::AddressOverflow)?;
        if logical_offset % F::ERASE_SIZE != 0 {
            return Err(FlashWriteError::Unaligned);
        }
        let erase_logical_end = logical_offset
            .checked_add(F::ERASE_SIZE)
            .ok_or(FlashWriteError::AddressOverflow)?;
        if erase_logical_end > self.partition_size {
            return Err(FlashWriteError::AddressOverflow);
        }

        let padded = bytes.len().div_ceil(F::WRITE_SIZE) * F::WRITE_SIZE;
        if padded > F::ERASE_SIZE || padded > self.scratch.len() {
            return Err(FlashWriteError::ScratchTooSmall);
        }

        self.scratch[..bytes.len()].copy_from_slice(bytes);
        self.scratch[bytes.len()..padded].fill(0);

        let absolute = usize::try_from(self.partition_offset)
            .map_err(|_| FlashWriteError::AddressOverflow)?
            .checked_add(logical_offset)
            .ok_or(FlashWriteError::AddressOverflow)?;
        let erase_end = absolute
            .checked_add(F::ERASE_SIZE)
            .ok_or(FlashWriteError::AddressOverflow)?;
        let absolute = u32::try_from(absolute).map_err(|_| FlashWriteError::AddressOverflow)?;
        let erase_end = u32::try_from(erase_end).map_err(|_| FlashWriteError::AddressOverflow)?;

        if self.erase_before_write {
            self.flash
                .erase(absolute, erase_end)
                .map_err(|_| FlashWriteError::Flash)?;
        }
        self.flash
            .write(absolute, &self.scratch[..padded])
            .map_err(|_| FlashWriteError::Flash)
    }
}

impl<F> ArtifactStorage for EspArtifactStorage<'_, F>
where
    F: NorFlash,
{
    type Error = FlashWriteError;

    fn write(&mut self, durable_offset: u64, pending: &[u8]) -> Result<u64, Self::Error> {
        let erase_size = F::ERASE_SIZE;
        let mut consumed = 0usize;
        while pending.len().saturating_sub(consumed) >= erase_size {
            let end = consumed + erase_size;
            self.flush_block(durable_offset + consumed as u64, &pending[consumed..end])?;
            consumed = end;
        }
        Ok(durable_offset + consumed as u64)
    }

    fn finish(&mut self, durable_offset: u64, pending: &[u8]) -> Result<u64, Self::Error> {
        if pending.is_empty() {
            return Ok(durable_offset);
        }
        if pending.len() >= F::ERASE_SIZE {
            return Err(FlashWriteError::Flash);
        }
        self.flush_block(durable_offset, pending)?;
        Ok(durable_offset + pending.len() as u64)
    }
}
