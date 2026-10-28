//! Dedicated, transactional store of the OTA metadata (OTM1 bytes) in the
//! `ota_meta` partition: two consecutive erase units, one copy each, managed by
//! [`iobewi_firmware_update::frame_store`]. It shares no code path, partition or
//! lifecycle with the application configuration (ConfigManager / EKV).
//!
//! The partition belongs to this store alone: `otadata` (boot selection) and
//! `config` (application parameters) are other partitions with other owners.
//! Every operation locks the one [`SharedFlash`] for its own duration and
//! releases it before returning, like every other flash consumer.

extern crate alloc;
use alloc::vec::Vec;
use embedded_storage::nor_flash::{NorFlash, ReadNorFlash};
use iobewi_esp_flash::{EspFlash, SharedFlash};
use iobewi_esp_partitions::PartitionRange;
use iobewi_firmware_layout::{DiscoveryError, Requirement};
use iobewi_firmware_update::frame_store::{FrameStore, Loaded};
use iobewi_firmware_update::metadata::MetadataStore;
use iobewi_firmware_update::nor_frames::NorFrames;

/// Partition label, type (`data`) and custom subtype of the region.
pub const LABEL: &str = "ota_meta";
pub const DATA_TYPE: u8 = 0x01;
pub const SUBTYPE: u8 = 0x41;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Table unreadable, partition missing/duplicated, or table geometry invalid.
    Discovery(DiscoveryError),
    /// The partition cannot host two copies on this flash.
    Geometry,
}

/// Finds `ota_meta` in the partition table, validating the whole table.
pub async fn discover(flash: &SharedFlash) -> Result<PartitionRange, Error> {
    let mut guard = flash.lock().await;
    let flash_size = u32::try_from(guard.capacity()).map_err(|_| Error::Geometry)?;
    let [region] = crate::partitions::discover(
        guard.storage(),
        &mut [0; iobewi_esp_partitions::TABLE_BUFFER_SIZE],
        &[Requirement {
            label: LABEL,
            kind: DATA_TYPE,
            subtype: SUBTYPE,
        }],
        flash_size,
        <EspFlash as NorFlash>::ERASE_SIZE as u32,
    )
    .map_err(Error::Discovery)?;
    if region.size < 2 * <EspFlash as NorFlash>::ERASE_SIZE {
        return Err(Error::Geometry);
    }
    Ok(region)
}

/// [`MetadataStore`] over the `ota_meta` partition.
///
/// `load_raw` maps a blank region to `None`; a region that is neither blank nor
/// holds a valid copy is an error, never silently re-initialised. Recovery is
/// the explicit [`EspOtaMeta::erase_all`].
pub struct EspOtaMeta<'a> {
    flash: &'a SharedFlash,
    region: PartitionRange,
}

impl<'a> EspOtaMeta<'a> {
    pub fn new(flash: &'a SharedFlash, region: PartitionRange) -> Self {
        Self { flash, region }
    }

    /// Explicit reprovisioning of the OTA metadata: erases both copies.
    pub async fn erase_all(&self) -> Result<(), ()> {
        let mut guard = self.flash.lock().await;
        let backend = NorFrames::new(&mut *guard, self.region.offset, self.region.size as u32)
            .map_err(|_| ())?;
        FrameStore::new(backend).erase_all().map_err(|_| ())
    }

    /// Whether the region is readable (blank or holding a valid copy). Used as
    /// the persistence part of the boot self-check, independent of any other
    /// store.
    pub async fn is_healthy(&self) -> bool {
        self.load_raw().await.is_ok()
    }
}

impl MetadataStore for EspOtaMeta<'_> {
    type Error = ();

    async fn load_raw(&self) -> Result<Option<Vec<u8>>, ()> {
        let mut guard = self.flash.lock().await;
        let backend = NorFrames::new(&mut *guard, self.region.offset, self.region.size as u32)
            .map_err(|_| ())?;
        match FrameStore::new(backend).load().map_err(|_| ())? {
            Loaded::Blank => Ok(None),
            Loaded::Payload { bytes, .. } => Ok(Some(bytes)),
            Loaded::Corrupted => Err(()),
        }
    }

    async fn commit_raw(&self, bytes: &[u8]) -> Result<(), ()> {
        let mut guard = self.flash.lock().await;
        let backend = NorFrames::new(&mut *guard, self.region.offset, self.region.size as u32)
            .map_err(|_| ())?;
        FrameStore::new(backend).commit(bytes).map_err(|_| ())
    }
}
