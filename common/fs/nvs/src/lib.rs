#![no_std]

//! Chip-independent logic of the NVS-backed ConfigSpace persistence.
//!
//! The NVS backend of `iobewi-config-space` stores each space as one NVS blob
//! (`CSM1` header: magic, generation, presence flag, then the payload). The
//! record codec now lives in `iobewi_config_space::record`, and is re-exported
//! here to preserve existing consumer paths during the migration. This crate
//! retains NVS key rules, geometry and entry accounting only.
//!
//! # Capacity formula (unchanged)
//!
//! One version of a blob of `e` encoded bytes occupies
//! `ceil(e/32) + ceil(e/4000) + 1` NVS entries (data items, blob chunks,
//! index). A space reserves **two** versions (the old one stays until the new
//! one is committed): `2 × (ceil(e/32) + ceil(e/4000) + 1)` with
//! `e = 13 + budget`.
//!
//! Budgets arrive as plain byte counts. NVS is retained for legacy access.

pub use esp_nvs::{ENTRIES_PER_PAGE, ITEM_SIZE, MAX_BLOB_DATA_PER_PAGE};

pub use iobewi_config_space::record::{
    CorruptRecord, FLAG_PRESENT, HEADER_LEN, MAGIC, decode_record, encode_record,
};
/// NVS keys are at most 15 bytes.
pub const MAX_NVS_KEY_LEN: usize = 15;

/// A ConfigSpace name usable as an NVS key: non-empty, at most 15 bytes,
/// ASCII, no NUL.
pub fn valid_space_name(space: &str) -> bool {
    !space.is_empty()
        && space.len() <= MAX_NVS_KEY_LEN
        && space.as_bytes().iter().all(|b| b.is_ascii() && *b != 0)
}

/// NVS entries occupied by one stored version of a blob of `encoded_size`
/// bytes (`None` on overflow).
pub fn entries_for_blob(encoded_size: usize) -> Option<usize> {
    let data_entries = encoded_size.checked_add(ITEM_SIZE - 1)? / ITEM_SIZE;
    let chunks = encoded_size.checked_add(MAX_BLOB_DATA_PER_PAGE - 1)? / MAX_BLOB_DATA_PER_PAGE;
    data_entries.checked_add(chunks)?.checked_add(1)
}

/// NVS entries a space with `max_bytes` of payload must be able to hold:
/// two versions of its largest record.
pub fn reservation_units(max_bytes: usize) -> Option<usize> {
    let encoded_size = HEADER_LEN.checked_add(max_bytes)?;
    entries_for_blob(encoded_size)?.checked_mul(2)
}

/// Usable capacity of a partition, in NVS entries, from its statistics:
/// everything empty or erased, plus the entries this backend's own blobs
/// already occupy (those are neither empty nor erased, yet were counted in
/// each space's reservation), minus one page that NVS keeps in reserve.
pub fn capacity_units(empty: usize, erased: usize, owned: usize) -> usize {
    empty
        .saturating_add(erased)
        .saturating_add(owned)
        .saturating_sub(ENTRIES_PER_PAGE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn space_names_follow_the_nvs_key_rules() {
        assert!(valid_space_name("wifi"));
        assert!(valid_space_name("123456789012345")); // 15 bytes
        assert!(!valid_space_name(""));
        assert!(!valid_space_name("1234567890123456")); // 16 bytes
        assert!(!valid_space_name("caf\u{e9}"));
        assert!(!valid_space_name("a\0b"));
    }

    #[test]
    fn entry_accounting_matches_the_historical_formula() {
        // 2 x (ceil(e/32) + ceil(e/4000) + 1), e = 13 + budget.
        assert_eq!(ITEM_SIZE, 32);
        assert_eq!(MAX_BLOB_DATA_PER_PAGE, 4000);
        assert_eq!(entries_for_blob(0), Some(1));
        assert_eq!(entries_for_blob(32), Some(1 + 1 + 1));
        assert_eq!(entries_for_blob(33), Some(2 + 1 + 1));
        assert_eq!(entries_for_blob(4000), Some(125 + 1 + 1));
        assert_eq!(entries_for_blob(4001), Some(126 + 2 + 1));
        // Wi-Fi space: 128-byte budget -> e = 141 -> 5 + 1 + 1 = 7 -> reserves 14.
        assert_eq!(reservation_units(128), Some(14));
        // TLS space: 10 + 3 x 2048 = 6154 -> e = 6167 -> 193 + 2 + 1 = 196 -> 392.
        assert_eq!(reservation_units(10 + 3 * 2048), Some(392));
        assert_eq!(reservation_units(usize::MAX), None);
        assert_eq!(entries_for_blob(usize::MAX), None);
    }

    #[test]
    fn capacity_counts_owned_entries_back_and_keeps_one_page_in_reserve() {
        assert_eq!(capacity_units(500, 20, 0), 500 + 20 - ENTRIES_PER_PAGE);
        // Persisting a value moves entries from empty to owned: capacity must not shrink.
        let before = capacity_units(500, 0, 0);
        let after = capacity_units(500 - 14, 0, 14);
        assert_eq!(before, after);
        assert_eq!(capacity_units(10, 0, 0), 0, "saturates at zero");
    }
}

/// Invalid discovered NVS geometry. Validation performs no flash access.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionGeometryError {
    Unaligned,
    TooSmall,
    Overflow,
    OutOfBounds,
}
/// Admit at least two erase pages (one usable, one reserved), with checked bounds.
/// NVS content/open errors are handled by the existing backend, not by erasing.
pub fn validate_partition_geometry(
    offset: usize,
    size: usize,
    capacity: usize,
    erase_size: usize,
) -> Result<(), PartitionGeometryError> {
    if erase_size == 0 || !offset.is_multiple_of(erase_size) || !size.is_multiple_of(erase_size) {
        return Err(PartitionGeometryError::Unaligned);
    }
    if size / erase_size < 2 {
        return Err(PartitionGeometryError::TooSmall);
    }
    let end = offset
        .checked_add(size)
        .ok_or(PartitionGeometryError::Overflow)?;
    if end > capacity {
        return Err(PartitionGeometryError::OutOfBounds);
    }
    Ok(())
}
#[cfg(test)]
mod geometry_tests {
    use super::*;
    #[test]
    fn discovered_geometry_is_bounded_and_aligned() {
        assert_eq!(validate_partition_geometry(4096, 8192, 16384, 4096), Ok(()));
        assert_eq!(
            validate_partition_geometry(4097, 8192, 16384, 4096),
            Err(PartitionGeometryError::Unaligned)
        );
        assert_eq!(
            validate_partition_geometry(4096, 4096, 16384, 4096),
            Err(PartitionGeometryError::TooSmall)
        );
        assert_eq!(
            validate_partition_geometry(4096, 8192, 8192, 4096),
            Err(PartitionGeometryError::OutOfBounds)
        );
        assert_eq!(
            validate_partition_geometry(0, 8192, 16384, 0),
            Err(PartitionGeometryError::Unaligned)
        );
        assert_eq!(
            validate_partition_geometry(usize::MAX - 1, 2, usize::MAX, 1),
            Err(PartitionGeometryError::Overflow)
        );
    }
}
