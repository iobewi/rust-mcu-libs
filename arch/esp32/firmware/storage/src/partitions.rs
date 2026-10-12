//! ESP partition reader bridge. Validation lives in common/firmware/layout.
use iobewi_esp_partitions::{self as partitions, FlashStorage, PartitionRange};
pub use iobewi_firmware_layout::{DiscoveryError, Requirement};

pub fn discover<const N: usize>(
    flash: &mut FlashStorage<'_>,
    scratch: &mut [u8; partitions::TABLE_BUFFER_SIZE],
    required: &[Requirement<'_>; N],
    flash_size: u32,
    erase_size: u32,
) -> Result<[PartitionRange; N], DiscoveryError> {
    let mut found = [None; N];
    let mut error = None;
    partitions::for_each_entry(flash, scratch, |entry| {
        for (i, req) in required.iter().enumerate() {
            if entry.label != req.label { continue; }
            if found[i].is_some() { error = Some(DiscoveryError::Duplicate); }
            else if entry.kind != req.kind || entry.subtype != req.subtype { error = Some(DiscoveryError::Missing); }
            else { found[i] = Some(partitions::PartitionRange { offset: entry.offset, size: entry.size as usize }); }
        }
    }).map_err(|_| DiscoveryError::TableUnreadable)?;
    if let Some(error) = error { return Err(error); }
    let entries = required.iter().enumerate().filter_map(|(i, req)| {
        found[i].map(|r| iobewi_firmware_layout::Entry {
            label: req.label, kind: req.kind, subtype: req.subtype, offset: r.offset, size: r.size as u32,
        })
    });
    let checked = iobewi_firmware_layout::validate(entries, required, flash_size, erase_size)?;
    Ok(checked.map(|r| PartitionRange { offset: r.offset, size: r.size }))
}
