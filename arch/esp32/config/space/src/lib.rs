#![no_std]

//! ESP NVS backend for IOBEWI ConfigSpace.
//!
//! ConfigSpace persistence semantics live here. Physical flash ownership and
//! the ESP NVS platform bridge are supplied by the ESP implementation crates, allowing
//! this backend to coexist with other storage consumers such as IOBEWI OTA.

extern crate alloc;

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use esp_nvs::Nvs;
use esp_nvs::error::Error as NvsError;
use iobewi_config_space::{Budget, ConfigBackend, Snapshot};
use iobewi_esp_flash::SharedFlash;
use iobewi_esp_nvs::{NvsFlash, open as open_nvs};
use iobewi_nvs_core::{
    capacity_units, decode_record, encode_record, entries_for_blob, reservation_units,
    valid_space_name,
};
use log::warn;

pub use iobewi_nvs_core::{ENTRIES_PER_PAGE, ITEM_SIZE, MAX_BLOB_DATA_PER_PAGE};

const NAMESPACE: esp_nvs::Key = esp_nvs::Key::from_str("cfg_space");
const HEALTH_NAMESPACE: esp_nvs::Key = esp_nvs::Key::from_str("cfg_health");
const HEALTH_KEY: esp_nvs::Key = esp_nvs::Key::from_str("canary");

pub use iobewi_esp_nvs::NvsPartition;
pub use iobewi_esp_flash::FlashPeripheral;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NvsConfigError {
    Unavailable,
    Write,
    InvalidSpace,
    CorruptRecord,
    GenerationOverflow,
}

static HEALTHY: AtomicBool = AtomicBool::new(true);

#[derive(Clone, Copy)]
pub struct NvsConfigBackend {
    flash: &'static SharedFlash,
    partition: NvsPartition,
    capacity_units: usize,
}

#[derive(Debug)]
pub enum NvsStartupError {
    Discovery(iobewi_esp_partitions::PartitionError),
    Geometry(iobewi_nvs_core::PartitionGeometryError),
    Backend(NvsConfigError),
}
impl NvsConfigBackend {
    /// Initialize the single shared ESP flash owner and discover the NVS partition.
    /// Call only once per firmware image, from the application composition root.
    /// For firmware with other flash consumers, initialize the owner separately
    /// and use `from_label` so all consumers share the same owner.
    pub async fn from_flash(
        flash: FlashPeripheral<'static>,
        label: &str,
    ) -> Result<Self, NvsStartupError> {
        let shared = iobewi_esp_flash::init(flash);
        Self::from_label(shared, label).await
    }

    /// Discover the named data/NVS partition using the existing shared owner.
    /// No fallback address, erase/reformat-on-error or new FlashStorage instance.
    pub async fn from_label(
        flash: &'static SharedFlash,
        label: &str,
    ) -> Result<Self, NvsStartupError> {
        use embedded_storage::nor_flash::{NorFlash, ReadNorFlash};
        let partition = {
            let mut owner = flash.lock().await;
            let mut table = [0u8; iobewi_esp_partitions::TABLE_BUFFER_SIZE];
            let range =
                iobewi_esp_partitions::find_by_label(owner.storage(), &mut table, label, 1, 2)
                    .map_err(NvsStartupError::Discovery)?;
            iobewi_nvs_core::validate_partition_geometry(
                range.offset as usize,
                range.size,
                owner.capacity(),
                iobewi_esp_flash::EspFlash::ERASE_SIZE,
            )
            .map_err(NvsStartupError::Geometry)?;
            NvsPartition::new(range.offset as usize, range.size)
        }; // Release the mutex before new() locks it again.
        Self::new(flash, partition)
            .await
            .map_err(NvsStartupError::Backend)
    }

    pub async fn new(
        flash: &'static SharedFlash,
        partition: NvsPartition,
    ) -> Result<Self, NvsConfigError> {
        let capacity_units = {
            let mut flash = flash.lock().await;
            let mut nvs = open_nvs(&mut flash, partition).map_err(|e| {
                warn!("NVS unavailable: {e:?}");
                HEALTHY.store(false, Ordering::Relaxed);
                NvsConfigError::Unavailable
            })?;
            let stats = nvs.statistics().map_err(|e| {
                warn!("Failed to read NVS statistics: {e:?}");
                HEALTHY.store(false, Ordering::Relaxed);
                NvsConfigError::Write
            })?;
            // Reservations are computed from each space's full budget, which
            // already covers the entries its stored blob occupies. Those
            // entries are neither empty nor erased, so they must be added back
            // or they would be counted twice and capacity would shrink with
            // every value persisted, until the next boot's claims fail.
            let owned = Self::owned_entries(&mut nvs)?;
            capacity_units(
                stats.entries_overall.empty as usize,
                stats.entries_overall.erased as usize,
                owned,
            )
        };
        HEALTHY.store(true, Ordering::Relaxed);
        Ok(Self {
            flash,
            partition,
            capacity_units,
        })
    }

    pub fn is_healthy(&self) -> bool {
        HEALTHY.load(Ordering::Relaxed)
    }

    pub async fn self_check(&self) -> bool {
        const VALUE: u8 = 0xA5;
        let ok = self
            .with_nvs(|nvs| {
                nvs.set(&HEALTH_NAMESPACE, &HEALTH_KEY, VALUE)
                    .map_err(|_| NvsConfigError::Write)?;
                let read_back = nvs
                    .get::<u8>(&HEALTH_NAMESPACE, &HEALTH_KEY)
                    .map_err(|_| NvsConfigError::Write)?
                    == VALUE;
                nvs.delete(&HEALTH_NAMESPACE, &HEALTH_KEY)
                    .map_err(|_| NvsConfigError::Write)?;
                Ok(read_back)
            })
            .await
            .unwrap_or(false);
        HEALTHY.store(ok, Ordering::Relaxed);
        ok
    }

    async fn with_nvs<R>(
        &self,
        f: impl FnOnce(&mut Nvs<NvsFlash<'_>>) -> Result<R, NvsConfigError>,
    ) -> Result<R, NvsConfigError> {
        let mut flash = self.flash.lock().await;
        let mut nvs = open_nvs(&mut flash, self.partition).map_err(|e| {
            warn!("NVS unavailable: {e:?}");
            HEALTHY.store(false, Ordering::Relaxed);
            NvsConfigError::Unavailable
        })?;
        let result = f(&mut nvs);
        if result.is_err() {
            HEALTHY.store(false, Ordering::Relaxed);
        }
        result
    }

    fn key(space: &str) -> Result<esp_nvs::Key, NvsConfigError> {
        if !valid_space_name(space) {
            return Err(NvsConfigError::InvalidSpace);
        }
        Ok(esp_nvs::Key::from_slice(space.as_bytes()))
    }

    /// Entries currently written by this backend's own blobs (one version per
    /// space; superseded versions are erased and already counted as free).
    fn owned_entries<T: esp_nvs::platform::Platform>(
        nvs: &mut Nvs<T>,
    ) -> Result<usize, NvsConfigError> {
        let mut keys = Vec::new();
        for entry in nvs.typed_entries() {
            let (namespace, key, _) = entry.map_err(|e| {
                warn!("Failed to enumerate NVS entries: {e:?}");
                NvsConfigError::Write
            })?;
            if namespace == NAMESPACE {
                keys.push(key);
            }
        }
        let mut owned = 0usize;
        for key in keys {
            let raw = nvs.get::<Vec<u8>>(&NAMESPACE, &key).map_err(|e| {
                warn!("Failed to read blob {}: {e:?}", key.as_str());
                NvsConfigError::Write
            })?;
            let entries = entries_for_blob(raw.len()).ok_or(NvsConfigError::CorruptRecord)?;
            owned = owned.saturating_add(entries);
        }
        Ok(owned)
    }

    async fn replace(
        &self,
        space: &str,
        present: bool,
        payload: &[u8],
    ) -> Result<u64, NvsConfigError> {
        let key = Self::key(space)?;
        self.with_nvs(|nvs| {
            let generation = match nvs.get::<Vec<u8>>(&NAMESPACE, &key) {
                Ok(raw) => {
                    let (generation, _, _) =
                        decode_record(&raw).map_err(|_| NvsConfigError::CorruptRecord)?;
                    generation
                        .checked_add(1)
                        .ok_or(NvsConfigError::GenerationOverflow)?
                }
                Err(NvsError::NamespaceNotFound | NvsError::KeyNotFound) => 1,
                Err(e) => {
                    warn!("Failed to read blob {}: {e:?}", key.as_str());
                    return Err(NvsConfigError::Write);
                }
            };
            let encoded = encode_record(generation, present, payload);
            nvs.set(&NAMESPACE, &key, encoded.as_slice()).map_err(|e| {
                warn!("Failed to save blob {}: {e:?}", key.as_str());
                NvsConfigError::Write
            })?;
            Ok(generation)
        })
        .await
    }
}

impl ConfigBackend for NvsConfigBackend {
    type Error = NvsConfigError;

    fn capacity_units(&self) -> usize {
        self.capacity_units
    }

    fn reservation_units(&self, space: &str, budget: Budget) -> Option<usize> {
        if !valid_space_name(space) {
            return None;
        }
        reservation_units(budget.max_bytes())
    }

    async fn load(&self, space: &str) -> Result<Option<Snapshot>, Self::Error> {
        let key = Self::key(space)?;
        self.with_nvs(|nvs| match nvs.get::<Vec<u8>>(&NAMESPACE, &key) {
            Ok(raw) => {
                let (generation, present, payload) =
                    decode_record(&raw).map_err(|_| NvsConfigError::CorruptRecord)?;
                Ok(present.then(|| Snapshot {
                    generation,
                    data: payload.to_vec(),
                }))
            }
            Err(NvsError::NamespaceNotFound | NvsError::KeyNotFound) => Ok(None),
            Err(e) => {
                warn!("Failed to read blob {}: {e:?}", key.as_str());
                Err(NvsConfigError::Write)
            }
        })
        .await
    }

    async fn commit(&self, space: &str, data: &[u8]) -> Result<u64, Self::Error> {
        self.replace(space, true, data).await
    }

    async fn clear(&self, space: &str) -> Result<u64, Self::Error> {
        self.replace(space, false, &[]).await
    }
}
