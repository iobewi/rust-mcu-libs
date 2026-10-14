//! Physical Workload regions and the capability they define. Pure: no flash
//! access, no platform. A platform locates its partitions (by name) and hands
//! the regions to [`assemble`]; anything that does not form a safe dual-slot
//! layout is [`Unsupported`] -- never a guess at "free" space.
//!
//! Names (`wl_meta`, `workload_a`, `workload_b`) are the contract with the
//! partition table; offsets and sizes live **only** in the table.

/// Physical side of the historical B-domain layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    A,
    B,
}

pub const LABEL_META: &str = "wl_meta";
pub const LABEL_SLOT_A: &str = "workload_a";
pub const LABEL_SLOT_B: &str = "workload_b";

/// Smallest slot that still makes a Workload useful (small Pod binary + its
/// metadata). Below this a device is `Unsupported` rather than given a toy slot.
pub const MIN_WORKLOAD_SLOT_SIZE: u32 = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub offset: u32,
    pub size: u32,
}

impl Region {
    pub const fn new(offset: u32, size: u32) -> Self {
        Self { offset, size }
    }

    pub const fn end(&self) -> u64 {
        self.offset as u64 + self.size as u64
    }

    pub const fn overlaps(&self, other: &Region) -> bool {
        (self.offset as u64) < other.end() && (other.offset as u64) < self.end()
    }
}

/// Why a device has no Workload storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unsupported {
    MissingMeta,
    MissingSlot(Side),
    /// `wl_meta` cannot hold two copies in two separate erase units.
    MetaTooSmall,
    Misaligned,
    /// A slot is smaller than [`MIN_WORKLOAD_SLOT_SIZE`].
    SlotTooSmall,
    /// The two slots must have the same size.
    SlotsDiffer,
    Overlap,
    Overflow,
}

/// A validated dual-slot Workload layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkloadLayout {
    meta: Region,
    slots: [Region; 2],
    erase_size: u32,
}

/// Validates the three regions found in the partition table.
pub fn assemble(
    meta: Option<Region>,
    slot_a: Option<Region>,
    slot_b: Option<Region>,
    erase_size: u32,
) -> Result<WorkloadLayout, Unsupported> {
    let meta = meta.ok_or(Unsupported::MissingMeta)?;
    let a = slot_a.ok_or(Unsupported::MissingSlot(Side::A))?;
    let b = slot_b.ok_or(Unsupported::MissingSlot(Side::B))?;
    if erase_size == 0 {
        return Err(Unsupported::Misaligned);
    }
    let metadata_min = erase_size.checked_mul(2).ok_or(Unsupported::Overflow)?;
    if meta.size < metadata_min {
        return Err(Unsupported::MetaTooSmall);
    }
    for region in [meta, a, b] {
        region
            .offset
            .checked_add(region.size)
            .ok_or(Unsupported::Overflow)?;
        if region.offset % erase_size != 0 || region.size % erase_size != 0 {
            return Err(Unsupported::Misaligned);
        }
    }
    if a.size < MIN_WORKLOAD_SLOT_SIZE || b.size < MIN_WORKLOAD_SLOT_SIZE {
        return Err(Unsupported::SlotTooSmall);
    }
    if a.size != b.size {
        return Err(Unsupported::SlotsDiffer);
    }
    if meta.overlaps(&a) || meta.overlaps(&b) || a.overlaps(&b) {
        return Err(Unsupported::Overlap);
    }
    Ok(WorkloadLayout {
        meta,
        slots: [a, b],
        erase_size,
    })
}

impl WorkloadLayout {
    pub fn meta(&self) -> Region {
        self.meta
    }

    pub fn slot(&self, side: Side) -> Region {
        self.slots[match side {
            Side::A => 0,
            Side::B => 1,
        }]
    }

    pub fn erase_size(&self) -> u32 {
        self.erase_size
    }

    /// Largest artifact a slot can hold.
    pub fn max_artifact_size(&self) -> u32 {
        self.slots[0].size
    }

    /// Offset, inside `wl_meta`, of OTM2 copy `copy` (one erase unit each, so a
    /// sector erase can never destroy both copies).
    pub fn meta_copy_offset(&self, copy: usize) -> u32 {
        copy as u32 * self.erase_size
    }

    /// Every region the Workload backend may ever write.
    pub fn regions(&self) -> [Region; 3] {
        [self.meta, self.slots[0], self.slots[1]]
    }
}

// ---------------------------------------------------------------------------
// Partition-table sanity checks (used by tests and by tooling that mirrors the
// real table; the runtime never trusts a table it cannot validate).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    App,
    Data,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartitionSpec {
    pub name: &'static str,
    pub kind: Kind,
    pub offset: u32,
    pub size: u32,
}

impl PartitionSpec {
    pub const fn region(&self) -> Region {
        Region {
            offset: self.offset,
            size: self.size,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableError {
    /// Two entries overlap (indices into the table).
    Overlap(usize, usize),
    OutOfFlash(usize),
    /// App partitions need 64 KiB alignment, data partitions 4 KiB.
    Misaligned(usize),
    /// Starts before the end of bootloader + partition table (0x9000).
    BelowFirstPartition(usize),
    /// An Agent partition differs from its reference definition.
    AgentChanged(&'static str),
}

pub const FIRST_PARTITION_OFFSET: u32 = 0x9000;
pub const APP_ALIGN: u32 = 0x1_0000;
pub const DATA_ALIGN: u32 = 0x1000;

pub fn check_table(
    table: &[PartitionSpec],
    flash_size: u32,
    agent_reference: &[PartitionSpec],
) -> Result<(), TableError> {
    for (i, p) in table.iter().enumerate() {
        if p.offset < FIRST_PARTITION_OFFSET {
            return Err(TableError::BelowFirstPartition(i));
        }
        let align = if p.kind == Kind::App {
            APP_ALIGN
        } else {
            DATA_ALIGN
        };
        if p.offset % align != 0 || p.size % DATA_ALIGN != 0 {
            return Err(TableError::Misaligned(i));
        }
        if p.region().end() > u64::from(flash_size) {
            return Err(TableError::OutOfFlash(i));
        }
        for (j, q) in table.iter().enumerate().skip(i + 1) {
            if p.region().overlaps(&q.region()) {
                return Err(TableError::Overlap(i, j));
            }
        }
    }
    for reference in agent_reference {
        if !table.iter().any(|p| p == reference) {
            return Err(TableError::AgentChanged(reference.name));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn r(offset: u32, size: u32) -> Option<Region> {
        Some(Region::new(offset, size))
    }
    #[test]
    fn accepts_historical_three_partition_layout_and_separate_metadata_copies() {
        let l = assemble(
            r(0x9000, 0x2000),
            r(0x20000, 0x40000),
            r(0x60000, 0x40000),
            0x1000,
        )
        .unwrap();
        assert_eq!(l.meta_copy_offset(0), 0);
        assert_eq!(l.meta_copy_offset(1), 0x1000);
        assert_eq!(l.slot(Side::B).offset, 0x60000);
    }
    #[test]
    fn fails_closed_on_missing_metadata_slots_and_too_small_regions() {
        assert_eq!(
            assemble(None, r(0x20000, 0x40000), r(0x60000, 0x40000), 0x1000),
            Err(Unsupported::MissingMeta)
        );
        assert_eq!(
            assemble(r(0x9000, 0x2000), None, r(0x60000, 0x40000), 0x1000),
            Err(Unsupported::MissingSlot(Side::A))
        );
        assert_eq!(
            assemble(
                r(0x9000, 0x1000),
                r(0x20000, 0x40000),
                r(0x60000, 0x40000),
                0x1000
            ),
            Err(Unsupported::MetaTooSmall)
        );
        assert_eq!(
            assemble(
                r(0x9000, 0x2000),
                r(0x20000, 0x10000),
                r(0x60000, 0x10000),
                0x1000
            ),
            Err(Unsupported::SlotTooSmall)
        );
    }
    #[test]
    fn rejects_overlap_size_mismatch_and_alignment() {
        assert_eq!(
            assemble(
                r(0x9000, 0x2000),
                r(0x20000, 0x40000),
                r(0x50000, 0x40000),
                0x1000
            ),
            Err(Unsupported::Overlap)
        );
        assert_eq!(
            assemble(
                r(0x9000, 0x2000),
                r(0x20000, 0x40000),
                r(0x60000, 0x50000),
                0x1000
            ),
            Err(Unsupported::SlotsDiffer)
        );
        assert_eq!(
            assemble(
                r(0x9001, 0x2000),
                r(0x20000, 0x40000),
                r(0x60000, 0x40000),
                0x1000
            ),
            Err(Unsupported::Misaligned)
        );
    }
    #[test]
    fn rejects_address_and_metadata_geometry_overflow() {
        assert_eq!(
            assemble(
                r(0x9000, 0x2000),
                r(0xffff0000, 0x40000),
                r(0x60000, 0x40000),
                0x1000
            ),
            Err(Unsupported::Overflow)
        );
        assert_eq!(
            assemble(
                r(0, u32::MAX),
                r(0x20000, 0x40000),
                r(0x60000, 0x40000),
                0x80000000
            ),
            Err(Unsupported::Overflow)
        );
    }
}
