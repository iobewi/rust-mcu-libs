//! B-domain discovery and bounded readback on the existing SharedFlash.
use embedded_storage::nor_flash::{NorFlash, ReadNorFlash};
use iobewi_esp_flash::{EspFlash, SharedFlash};
use iobewi_firmware_layout::{
    Requirement,
    domain_b::{self, Region, WorkloadLayout},
};
use iobewi_firmware_update::compat::otm2::{Side, slot_index};
use iobewi_workload_native::runtime::{ArtifactReader, RuntimeError};
use sha2::{Digest as _, Sha256};

pub async fn discover(flash: &SharedFlash) -> Result<WorkloadLayout, ()> {
    let mut guard = flash.lock().await;
    let capacity = u32::try_from(guard.capacity()).map_err(|_| ())?;
    let required = [
        domain_b::LABEL_META,
        domain_b::LABEL_SLOT_A,
        domain_b::LABEL_SLOT_B,
    ]
    .map(|label| Requirement {
        label,
        kind: 1,
        subtype: 6,
    });
    let [meta, a, b] = iobewi_esp_firmware_storage::partitions::discover(
        guard.storage(),
        &mut [0; iobewi_esp_partitions::TABLE_BUFFER_SIZE],
        &required,
        capacity,
        EspFlash::ERASE_SIZE as u32,
    )
    .map_err(|_| ())?;
    let region =
        |p: iobewi_esp_partitions::PartitionRange| Some(Region::new(p.offset, p.size as u32));
    domain_b::assemble(
        region(meta),
        region(a),
        region(b),
        EspFlash::ERASE_SIZE as u32,
    )
    .map_err(|_| ())
}
pub fn region(layout: &WorkloadLayout, side: Side) -> Region {
    layout.regions()[1 + slot_index(side)]
}
pub struct Reader<'a> {
    pub flash: &'a SharedFlash,
    pub region: Region,
    pub size: u32,
}
impl ArtifactReader for Reader<'_> {
    async fn read(&self, offset: u64, out: &mut [u8]) -> Result<(), RuntimeError> {
        let end = offset
            .checked_add(out.len() as u64)
            .ok_or(RuntimeError { reason: "bounds" })?;
        if self.size > self.region.size || end > self.size as u64 {
            return Err(RuntimeError { reason: "bounds" });
        }
        let address = (self.region.offset as u64)
            .checked_add(offset)
            .and_then(|n| u32::try_from(n).ok())
            .ok_or(RuntimeError { reason: "bounds" })?;
        self.flash
            .lock()
            .await
            .read(address, out)
            .map_err(|_| RuntimeError { reason: "flash" })
    }
}
impl Reader<'_> {
    pub async fn verify(&self, expected: &[u8; 32]) -> Result<(), RuntimeError> {
        let mut hash = Sha256::new();
        let mut buffer = [0; 2048];
        let mut offset = 0u64;
        while offset < self.size as u64 {
            let n = ((self.size as u64 - offset) as usize).min(buffer.len());
            self.read(offset, &mut buffer[..n]).await?;
            hash.update(&buffer[..n]);
            offset += n as u64;
        }
        let actual: [u8; 32] = hash.finalize().into();
        if &actual != expected {
            return Err(RuntimeError { reason: "digest" });
        }
        Ok(())
    }
}
