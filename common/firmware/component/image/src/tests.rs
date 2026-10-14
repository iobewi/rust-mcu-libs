use super::*;
use std::vec::Vec;

const LAYOUT: TargetLayout = TargetLayout {
    target: Target::Esp32s3,
    code_addr: 0x403D_3700,
    code_capacity: 0x5000,
    data_addr: 0x3FCE_8700,
    data_capacity: 0x3000,
    abi_version: 1,
    provides: RuntimeApi::new(1, 0),
};

fn good() -> ImageHeader {
    ImageHeader {
        target: 1,
        abi_version: 1,
        requires: RuntimeApi::new(1, 0),
        flags: 0,
        image_size: 64 + 0x100 + 0x40,
        entry: 0x403D_3700,
        code_addr: 0x403D_3700,
        code_offset: 64,
        code_size: 0x100,
        data_addr: 0x3FCE_8700,
        data_offset: 64 + 0x100,
        data_size: 0x40,
        bss_size: 0x20,
    }
}

fn size(h: &ImageHeader) -> u64 {
    u64::from(h.image_size)
}

#[test]
fn a_good_image_round_trips_and_validates() {
    let h = good();
    let bytes = h.encode();
    assert_eq!(ImageHeader::decode(&bytes), Ok(h));
    assert_eq!(h.validate(&LAYOUT, size(&h)), Ok(()));
    assert_eq!(h.ram_footprint(), 0x100 + 0x40 + 0x20);
}

#[test]
fn header_offsets_are_the_documented_ones() {
    let b = good().encode();
    assert_eq!(&b[0..4], b"IWNI");
    assert_eq!(u16::from_le_bytes([b[4], b[5]]), 1, "format version @4");
    assert_eq!(u16::from_le_bytes([b[6], b[7]]), 64, "header len @6");
    assert_eq!(u16::from_le_bytes([b[8], b[9]]), 1, "target @8");
    assert_eq!(u16::from_le_bytes([b[10], b[11]]), 1, "abi @10");
    assert_eq!(u16::from_le_bytes([b[12], b[13]]), 1, "api major @12");
    assert_eq!(u16::from_le_bytes([b[14], b[15]]), 0, "api minor @14");
    assert_eq!(
        u32::from_le_bytes(b[24..28].try_into().unwrap()),
        0x403D_3700,
        "entry @24"
    );
    assert_eq!(
        u32::from_le_bytes(b[36..40].try_into().unwrap()),
        0x100,
        "code size @36"
    );
    assert_eq!(
        u32::from_le_bytes(b[52..56].try_into().unwrap()),
        0x20,
        "bss size @52"
    );
    assert!(b[56..].iter().all(|&x| x == 0));
    assert_eq!(b.len(), HEADER_LEN);
}

#[test]
fn bad_magic_version_and_length_are_refused() {
    let mut b = good().encode();
    b[0] = b'X';
    assert_eq!(ImageHeader::decode(&b), Err(ImageError::BadMagic));
    let mut b = good().encode();
    b[4] = 2;
    assert_eq!(
        ImageHeader::decode(&b),
        Err(ImageError::BadFormatVersion(2))
    );
    let mut b = good().encode();
    b[6] = 80;
    assert_eq!(ImageHeader::decode(&b), Err(ImageError::BadHeaderLen(80)));
    assert_eq!(
        ImageHeader::decode(&good().encode()[..63]),
        Err(ImageError::Truncated)
    );
    assert_eq!(ImageHeader::decode(&[]), Err(ImageError::Truncated));
}

#[test]
fn reserved_and_flags_must_be_zero() {
    let mut b = good().encode();
    b[63] = 1;
    assert_eq!(ImageHeader::decode(&b), Err(ImageError::ReservedNotZero));
    let mut h = good();
    h.flags = 1;
    assert_eq!(
        ImageHeader::decode(&h.encode()),
        Err(ImageError::UnknownFlags(1))
    );
}

#[test]
fn wrong_target_abi_or_runtime_api_refuses_activation() {
    let mut h = good();
    h.target = 2; // C3 image on an S3 device
    assert_eq!(
        h.validate(&LAYOUT, size(&h)),
        Err(ImageError::TargetMismatch {
            image: 2,
            device: 1
        })
    );
    h.target = 77;
    assert_eq!(
        h.validate(&LAYOUT, size(&h)),
        Err(ImageError::UnknownTarget(77))
    );
    let mut h = good();
    h.abi_version = 2;
    assert_eq!(
        h.validate(&LAYOUT, size(&h)),
        Err(ImageError::AbiMismatch {
            image: 2,
            device: 1
        })
    );
    let mut h = good();
    h.requires = RuntimeApi::new(1, 1);
    assert!(matches!(
        h.validate(&LAYOUT, size(&h)),
        Err(ImageError::RuntimeApiMismatch { .. })
    ));
    h.requires = RuntimeApi::new(2, 0);
    assert!(matches!(
        h.validate(&LAYOUT, size(&h)),
        Err(ImageError::RuntimeApiMismatch { .. })
    ));
}

#[test]
fn size_must_match_what_ota_stored() {
    let h = good();
    assert_eq!(
        h.validate(&LAYOUT, size(&h) + 1),
        Err(ImageError::SizeMismatch {
            header: h.image_size,
            artifact: size(&h) + 1
        })
    );
}

#[test]
fn bounds_are_checked_with_overflow_safety() {
    let mut h = good();
    h.code_offset = u32::MAX - 10;
    assert_eq!(h.validate(&LAYOUT, size(&h)), Err(ImageError::BadBounds));
    let mut h = good();
    h.data_offset = u32::MAX - 4;
    assert_eq!(h.validate(&LAYOUT, size(&h)), Err(ImageError::BadBounds));
    let mut h = good();
    h.code_offset = 10; // overlaps the header
    assert_eq!(h.validate(&LAYOUT, size(&h)), Err(ImageError::BadBounds));
    let mut h = good();
    h.code_size = h.image_size; // runs past the image
    assert_eq!(h.validate(&LAYOUT, size(&h)), Err(ImageError::BadBounds));
    let mut h = good();
    h.data_offset = 64; // data overlaps code
    assert_eq!(h.validate(&LAYOUT, size(&h)), Err(ImageError::BadBounds));
    let mut h = good();
    h.bss_size = u32::MAX; // data + bss overflows
    assert_eq!(h.validate(&LAYOUT, size(&h)), Err(ImageError::BadBounds));
    let mut h = good();
    h.code_addr = u32::MAX - 3;
    h.entry = h.code_addr;
    assert_eq!(h.validate(&LAYOUT, size(&h)), Err(ImageError::AddrMismatch));
}

#[test]
fn addresses_must_be_the_loaders_fixed_link_layout() {
    let mut h = good();
    h.code_addr += 4;
    h.entry += 4;
    assert_eq!(h.validate(&LAYOUT, size(&h)), Err(ImageError::AddrMismatch));
    let mut h = good();
    h.data_addr += 4;
    assert_eq!(h.validate(&LAYOUT, size(&h)), Err(ImageError::AddrMismatch));
}

#[test]
fn regions_must_fit_the_workload_ram_budget() {
    let mut h = good();
    h.code_size = LAYOUT.code_capacity + 4;
    h.image_size = 64 + h.code_size + h.data_size;
    h.data_offset = 64 + h.code_size;
    assert_eq!(h.validate(&LAYOUT, size(&h)), Err(ImageError::TooLarge));
    let mut h = good();
    h.bss_size = LAYOUT.data_capacity;
    assert_eq!(h.validate(&LAYOUT, size(&h)), Err(ImageError::TooLarge));
    let mut h = good();
    h.code_size = 0;
    assert_eq!(h.validate(&LAYOUT, size(&h)), Err(ImageError::EmptyCode));
}

#[test]
fn entry_must_be_inside_the_provided_code_and_aligned() {
    for entry in [
        0u32,
        0x403D_36FC,
        0x403D_3700 + 0x100,
        0x403D_3702,
        0xFFFF_FFFC,
        0x3FCE_8700,
    ] {
        let mut h = good();
        h.entry = entry;
        assert_eq!(
            h.validate(&LAYOUT, size(&h)),
            Err(ImageError::BadEntry),
            "entry {entry:#x}"
        );
    }
    let mut h = good();
    h.entry = 0x403D_3700 + 0xFC; // last aligned word of the code
    assert_eq!(h.validate(&LAYOUT, size(&h)), Ok(()));
}

#[test]
fn every_error_has_a_stable_reason() {
    let reasons: Vec<&str> = [
        ImageError::Truncated,
        ImageError::BadMagic,
        ImageError::BadFormatVersion(0),
        ImageError::BadHeaderLen(0),
        ImageError::UnknownTarget(0),
        ImageError::TargetMismatch {
            image: 0,
            device: 0,
        },
        ImageError::AbiMismatch {
            image: 0,
            device: 0,
        },
        ImageError::RuntimeApiMismatch {
            required: RuntimeApi::new(0, 0),
            provided: RuntimeApi::new(0, 0),
        },
        ImageError::UnknownFlags(0),
        ImageError::ReservedNotZero,
        ImageError::SizeMismatch {
            header: 0,
            artifact: 0,
        },
        ImageError::BadBounds,
        ImageError::EmptyCode,
        ImageError::AddrMismatch,
        ImageError::TooLarge,
        ImageError::BadEntry,
    ]
    .iter()
    .map(ImageError::reason)
    .collect();
    let mut sorted = reasons.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), reasons.len(), "reasons are distinct");
}

#[test]
fn the_s3_constants_match_the_linker_script_and_the_bus_alias() {
    let ld = include_str!("../../sdk/ld/esp32s3-v1.ld");
    assert!(
        ld.contains(&std::format!(
            "ORIGIN = {:#X}, LENGTH = {:#X}",
            ESP32S3_CODE_ADDR,
            ESP32S3_CODE_CAPACITY
        )),
        "CODE line"
    );
    assert!(
        ld.contains(&std::format!(
            "ORIGIN = {:#X}, LENGTH = {:#X}",
            ESP32S3_DATA_ADDR,
            ESP32S3_DATA_CAPACITY
        )),
        "DATA line"
    );
    // code (ibus) and region (dbus) are the same physical bytes; data follows the code.
    assert_eq!(
        ESP32S3_CODE_ADDR - ESP32S3_IBUS_MINUS_DBUS,
        ESP32S3_REGION_DBUS
    );
    assert_eq!(
        ESP32S3_REGION_DBUS + ESP32S3_CODE_CAPACITY,
        ESP32S3_DATA_ADDR
    );
    assert_eq!(
        ESP32S3_DATA_ADDR + ESP32S3_DATA_CAPACITY,
        ESP32S3_REGION_DBUS + ESP32S3_REGION_SIZE
    );
    // The whole region stays inside the reclaimed dram2 area (ends 0x3FCEB700) and the
    // instruction-bus alias of SRAM1 (ends 0x403E0000).
    const { assert!(ESP32S3_REGION_DBUS + ESP32S3_REGION_SIZE <= 0x3FCE_B700) };
    const { assert!(ESP32S3_CODE_ADDR + ESP32S3_CODE_CAPACITY <= 0x403E_0000) };
    assert_eq!(
        esp32s3_layout(RuntimeApi::new(1, 0), 1).target,
        Target::Esp32s3
    );
}
