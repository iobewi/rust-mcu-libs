//! ESP partition reader bridge. Geometry is validated for the entire table,
//! including unrelated configuration and future domain partitions.
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
    let mut seen = [false; N];
    let mut error = None;
    const MAX_ENTRIES: usize = partitions::TABLE_BUFFER_SIZE / 32;
    let mut regions = [iobewi_firmware_layout::Region { offset: 0, size: 0 }; MAX_ENTRIES];
    let mut count = 0;
    partitions::for_each_entry(flash, scratch, |entry| {
        if count >= MAX_ENTRIES {
            error = Some(DiscoveryError::TableUnreadable);
            return;
        }
        regions[count] = iobewi_firmware_layout::Region {
            offset: entry.offset,
            size: entry.size as usize,
        };
        count += 1;
        // ESP application starts must be bootable; data still uses erase geometry.
        if entry.offset < 0x9000 || (entry.kind == 0 && entry.offset % 0x10000 != 0) {
            error = Some(DiscoveryError::Misaligned);
        }
        for (i, req) in required.iter().enumerate() {
            if entry.label != req.label {
                continue;
            }
            if seen[i] {
                error = Some(DiscoveryError::Duplicate);
            } else if entry.kind != req.kind || entry.subtype != req.subtype {
                error = Some(DiscoveryError::Missing);
            } else {
                found[i] = Some(partitions::PartitionRange {
                    offset: entry.offset,
                    size: entry.size as usize,
                });
            }
            seen[i] = true;
        }
    })
    .map_err(|_| DiscoveryError::TableUnreadable)?;
    if let Some(error) = error {
        return Err(error);
    }
    iobewi_firmware_layout::validate_regions(&regions[..count], flash_size, erase_size)?;
    let entries = required.iter().enumerate().filter_map(|(i, req)| {
        found[i].map(|r| iobewi_firmware_layout::Entry {
            label: req.label,
            kind: req.kind,
            subtype: req.subtype,
            offset: r.offset,
            size: r.size as u32,
        })
    });
    let checked = iobewi_firmware_layout::validate(entries, required, flash_size, erase_size)?;
    Ok(checked.map(|r| PartitionRange {
        offset: r.offset,
        size: r.size,
    }))
}
