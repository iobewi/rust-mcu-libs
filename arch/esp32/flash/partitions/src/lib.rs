#![no_std]

//! ESP-IDF partition-table and raw partition helpers.
//!
//! Functions here are intentionally policy-free. Slot selection, rollback,
//! ConfigSpace ownership and firmware transactions remain in higher layers.

use embedded_storage::nor_flash::NorFlash;
use esp_bootloader_esp_idf::partitions::{
    PARTITION_TABLE_MAX_LEN, PartitionType, read_partition_table,
};
pub use esp_storage::FlashStorage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartitionRange {
    pub offset: u32,
    pub size: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionError {
    TableUnreadable,
    NotFound,
    AddressOverflow,
    Unaligned,
    Flash,
}

pub const TABLE_BUFFER_SIZE: usize = PARTITION_TABLE_MAX_LEN;

/// One raw partition-table entry, read without interpreting its subtype.
///
/// `esp-bootloader-esp-idf`'s typed accessors `unwrap` the subtype of every entry
/// they inspect, so a table holding an entry the typed enums do not know can make
/// a typed lookup panic. This view only compares raw bytes, so it never does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawEntry<'a> {
    pub label: &'a str,
    /// ESP-IDF raw type (`0` app, `1` data...).
    pub kind: u8,
    pub subtype: u8,
    pub offset: u32,
    pub size: u32,
}

/// Calls `visit` for every entry of the partition table, in table order.
pub fn for_each_entry(
    flash: &mut FlashStorage<'_>,
    table_buffer: &mut [u8; PARTITION_TABLE_MAX_LEN],
    mut visit: impl FnMut(RawEntry<'_>),
) -> Result<(), PartitionError> {
    let table =
        read_partition_table(flash, table_buffer).map_err(|_| PartitionError::TableUnreadable)?;
    for entry in table.iter() {
        visit(RawEntry {
            label: entry.label_as_str(),
            kind: entry.raw_type(),
            subtype: entry.raw_subtype(),
            offset: entry.offset(),
            size: entry.len(),
        });
    }
    Ok(())
}

/// Locates a partition by its **label** among entries of a given raw type and
/// subtype (for partitions that share a type: the Workload `data/undefined` ones).
pub fn find_by_label(
    flash: &mut FlashStorage<'_>,
    table_buffer: &mut [u8; PARTITION_TABLE_MAX_LEN],
    label: &str,
    kind: u8,
    subtype: u8,
) -> Result<PartitionRange, PartitionError> {
    let mut found = None;
    for_each_entry(flash, table_buffer, |entry| {
        if found.is_none() && entry.kind == kind && entry.subtype == subtype && entry.label == label
        {
            found = Some(PartitionRange {
                offset: entry.offset,
                size: entry.size as usize,
            });
        }
    })?;
    found.ok_or(PartitionError::NotFound)
}

/// Locate one partition of the requested ESP-IDF type.
pub fn find(
    flash: &mut FlashStorage<'_>,
    table_buffer: &mut [u8; PARTITION_TABLE_MAX_LEN],
    kind: PartitionType,
) -> Result<PartitionRange, PartitionError> {
    let table =
        read_partition_table(flash, table_buffer).map_err(|_| PartitionError::TableUnreadable)?;
    let entry = table
        .find_partition(kind)
        .map_err(|_| PartitionError::TableUnreadable)?
        .ok_or(PartitionError::NotFound)?;

    Ok(PartitionRange {
        offset: entry.offset(),
        size: entry.len() as usize,
    })
}

/// Erase a logical, erase-aligned range inside a previously located partition.
pub fn erase_range<F>(
    flash: &mut F,
    partition: PartitionRange,
    logical_from: u64,
    logical_to: u64,
) -> Result<(), PartitionError>
where
    F: NorFlash,
{
    if logical_to < logical_from {
        return Err(PartitionError::AddressOverflow);
    }

    let from = usize::try_from(logical_from).map_err(|_| PartitionError::AddressOverflow)?;
    let to = usize::try_from(logical_to).map_err(|_| PartitionError::AddressOverflow)?;

    if from % F::ERASE_SIZE != 0 || to % F::ERASE_SIZE != 0 {
        return Err(PartitionError::Unaligned);
    }
    if to > partition.size {
        return Err(PartitionError::AddressOverflow);
    }
    if from == to {
        return Ok(());
    }

    let base = usize::try_from(partition.offset).map_err(|_| PartitionError::AddressOverflow)?;
    let absolute_from = base
        .checked_add(from)
        .ok_or(PartitionError::AddressOverflow)?;
    let absolute_to = base
        .checked_add(to)
        .ok_or(PartitionError::AddressOverflow)?;

    flash
        .erase(
            u32::try_from(absolute_from).map_err(|_| PartitionError::AddressOverflow)?,
            u32::try_from(absolute_to).map_err(|_| PartitionError::AddressOverflow)?,
        )
        .map_err(|_| PartitionError::Flash)
}
