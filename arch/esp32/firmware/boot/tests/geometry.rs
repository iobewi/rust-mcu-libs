//! The two reference geometries of the ESP32-S3 qualification board, read from the versioned
//! partition files, decode to the same logical layout; the boot decision does not care where
//! the metadata sits; a table that crosses the geometry the bootloader was built for is refused.

use iobewi_esp_boot::{geometry, layout};
use iobewi_firmware_boot::{Boot, Entry, plan_boot, state};
use iobewi_firmware_layout::{authorization::SlotLayout, domain_a};

const LEGACY_CSV: &str =
    include_str!("../../../../../examples/firmware/ota-system/partitions-s3-16mb-legacy.csv");
const WIDE_CSV: &str =
    include_str!("../../../../../examples/firmware/ota-system/partitions-s3-16mb-wide.csv");
const FLASH: u32 = 0x100_0000;

fn number(text: &str) -> u32 {
    let text = text.trim();
    text.strip_prefix("0x").map_or_else(
        || text.parse().unwrap(),
        |hex| u32::from_str_radix(hex, 16).unwrap(),
    )
}

/// (declared table offset, raw table as ESP-IDF's generator lays it out)
fn table(csv: &str) -> (u32, [u8; 0xc00]) {
    let declared = csv
        .lines()
        .find_map(|l| l.strip_prefix("# table-offset:"))
        .map(number)
        .expect("the file declares its table offset");
    let mut out = [0xffu8; 0xc00];
    let rows = csv
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'));
    for (i, row) in rows.enumerate() {
        let f: Vec<&str> = row.split(',').map(str::trim).collect();
        let kind = match f[1] {
            "data" => 1u8,
            "app" => 0,
            other => panic!("type {other}"),
        };
        let sub = match f[2] {
            "ota" => 0,
            "ota_0" => 0x10,
            "ota_1" => 0x11,
            other => number(other) as u8,
        };
        let e = &mut out[i * 32..(i + 1) * 32];
        e.fill(0);
        e[..4].copy_from_slice(&[0xaa, 0x50, kind, sub]);
        e[4..8].copy_from_slice(&number(f[3]).to_le_bytes());
        e[8..12].copy_from_slice(&number(f[4]).to_le_bytes());
        e[12..12 + f[0].len()].copy_from_slice(f[0].as_bytes());
    }
    (declared, out)
}

fn first_min(table_offset: u32) -> u32 {
    geometry::first_partition_min(table_offset)
}

#[test]
fn both_geometries_decode_and_keep_the_slots_and_config_in_place() {
    let (legacy_at, legacy) = table(LEGACY_CSV);
    let (wide_at, wide) = table(WIDE_CSV);
    assert_eq!((legacy_at, wide_at), (0x8000, 0x10000));
    let a = layout::discover_with(&legacy, FLASH, first_min(legacy_at)).unwrap();
    let b = layout::discover_with(&wide, FLASH, first_min(wide_at)).unwrap();
    // Slots do not move; only the metadata does.
    assert_eq!(a.images, b.images);
    assert_eq!(b.images[0].offset, 0x20000);
    assert_eq!(b.images[1].offset, 0x1d0000);
    assert_eq!((a.otadata.offset, b.otadata.offset), (0xd000, 0x11000));
    assert_eq!(a.otadata.size, b.otadata.size);
    assert_eq!(
        b.otadata.offset,
        first_min(wide_at),
        "otadata is the first partition"
    );
}

#[test]
fn proof_sectors_and_usable_capacity_are_identical_in_both_geometries() {
    let layouts = [table(LEGACY_CSV), table(WIDE_CSV)]
        .map(|(at, t)| layout::discover_with(&t, FLASH, first_min(at)).unwrap());
    for l in &layouts {
        for (slot, expected) in l.images.iter().zip([0x1cf000u32, 0x37f000]) {
            assert_eq!(slot.proof_offset(), expected);
            assert_eq!(slot.usable_size(), 0x1b0000 - domain_a::PROOF_SECTOR_SIZE);
        }
        assert!(SlotLayout::for_a(l, FLASH).is_ok());
    }
}

#[test]
fn a_table_crossing_the_bootloaders_geometry_is_refused() {
    let (_, legacy) = table(LEGACY_CSV);
    let (_, wide) = table(WIDE_CSV);
    // A bootloader built for the wide table must not accept metadata in front of it...
    assert!(layout::discover_with(&legacy, FLASH, first_min(0x10000)).is_err());
    // ...while the wide table is valid for either (it only starts later).
    assert!(layout::discover_with(&wide, FLASH, first_min(0x8000)).is_ok());
}

#[test]
fn the_boot_decision_is_the_same_wherever_the_metadata_sits() {
    // The decision is made on entries and slot indexes; the layout only names the flash offset.
    let entries = [
        Entry::new(1, state::VALID).encode(),
        Entry::new(2, state::NEW).encode(),
    ];
    for (at, t) in [table(LEGACY_CSV), table(WIDE_CSV)] {
        let l = layout::discover_with(&t, FLASH, first_min(at)).unwrap();
        let plan = plan_boot(entries, 2, &mut |_| true);
        match plan.boot {
            Boot::Slot { slot, seq, .. } => {
                assert_eq!((slot, seq), (1, 2));
                assert_eq!(l.images[usize::from(slot)].offset, 0x1d0000);
            }
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn this_build_uses_one_of_the_supported_geometries() {
    assert!([0x8000, 0x10000].contains(&geometry::TABLE_OFFSET));
    assert_eq!(
        geometry::FIRST_PARTITION_MIN,
        geometry::TABLE_OFFSET + 0x1000
    );
}
