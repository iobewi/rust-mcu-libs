#![no_std]
//! Host-testable layout validation shared by ESP adapters.
//! No flash driver, boot decision, or partition-table parser.
pub mod authorization;
pub mod domain_a;
pub mod domain_b;
pub mod session;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry<'a> {
    pub label: &'a str,
    pub kind: u8,
    pub subtype: u8,
    pub offset: u32,
    pub size: u32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub offset: u32,
    pub size: usize,
}

impl Region {
    /// Whether `len` bytes starting at flash offset `absolute` lie entirely inside the region.
    pub const fn contains_range(&self, absolute: u32, len: usize) -> bool {
        absolute >= self.offset
            && (absolute as u64 + len as u64) <= (self.offset as u64 + self.size as u64)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Requirement<'a> {
    pub label: &'a str,
    pub kind: u8,
    pub subtype: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryError {
    TableUnreadable,
    Missing,
    Duplicate,
    Misaligned,
    OutOfFlash,
    Overlap,
    Overflow,
}

/// Validate all supplied physical regions, including partitions not selected
/// as update targets. Platform-specific placement constraints remain in arch/.
pub fn validate_regions(
    regions: &[Region],
    flash_size: u32,
    erase_size: u32,
) -> Result<(), DiscoveryError> {
    if erase_size == 0 {
        return Err(DiscoveryError::Misaligned);
    }
    for (i, region) in regions.iter().enumerate() {
        let size = u32::try_from(region.size).map_err(|_| DiscoveryError::Overflow)?;
        if size == 0 || region.offset % erase_size != 0 || size % erase_size != 0 {
            return Err(DiscoveryError::Misaligned);
        }
        let end = region
            .offset
            .checked_add(size)
            .ok_or(DiscoveryError::Overflow)?;
        if end > flash_size {
            return Err(DiscoveryError::OutOfFlash);
        }
        for other in &regions[..i] {
            if (region.offset as u64) < other.offset as u64 + other.size as u64
                && (other.offset as u64) < end as u64
            {
                return Err(DiscoveryError::Overlap);
            }
        }
    }
    Ok(())
}

/// Check a provided partition-table view, shared by the production reader and host tests.
/// Only regions named in `required` are returned, in caller-specified order.
/// A second physical entry matching a requested label is always an error,
/// even if its type/subtype differs: duplicate names cannot select a safe target.
pub fn validate<'a, const N: usize>(
    entries: impl IntoIterator<Item = Entry<'a>>,
    required: &[Requirement<'_>; N],
    flash_size: u32,
    erase_size: u32,
) -> Result<[Region; N], DiscoveryError> {
    if erase_size == 0
        || required
            .iter()
            .enumerate()
            .any(|(i, r)| required[..i].iter().any(|other| other.label == r.label))
    {
        return Err(DiscoveryError::Duplicate);
    }
    let mut matches: [Option<Region>; N] = [None; N];
    for entry in entries {
        for (i, req) in required.iter().enumerate() {
            if entry.label != req.label {
                continue;
            }
            if matches[i].is_some() {
                return Err(DiscoveryError::Duplicate);
            }
            if entry.kind != req.kind || entry.subtype != req.subtype {
                return Err(DiscoveryError::Missing);
            }
            if entry.size == 0 || entry.offset % erase_size != 0 || entry.size % erase_size != 0 {
                return Err(DiscoveryError::Misaligned);
            }
            let end = entry
                .offset
                .checked_add(entry.size)
                .ok_or(DiscoveryError::Overflow)?;
            if end > flash_size {
                return Err(DiscoveryError::OutOfFlash);
            }
            matches[i] = Some(Region {
                offset: entry.offset,
                size: entry.size as usize,
            });
        }
    }
    let mut result = [Region { offset: 0, size: 0 }; N];
    for (i, value) in matches.iter().enumerate() {
        result[i] = value.ok_or(DiscoveryError::Missing)?;
    }
    for i in 0..N {
        let a = &result[i];
        let a_end = (a.offset as u64) + a.size as u64;
        for b in &result[i + 1..] {
            let b_end = (b.offset as u64) + b.size as u64;
            if (a.offset as u64) < b_end && (b.offset as u64) < a_end {
                return Err(DiscoveryError::Overlap);
            }
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    const A: [Requirement<'static>; 2] = [
        Requirement {
            label: "ota_0",
            kind: 0,
            subtype: 0x10,
        },
        Requirement {
            label: "ota_1",
            kind: 0,
            subtype: 0x11,
        },
    ];
    fn entry(label: &'static str, subtype: u8, offset: u32, size: u32) -> Entry<'static> {
        Entry {
            label,
            kind: 0,
            subtype,
            offset,
            size,
        }
    }
    #[test]
    fn accepts_two_distinct_aligned_slots() {
        let out = validate(
            [
                entry("ota_0", 0x10, 0x20000, 0x10000),
                entry("ota_1", 0x11, 0x30000, 0x10000),
            ],
            &A,
            0x100000,
            0x1000,
        )
        .unwrap();
        assert_eq!(out[1].offset, 0x30000);
    }
    #[test]
    fn rejects_missing_duplicate_wrong_type_and_overlap() {
        assert_eq!(
            validate(
                [entry("ota_0", 0x10, 0x20000, 0x10000)],
                &A,
                0x100000,
                0x1000
            ),
            Err(DiscoveryError::Missing)
        );
        assert_eq!(
            validate(
                [
                    entry("ota_0", 0x10, 0x20000, 0x10000),
                    entry("ota_0", 0x10, 0x30000, 0x10000)
                ],
                &A,
                0x100000,
                0x1000
            ),
            Err(DiscoveryError::Duplicate)
        );
        assert_eq!(
            validate(
                [
                    entry("ota_0", 0x11, 0x20000, 0x10000),
                    entry("ota_1", 0x11, 0x30000, 0x10000)
                ],
                &A,
                0x100000,
                0x1000
            ),
            Err(DiscoveryError::Missing)
        );
        assert_eq!(
            validate(
                [
                    entry("ota_0", 0x10, 0x20000, 0x20000),
                    entry("ota_1", 0x11, 0x30000, 0x10000)
                ],
                &A,
                0x100000,
                0x1000
            ),
            Err(DiscoveryError::Overlap)
        );
    }
    #[test]
    fn rejects_unaligned_out_of_flash_and_overflow() {
        assert_eq!(
            validate(
                [
                    entry("ota_0", 0x10, 0x20001, 0x10000),
                    entry("ota_1", 0x11, 0x30000, 0x10000)
                ],
                &A,
                0x100000,
                0x1000
            ),
            Err(DiscoveryError::Misaligned)
        );
        assert_eq!(
            validate(
                [
                    entry("ota_0", 0x10, 0x20000, 0x10000),
                    entry("ota_1", 0x11, 0xf0000, 0x20000)
                ],
                &A,
                0x100000,
                0x1000
            ),
            Err(DiscoveryError::OutOfFlash)
        );
        assert_eq!(
            validate(
                [
                    entry("ota_0", 0x10, 0xfffff000, 0x2000),
                    entry("ota_1", 0x11, 0x30000, 0x10000)
                ],
                &A,
                u32::MAX,
                0x1000
            ),
            Err(DiscoveryError::Overflow)
        );
    }
    #[test]
    fn validates_entire_16_mib_map_including_unselected_domains_and_config() {
        let mut regions = [
            Region {
                offset: 0x9000,
                size: 0x2000,
            },
            Region {
                offset: 0x20000,
                size: 0x180000,
            },
            Region {
                offset: 0x1a0000,
                size: 0x180000,
            },
            Region {
                offset: 0x320000,
                size: 0x2000,
            },
            Region {
                offset: 0x330000,
                size: 0x40000,
            },
            Region {
                offset: 0x370000,
                size: 0x40000,
            },
            Region {
                offset: 0x3b0000,
                size: 0x2000,
            },
            Region {
                offset: 0x400000,
                size: 0x400000,
            },
            Region {
                offset: 0x800000,
                size: 0x400000,
            },
            Region {
                offset: 0xc00000,
                size: 0x400000,
            },
        ];
        assert_eq!(validate_regions(&regions, 0x1000000, 0x1000), Ok(()));
        regions[9].offset -= 0x1000;
        assert_eq!(
            validate_regions(&regions, 0x1000000, 0x1000),
            Err(DiscoveryError::Overlap)
        );
        regions[9].offset = 0xc01000;
        assert_eq!(
            validate_regions(&regions, 0x1000000, 0x1000),
            Err(DiscoveryError::OutOfFlash)
        );
    }

    #[test]
    fn a_region_contains_only_ranges_that_end_inside_it() {
        let r = Region {
            offset: 0x1cf000,
            size: 0x1000,
        };
        assert!(r.contains_range(0x1cf000, 0x1000));
        assert!(r.contains_range(0x1cf010, 4));
        for (at, len) in [
            (0x1ce000u32, 4usize),
            (0x1cf000 - 4, 8),
            (0x1d0000, 4),
            (0x1cf000, 0x1001),
            (u32::MAX - 1, 8),
        ] {
            assert!(!r.contains_range(at, len), "{at:#x}+{len}");
        }
    }
}
