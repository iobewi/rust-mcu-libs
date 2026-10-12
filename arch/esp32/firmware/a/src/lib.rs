#![no_std]

//! ESP32 legacy A-only firmware layout mapping.
//! Pure validation, independent of flash access. The actual flash reader,
//! writer and custom EWBT-compatible bootloader are separate integration gates.

#[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
pub mod flash;

pub mod selection;

use iobewi_firmware_slots::{Domain, FirmwareSlot, Slot};

pub const PARTITION_LAYOUT: &str = "embewi-ab-v1";
pub const OTA0: &str = "ota_0";
pub const OTA1: &str = "ota_1";
pub const OTADATA: &str = "otadata";
pub const APP_TYPE: u8 = 0;
pub const DATA_TYPE: u8 = 1;
pub const OTA0_SUBTYPE: u8 = 0x10;
pub const OTA1_SUBTYPE: u8 = 0x11;
pub const OTADATA_SUBTYPE: u8 = 0x00;
pub const OTADATA_MIN_SIZE: u32 = 0x2000;

/// Discovered partition table entry, without owning the flash driver.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Partition {
    pub offset: u32,
    pub size: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry<'a> {
    pub label: &'a str,
    pub kind: u8,
    pub subtype: u8,
    pub partition: Partition,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutError {
    UnsupportedDomain,
    MissingSlot,
    MissingMetadata,
    Duplicate,
    Unaligned,
    OutOfBounds,
    Overlapping,
    Overflow,
    MetadataTooSmall,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub images: [Partition; 2],
    pub otadata: Partition,
}

pub fn label(slot: FirmwareSlot) -> Result<&'static str, LayoutError> {
    match (slot.domain, slot.slot) {
        (Domain::A, Slot::Zero) => Ok(OTA0),
        (Domain::A, Slot::One) => Ok(OTA1),
        (Domain::B, _) => Err(LayoutError::UnsupportedDomain),
    }
}

/// Select exactly two ESP application slots and one metadata partition.
/// Reject ambiguous, overlapping and out-of-flash partition maps.
pub fn discover(entries: &[Entry<'_>], flash_size: u32) -> Result<Layout, LayoutError> {
    let mut slots = [None, None];
    let mut metadata = None;
    for entry in entries {
        let index = match (entry.kind, entry.subtype, entry.label) {
            (APP_TYPE, OTA0_SUBTYPE, OTA0) => Some(0),
            (APP_TYPE, OTA1_SUBTYPE, OTA1) => Some(1),
            _ => None,
        };
        let is_metadata =
            (entry.kind, entry.subtype, entry.label) == (DATA_TYPE, OTADATA_SUBTYPE, OTADATA);
        if index.is_none() && !is_metadata {
            continue;
        }
        let p = entry.partition;
        if p.size == 0 || p.offset % 0x1000 != 0 || p.size % 0x1000 != 0 {
            return Err(LayoutError::Unaligned);
        }
        let end = p.offset.checked_add(p.size).ok_or(LayoutError::Overflow)?;
        if end > flash_size {
            return Err(LayoutError::OutOfBounds);
        }
        if let Some(i) = index {
            if slots[i].replace(p).is_some() {
                return Err(LayoutError::Duplicate);
            }
        } else if metadata.replace(p).is_some() {
            return Err(LayoutError::Duplicate);
        }
    }
    let [Some(a0), Some(a1)] = slots else {
        return Err(LayoutError::MissingSlot);
    };
    let meta = metadata.ok_or(LayoutError::MissingMetadata)?;
    if meta.size < OTADATA_MIN_SIZE {
        return Err(LayoutError::MetadataTooSmall);
    }
    for (a, b) in [(a0, a1), (a0, meta), (a1, meta)] {
        if a.offset < b.offset + b.size && b.offset < a.offset + a.size {
            return Err(LayoutError::Overlapping);
        }
    }
    Ok(Layout {
        images: [a0, a1],
        otadata: meta,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(label: &'static str, kind: u8, subtype: u8, offset: u32, size: u32) -> Entry<'static> {
        Entry {
            label,
            kind,
            subtype,
            partition: Partition { offset, size },
        }
    }
    fn standard() -> [Entry<'static>; 3] {
        [
            entry(OTA0, APP_TYPE, OTA0_SUBTYPE, 0x20000, 0x180000),
            entry(OTA1, APP_TYPE, OTA1_SUBTYPE, 0x1a0000, 0x180000),
            entry(OTADATA, DATA_TYPE, OTADATA_SUBTYPE, 0xd000, 0x2000),
        ]
    }

    #[test]
    fn a_only_maps_legacy_labels_without_changing_slot_ids() {
        let map = discover(&standard(), 0x400000).unwrap();
        assert_eq!(map.images[0].offset, 0x20000);
        assert_eq!(label(FirmwareSlot::new(Domain::A, Slot::One)), Ok(OTA1));
        assert_eq!(
            label(FirmwareSlot::new(Domain::B, Slot::Zero)),
            Err(LayoutError::UnsupportedDomain)
        );
        assert_eq!(PARTITION_LAYOUT, "embewi-ab-v1");
    }
    #[test]
    fn reject_missing_duplicate_overlapping_and_out_of_bounds() {
        let e = standard();
        assert_eq!(
            discover(&e[..2], 0x400000),
            Err(LayoutError::MissingMetadata)
        );
        let mut e = standard();
        e[1] = e[0];
        assert_eq!(discover(&e, 0x400000), Err(LayoutError::Duplicate));
        let mut e = standard();
        e[1].partition.offset = 0x20000;
        assert_eq!(discover(&e, 0x400000), Err(LayoutError::Overlapping));
        let mut e = standard();
        e[0].partition.size = 0x400000;
        assert_eq!(discover(&e, 0x400000), Err(LayoutError::OutOfBounds));
    }
}
