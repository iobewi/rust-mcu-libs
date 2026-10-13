//! Boot table discovery without a runtime flash driver.
use iobewi_firmware_layout::{Region, domain_a};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidLayout;
/// Validate all physical entries before selecting the historical A slots.
/// B/C partitions are checked geometrically but never boot-selected here.
pub fn discover(table: &[u8], flash_size: u32) -> Result<domain_a::Layout, InvalidLayout> {
    let entries = iobewi_esp_partitions::raw::entries(table).map_err(|_| InvalidLayout)?;
    let empty = domain_a::Entry {
        label: "",
        kind: 0,
        subtype: 0,
        partition: domain_a::Partition { offset: 0, size: 0 },
    };
    let mut selected = [empty; 3];
    let mut n = 0;
    let mut regions = [Region { offset: 0, size: 0 }; iobewi_esp_partitions::raw::MAX_LEN / 32];
    let mut count = 0;
    for e in entries {
        if e.offset < 0x9000 {
            return Err(InvalidLayout);
        }
        regions[count] = Region {
            offset: e.offset,
            size: e.size as usize,
        };
        count += 1;
        if [domain_a::OTA0, domain_a::OTA1, domain_a::OTADATA].contains(&e.label) {
            if n == 3 {
                return Err(InvalidLayout);
            }
            selected[n] = domain_a::Entry {
                label: e.label,
                kind: e.kind,
                subtype: e.subtype,
                partition: domain_a::Partition {
                    offset: e.offset,
                    size: e.size,
                },
            };
            n += 1;
        }
    }
    iobewi_firmware_layout::validate_regions(&regions[..count], flash_size, 0x1000)
        .map_err(|_| InvalidLayout)?;
    domain_a::discover(&selected[..n], flash_size).map_err(|_| InvalidLayout)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn table() -> [u8; 0xc00] {
        let mut out = [0xff; 0xc00];
        for (i, (name, kind, sub, offset, size)) in [
            ("otadata", 1, 0, 0xd000u32, 0x2000u32),
            ("ota_0", 0, 0x10, 0x20000, 0x180000),
            ("ota_1", 0, 0x11, 0x1a0000, 0x180000),
            ("b0", 1, 0x40, 0x320000, 0x100000),
            ("c0", 1, 0xfe, 0x800000, 0x800000),
        ]
        .into_iter()
        .enumerate()
        {
            let e = &mut out[i * 32..(i + 1) * 32];
            e.fill(0);
            e[..4].copy_from_slice(&[0xaa, 0x50, kind, sub]);
            e[4..8].copy_from_slice(&offset.to_le_bytes());
            e[8..12].copy_from_slice(&size.to_le_bytes());
            e[12..12 + name.len()].copy_from_slice(name.as_bytes());
        }
        out
    }
    #[test]
    fn selects_only_a_on_16_mib_with_custom_domains() {
        let t = table();
        let l = discover(&t, 0x1000000).unwrap();
        assert_eq!(l.images[0].offset, 0x20000);
        assert_eq!(l.otadata.offset, 0xd000);
        assert_eq!(discover(&t, 0x400000), Err(InvalidLayout));
    }
    #[test]
    fn refuses_ambiguous_or_dangerous_tables_before_boot_writes() {
        let mut t = table();
        t[3 * 32 + 4..3 * 32 + 8].copy_from_slice(&0x20000u32.to_le_bytes());
        assert_eq!(discover(&t, 0x1000000), Err(InvalidLayout));
        let mut t = table();
        t[3 * 32 + 12..3 * 32 + 28].fill(0);
        t[3 * 32 + 12..3 * 32 + 17].copy_from_slice(b"ota_0");
        assert_eq!(discover(&t, 0x1000000), Err(InvalidLayout));
        let mut t = table();
        t[4..8].copy_from_slice(&0x8000u32.to_le_bytes());
        assert_eq!(discover(&t, 0x1000000), Err(InvalidLayout));
    }
}
