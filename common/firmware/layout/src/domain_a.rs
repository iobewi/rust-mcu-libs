//! ESP32 legacy A-only firmware layout mapping.
//! Pure validation, independent of flash access. The actual flash reader,
//! writer and custom transactional-boot-metadata-compatible bootloader are separate integration gates.

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
/// The last sector of every application slot holds its boot proof (IAP1, see
/// `iobewi-firmware-bootauth`). No image byte may ever be written there: the usable
/// image capacity of a slot is its size minus this sector, whatever the boot profile.
pub const PROOF_SECTOR_SIZE: u32 = 0x1000;

/// Discovered partition table entry, without owning the flash driver.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Partition {
    pub offset: u32,
    pub size: u32,
}

impl Partition {
    /// Bytes an image may occupy: the partition without its proof sector.
    pub const fn usable_size(&self) -> u32 {
        self.size.saturating_sub(PROOF_SECTOR_SIZE)
    }

    /// Flash offset of the proof sector (the last sector of the partition).
    pub const fn proof_offset(&self) -> u32 {
        self.offset + self.usable_size()
    }
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
        (_, _) => Err(LayoutError::UnsupportedDomain),
    }
}

/// Select exactly two ESP application slots and one metadata partition.
/// Reject ambiguous, overlapping and out-of-flash partition maps.
pub fn discover(entries: &[Entry<'_>], flash_size: u32) -> Result<Layout, LayoutError> {
    let required = [
        crate::Requirement {
            label: OTA0,
            kind: APP_TYPE,
            subtype: OTA0_SUBTYPE,
        },
        crate::Requirement {
            label: OTA1,
            kind: APP_TYPE,
            subtype: OTA1_SUBTYPE,
        },
        crate::Requirement {
            label: OTADATA,
            kind: DATA_TYPE,
            subtype: OTADATA_SUBTYPE,
        },
    ];
    // Reject ambiguous labels even when one duplicate has the wrong subtype.
    for req in &required {
        if entries.iter().filter(|e| e.label == req.label).count() > 1 {
            return Err(LayoutError::Duplicate);
        }
    }
    let checked = crate::validate(
        entries.iter().map(|e| crate::Entry {
            label: e.label,
            kind: e.kind,
            subtype: e.subtype,
            offset: e.partition.offset,
            size: e.partition.size,
        }),
        &required,
        flash_size,
        0x1000,
    )
    .map_err(|error| match error {
        crate::DiscoveryError::Missing => {
            if required[..2].iter().any(|r| {
                !entries
                    .iter()
                    .any(|e| e.label == r.label && e.kind == r.kind && e.subtype == r.subtype)
            }) {
                LayoutError::MissingSlot
            } else {
                LayoutError::MissingMetadata
            }
        }
        crate::DiscoveryError::Duplicate => LayoutError::Duplicate,
        crate::DiscoveryError::Misaligned => LayoutError::Unaligned,
        crate::DiscoveryError::OutOfFlash => LayoutError::OutOfBounds,
        crate::DiscoveryError::Overflow => LayoutError::Overflow,
        crate::DiscoveryError::Overlap => LayoutError::Overlapping,
        crate::DiscoveryError::TableUnreadable => LayoutError::MissingMetadata,
    })?;
    let [a0, a1, meta] = checked.map(|r| Partition {
        offset: r.offset,
        size: r.size as u32,
    });
    if a0.offset % 0x10000 != 0 || a1.offset % 0x10000 != 0 {
        return Err(LayoutError::Unaligned);
    }
    if meta.size < OTADATA_MIN_SIZE {
        return Err(LayoutError::MetadataTooSmall);
    }
    // A slot must hold at least one image sector besides its proof sector.
    if a0.size <= PROOF_SECTOR_SIZE || a1.size <= PROOF_SECTOR_SIZE {
        return Err(LayoutError::OutOfBounds);
    }
    Ok(Layout {
        images: [a0, a1],
        otadata: meta,
    })
}

impl Layout {
    /// Recheck public fields before authorizing a writer.
    pub fn validate(&self, flash_size: u32) -> Result<(), LayoutError> {
        discover(
            &[
                Entry {
                    label: OTA0,
                    kind: APP_TYPE,
                    subtype: OTA0_SUBTYPE,
                    partition: self.images[0],
                },
                Entry {
                    label: OTA1,
                    kind: APP_TYPE,
                    subtype: OTA1_SUBTYPE,
                    partition: self.images[1],
                },
                Entry {
                    label: OTADATA,
                    kind: DATA_TYPE,
                    subtype: OTADATA_SUBTYPE,
                    partition: self.otadata,
                },
            ],
            flash_size,
        )
        .map(|_| ())
    }
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
    #[test]
    fn rejects_nonbootable_app_alignment_and_wrong_subtype_duplicates() {
        let mut e = standard();
        e[0].partition.offset -= 0x1000;
        assert_eq!(discover(&e, 0x400000), Err(LayoutError::Unaligned));
        let mut wrong = e[0];
        wrong.subtype = OTA1_SUBTYPE;
        let table = [standard()[0], standard()[1], standard()[2], wrong];
        assert_eq!(discover(&table, 0x400000), Err(LayoutError::Duplicate));
        let mut e = standard();
        e[0].partition.offset = 0xffff0000;
        assert_eq!(discover(&e, u32::MAX), Err(LayoutError::Overflow));
    }
    #[test]
    fn the_last_sector_of_a_slot_is_its_proof_sector() {
        let p = Partition {
            offset: 0x20000,
            size: 0x1b0000,
        };
        assert_eq!(p.usable_size(), 0x1b0000 - PROOF_SECTOR_SIZE);
        assert_eq!(p.proof_offset(), 0x20000 + 0x1b0000 - PROOF_SECTOR_SIZE);
        assert_eq!(p.proof_offset() + PROOF_SECTOR_SIZE, p.offset + p.size);
    }
    #[test]
    fn a_slot_without_room_for_an_image_sector_is_refused() {
        let mut e = standard();
        e[0].partition.size = PROOF_SECTOR_SIZE;
        assert_eq!(discover(&e, 0x400000), Err(LayoutError::OutOfBounds));
    }
}
