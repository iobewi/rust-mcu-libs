use super::*;
use iobewi_workload_image::{ImageError, ImageHeader, esp32s3_layout};

const LAYOUT: TargetLayout = esp32s3_layout(RuntimeApi::new(1, 0), 1);

/// A minimal ELF32-LE with the given allocatable sections (no program headers needed).
fn elf(entry: u32, secs: &[(u32, &[u8], bool, u32)]) -> Vec<u8> {
    // (addr, bytes, nobits, nobits_size)
    let mut body = Vec::new();
    let mut shdrs: Vec<[u32; 10]> = vec![[0; 10]];
    let base = 52u32;
    for (addr, bytes, nobits, nsize) in secs {
        let off = base + body.len() as u32;
        let size = if *nobits { *nsize } else { bytes.len() as u32 };
        if !*nobits {
            body.extend_from_slice(bytes);
        }
        shdrs.push([
            0,
            if *nobits { 8 } else { 1 },
            2,
            *addr,
            off,
            size,
            0,
            0,
            4,
            0,
        ]);
    }
    let shoff = base + body.len() as u32;
    let mut out = vec![0u8; 52];
    out[0..4].copy_from_slice(b"\x7fELF");
    out[4] = 1;
    out[5] = 1;
    out[24..28].copy_from_slice(&entry.to_le_bytes());
    out[32..36].copy_from_slice(&shoff.to_le_bytes());
    out[46..48].copy_from_slice(&40u16.to_le_bytes());
    out[48..50].copy_from_slice(&(shdrs.len() as u16).to_le_bytes());
    out.extend_from_slice(&body);
    for s in shdrs {
        for w in s {
            out.extend_from_slice(&w.to_le_bytes());
        }
    }
    out
}

fn opts() -> PackOptions {
    PackOptions {
        layout: LAYOUT,
        requires: RuntimeApi::new(1, 0),
        overrides: Overrides::default(),
    }
}

#[test]
fn packs_code_data_and_bss_into_a_valid_deterministic_image() {
    let code = [1u8, 2, 3, 4, 5, 6, 7, 8];
    let e = elf(
        0x403D_3704,
        &[
            (0x403D_3700, &code, false, 0),
            (0x3FCE_8700, &[9, 9, 9, 9], false, 0),
            (0x3FCE_8704, &[], true, 16),
        ],
    );
    let a = pack(&e, &opts()).unwrap();
    let b = pack(&e, &opts()).unwrap();
    assert_eq!(a.bytes, b.bytes, "deterministic");
    let h = ImageHeader::decode(&a.bytes).unwrap();
    assert_eq!((h.code_size, h.data_size, h.bss_size), (8, 4, 16));
    assert_eq!(h.image_size as usize, a.bytes.len());
    assert_eq!(&a.bytes[64..72], &code);
    assert_eq!(&a.bytes[72..76], &[9, 9, 9, 9]);
    assert_eq!(h.validate(&LAYOUT, a.bytes.len() as u64), Ok(()));
}

#[test]
fn a_section_outside_the_regions_is_refused() {
    let e = elf(
        0x403D_3700,
        &[
            (0x403D_3700, &[0; 4], false, 0),
            (0x4200_0000, &[0; 4], false, 0),
        ],
    );
    assert!(matches!(
        pack(&e, &opts()),
        Err(PackError::OutsideRegions { .. })
    ));
}

#[test]
fn code_over_the_budget_and_bad_entry_are_refused_by_the_shared_gate() {
    let big = vec![0u8; LAYOUT.code_capacity as usize + 4];
    let e = elf(0x403D_3700, &[(0x403D_3700, &big, false, 0)]);
    assert!(matches!(
        pack(&e, &opts()),
        Err(PackError::OutsideRegions { .. } | PackError::Image(ImageError::TooLarge))
    ));
    let e = elf(0x403D_3702, &[(0x403D_3700, &[0; 8], false, 0)]);
    assert_eq!(
        pack(&e, &opts()).err(),
        Some(PackError::Image(ImageError::BadEntry))
    );
    let e = elf(0x4000_0000, &[(0x403D_3700, &[0; 8], false, 0)]);
    assert_eq!(
        pack(&e, &opts()).err(),
        Some(PackError::Image(ImageError::BadEntry))
    );
}

#[test]
fn not_an_elf_and_truncated_elfs_are_refused() {
    assert_eq!(pack(b"nope", &opts()).err(), Some(PackError::NotElf32Le));
    let mut e = elf(0x403D_3700, &[(0x403D_3700, &[0; 8], false, 0)]);
    e.truncate(60);
    assert_eq!(pack(&e, &opts()).err(), Some(PackError::Truncated));
}

#[test]
fn overrides_build_hostile_images_the_loader_gate_then_refuses() {
    let e = elf(0x403D_3700, &[(0x403D_3700, &[0; 8], false, 0)]);
    let mut o = opts();
    o.overrides.target = Some(2);
    let p = pack(&e, &o).unwrap();
    let h = ImageHeader::decode(&p.bytes).unwrap();
    assert!(matches!(
        h.validate(&LAYOUT, p.bytes.len() as u64),
        Err(ImageError::TargetMismatch { .. })
    ));
    let mut o = opts();
    o.overrides.magic = Some(*b"XXXX");
    assert_eq!(
        ImageHeader::decode(&pack(&e, &o).unwrap().bytes),
        Err(ImageError::BadMagic)
    );
    let mut o = opts();
    o.overrides.entry = Some(0x403D_3770);
    let p = pack(&e, &o).unwrap();
    assert_eq!(
        ImageHeader::decode(&p.bytes)
            .unwrap()
            .validate(&LAYOUT, p.bytes.len() as u64),
        Err(ImageError::BadEntry)
    );
}
