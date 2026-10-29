//! Where the partition table sits, and what that implies for everything in front of it.
//!
//! One value, [`TABLE_OFFSET`], set when the firmware is built through
//! `ESP_BOOTLOADER_ESP_IDF_CONFIG_PARTITION_TABLE_OFFSET` (default `0x8000`). The second stage
//! reads the table there, the application finds it there (`esp-bootloader-esp-idf` takes the
//! same variable), and the first partition may not start before the sector that follows it.
//! The second stage's own image must end before the table.
//!
//! Nothing outside this module (and the tools that read the same variable) knows an absolute
//! flash address: partitions are found by label in the table.

include!(concat!(env!("OUT_DIR"), "/geometry.rs"));

/// The historical offset (ESP-IDF's default); the C3 reference keeps it.
pub const HISTORICAL_TABLE_OFFSET: u32 = 0x8000;
/// The table occupies one flash sector.
pub const TABLE_SECTOR: u32 = 0x1000;

/// First flash offset a partition may use for a table at `table_offset`.
pub const fn first_partition_min(table_offset: u32) -> u32 {
    table_offset + TABLE_SECTOR
}

/// Bytes available to the second stage image: everything in front of the table.
pub const fn bootloader_region(table_offset: u32) -> u32 {
    table_offset
}

/// First flash offset a partition may use with this build's geometry.
pub const FIRST_PARTITION_MIN: u32 = first_partition_min(TABLE_OFFSET);
/// Bytes available to the second stage image with this build's geometry.
pub const BOOTLOADER_REGION: u32 = bootloader_region(TABLE_OFFSET);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_partition_follows_the_table_sector() {
        assert_eq!(first_partition_min(0x8000), 0x9000);
        assert_eq!(first_partition_min(0x10000), 0x11000);
        assert_eq!(bootloader_region(0x10000), 0x10000);
    }

    #[test]
    fn this_builds_geometry_is_consistent_and_stamped() {
        assert_eq!(FIRST_PARTITION_MIN, TABLE_OFFSET + TABLE_SECTOR);
        assert_eq!(TABLE_OFFSET % 0x1000, 0);
        let text = core::str::from_utf8(&STAMP[..STAMP.len() - 1]).unwrap();
        assert_eq!(
            text,
            std::format!("IOBEWI-GEOMETRY table={TABLE_OFFSET:#010x}")
        );
    }
}
