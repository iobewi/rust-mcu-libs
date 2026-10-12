//! ESP shared-flash OTA artifact writer ported from iobewi_old.
//! Requires an independently verified inactive partition; no boot policy here.
//! One mutex acquisition per append or finish, released before ConfigSpace.

extern crate alloc;
use crate::{EspArtifactStorage, FlashWriteError, erase_partition_range};
use alloc::boxed::Box;
use alloc::vec;
use embedded_storage::nor_flash::NorFlash;
use iobewi_esp_flash::{EspFlash, SharedFlash};
use iobewi_esp_partitions::PartitionRange;
use iobewi_firmware_layout::authorization::InactiveWriteTarget;
use iobewi_firmware_update::{Committed, Error, WriteSession};

pub const fn erase_batch_size() -> u64 {
    64 * 1024
}
pub const fn erase_size() -> usize {
    <EspFlash as NorFlash>::ERASE_SIZE
}

pub struct ArtifactWriter<'a> {
    _permit: &'a InactiveWriteTarget,
    partition: PartitionRange,
    scratch: Box<[u8]>,
    erased_through: u64,
    sectors_flushed: u32,
    erase_batches: u32,
}

impl<'a> ArtifactWriter<'a> {
    /// Requires the portable inactive-slot authorization proof.
    /// The platform must independently establish running-slot identity.
    pub fn new(target: &'a InactiveWriteTarget) -> Self {
        let region = target.region();
        let partition = PartitionRange {
            offset: region.offset,
            size: region.size,
        };
        Self {
            _permit: target,
            partition,
            scratch: vec![0u8; erase_size()].into_boxed_slice(),
            erased_through: 0,
            sectors_flushed: 0,
            erase_batches: 0,
        }
    }

    pub fn partition(&self) -> PartitionRange {
        self.partition
    }
    pub fn sectors_flushed(&self) -> u32 {
        self.sectors_flushed
    }
    pub fn erase_batches(&self) -> u32 {
        self.erase_batches
    }

    /// Erase ahead in 64 KiB blocks while IOBEWI OTA's durable watermark still
    /// advances only after each sector has been programmed successfully.
    pub async fn append(
        &mut self,
        flash: &SharedFlash,
        engine: &mut WriteSession,
        data: &[u8],
    ) -> bool {
        if !engine.can_append(data.len()) {
            return false;
        }
        let end_received = engine.received() + data.len() as u64;
        let erase_batch = erase_batch_size();
        let desired_erased = end_received
            .div_ceil(erase_batch)
            .saturating_mul(erase_batch)
            .min(self.partition.size as u64);

        let mut flash_guard = flash.lock().await;
        let before = engine.durable();
        let raw_flash = &mut *flash_guard;
        let ok = if desired_erased > self.erased_through {
            if erase_partition_range(
                raw_flash,
                self.partition,
                self.erased_through,
                desired_erased,
            )
            .is_err()
            {
                false
            } else {
                self.erase_batches += ((desired_erased - self.erased_through) / erase_batch) as u32;
                self.erased_through = desired_erased;
                match EspArtifactStorage::new_pre_erased(
                    raw_flash,
                    self.partition,
                    self.scratch.as_mut(),
                ) {
                    Ok(mut backend) => engine.append(&mut backend, data).is_ok(),
                    Err(_) => false,
                }
            }
        } else {
            match EspArtifactStorage::new_pre_erased(
                raw_flash,
                self.partition,
                self.scratch.as_mut(),
            ) {
                Ok(mut backend) => engine.append(&mut backend, data).is_ok(),
                Err(_) => false,
            }
        };
        self.sectors_flushed += (engine.durable() - before).div_ceil(erase_size() as u64) as u32;
        ok
    }

    /// Flush a final partial sector after every received byte was covered
    /// by a prior erase. Returns IOBEWI OTA's digest/completeness decision.
    pub async fn finish(
        &mut self,
        flash: &SharedFlash,
        engine: WriteSession,
    ) -> Result<Committed, Error<FlashWriteError>> {
        if engine.received() > self.erased_through {
            return Err(Error::Incomplete {
                durable: engine.durable(),
            });
        }
        let before = engine.durable();
        let mut guard = flash.lock().await;
        let mut backend =
            EspArtifactStorage::new_pre_erased(&mut *guard, self.partition, self.scratch.as_mut())
                .map_err(Error::Backend)?;
        let committed = engine.finish(&mut backend)?;
        self.sectors_flushed += (committed.size - before).div_ceil(erase_size() as u64) as u32;
        Ok(committed)
    }
}
