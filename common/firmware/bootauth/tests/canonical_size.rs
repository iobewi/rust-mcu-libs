//! #104: a signed length must be exactly the length of the image the loader will load.

mod common;

use common::*;
use iobewi_firmware_boot::{Boot, Entry, Raw, state};
use iobewi_firmware_bootauth::select::{Outcome, Structure, plan};
use iobewi_firmware_bootauth::{Refusal, UnsignedBoot};

fn two_valid() -> [Raw; 2] {
    [
        Entry::new(1, state::VALID).encode(),
        Entry::new(2, state::VALID).encode(),
    ]
}

/// Slot 1 (highest sequence, examined first) holds an image of `len` bytes signed over
/// `signed` bytes; the structure stage reports `canonical` for it. Slot 0 is a sound 4000-byte image.
fn run(len: usize, signed: usize, canonical: u32) -> (Boot, Vec<(u8, Outcome)>, usize) {
    let mut flash = MemFlash::new();
    good_slot(&mut flash, SLOTS[0], &image(4000, 1));
    let full = image(len, 2);
    install(
        &mut flash,
        SLOTS[1],
        &full[..signed],
        &manifest(key_a(), &full[..signed]),
        key_a(),
        &key_a().public_struct,
    );
    flash.poke(SLOTS[1].offset, &full); // the whole image is in flash, only `signed` bytes are covered
    let mut report = Vec::new();
    let plan = plan(
        two_valid(),
        2,
        &verified(),
        &mut flash,
        SLOTS,
        &mut |slot| Structure::Bootable {
            canonical_size: if slot == 1 { canonical } else { 4000 },
        },
        &mut |s, o| report.push((s, o)),
    )
    .unwrap();
    let writes = plan.writes().count();
    (plan.boot, report, writes)
}

fn slot_of(boot: Boot) -> u8 {
    match boot {
        Boot::Slot { slot, .. } => slot,
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_image_signed_over_its_exact_canonical_size_is_admitted() {
    let (boot, report, _) = run(6000, 6000, 6000);
    assert_eq!(slot_of(boot), 1);
    assert!(report.is_empty());
}

#[test]
fn a_proof_covering_only_a_prefix_of_the_image_is_refused_and_the_other_slot_boots() {
    // 1 MiB-boundary shape at test scale: the image is longer than what the proof covers.
    let (boot, report, writes) = run(6000, 4096, 6000);
    assert_eq!(slot_of(boot), 0);
    assert_eq!(report, vec![(1, Outcome::Proof(Refusal::BadSize))]);
    assert_eq!(
        writes, 1,
        "the refusal is certain, so it is recorded as Invalid"
    );
}

#[test]
fn a_proof_covering_more_than_the_canonical_image_is_refused_too() {
    let (boot, report, _) = run(6000, 6000, 5000);
    assert_eq!(slot_of(boot), 0);
    assert_eq!(report, vec![(1, Outcome::Proof(Refusal::BadSize))]);
}

#[test]
fn a_length_one_byte_off_either_way_is_refused() {
    for canonical in [5999u32, 6001] {
        let (boot, report, _) = run(6000, 6000, canonical);
        assert_eq!(slot_of(boot), 0, "canonical {canonical}");
        assert_eq!(report, vec![(1, Outcome::Proof(Refusal::BadSize))]);
    }
}

#[test]
fn residues_after_the_canonical_end_are_not_part_of_the_comparison() {
    // 6000 bytes of image, then stale bytes up to 8000 in the slot: the proof is over 6000.
    let mut flash = MemFlash::new();
    good_slot(&mut flash, SLOTS[0], &image(4000, 1));
    let full = image(6000, 2);
    install(
        &mut flash,
        SLOTS[1],
        &full,
        &manifest(key_a(), &full),
        key_a(),
        &key_a().public_struct,
    );
    flash.poke(SLOTS[1].offset + 6000, &[0xA5; 2000]);
    let mut report = Vec::new();
    let p = plan(
        two_valid(),
        2,
        &verified(),
        &mut flash,
        SLOTS,
        &mut |slot| Structure::Bootable {
            canonical_size: if slot == 1 { 6000 } else { 4000 },
        },
        &mut |s, o| report.push((s, o)),
    )
    .unwrap();
    assert_eq!(slot_of(p.boot), 1);
    assert!(report.is_empty());
}

#[test]
fn an_unsigned_profile_signs_no_length_and_is_not_subject_to_the_comparison() {
    let mut flash = MemFlash::new();
    flash.poke(SLOTS[0].offset, &image(4000, 1));
    let mut report = Vec::new();
    let p = plan(
        [
            Entry::new(1, state::VALID).encode(),
            iobewi_firmware_boot::BLANK,
        ],
        2,
        &UnsignedBoot,
        &mut flash,
        SLOTS,
        &mut |_| Structure::Bootable {
            canonical_size: 1234,
        },
        &mut |s, o| report.push((s, o)),
    )
    .unwrap();
    assert_eq!(slot_of(p.boot), 0);
    assert!(report.is_empty());
}

#[test]
fn a_slot_whose_structure_cannot_be_measured_never_reaches_the_comparison() {
    let mut flash = MemFlash::new();
    good_slot(&mut flash, SLOTS[0], &image(4000, 1));
    good_slot(&mut flash, SLOTS[1], &image(4000, 2));
    let mut report = Vec::new();
    let p = plan(
        two_valid(),
        2,
        &verified(),
        &mut flash,
        SLOTS,
        &mut |slot| {
            if slot == 1 {
                Structure::Refused(7)
            } else {
                Structure::Bootable {
                    canonical_size: 4000,
                }
            }
        },
        &mut |s, o| report.push((s, o)),
    )
    .unwrap();
    assert_eq!(slot_of(p.boot), 0);
    assert_eq!(report, vec![(1, Outcome::Structure(7))]);
}
