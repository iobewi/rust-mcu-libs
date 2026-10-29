//! `plan_boot` driven by the boot profile: which slot boots, what is recorded in `otadata`,
//! and what is not recorded when the flash itself misbehaves.

mod common;

use common::*;
use iobewi_firmware_boot::{Boot, Entry, Halt, Raw, Write, state};
use iobewi_firmware_bootauth::select::{Interrupted, Outcome, Structure, plan};
use iobewi_firmware_bootauth::{BootPolicy, Refusal, UnsignedBoot};

fn entries(a: Option<Entry>, b: Option<Entry>) -> [Raw; 2] {
    [
        a.map_or(iobewi_firmware_boot::BLANK, |e| e.encode()),
        b.map_or(iobewi_firmware_boot::BLANK, |e| e.encode()),
    ]
}

type Report = Vec<(u8, Outcome)>;

fn run<P: BootPolicy>(
    policy: &P,
    flash: &mut MemFlash,
    otadata: [Raw; 2],
    structure: &mut dyn FnMut(u8) -> Structure,
) -> (Result<iobewi_firmware_boot::Plan, Interrupted>, Report) {
    let mut report = Report::new();
    let result = plan(
        otadata,
        2,
        policy,
        flash,
        SLOTS,
        structure,
        &mut |slot, outcome| report.push((slot, outcome)),
    );
    (result, report)
}

fn bootable(_: u8) -> Structure {
    Structure::Bootable
}

fn writes(plan: &iobewi_firmware_boot::Plan) -> Vec<Write> {
    plan.writes().collect()
}

/// seq 1 selects slot 0, seq 2 slot 1.
fn confirmed_slot0_and_candidate_slot1() -> [Raw; 2] {
    entries(
        Some(Entry::new(1, state::VALID)),
        Some(Entry::new(2, state::NEW)),
    )
}

#[test]
fn an_authentic_candidate_is_booted_pending_and_the_confirmed_slot_is_kept() {
    let mut flash = MemFlash::new();
    good_slot(&mut flash, SLOTS[0], &image(4000, 1));
    good_slot(&mut flash, SLOTS[1], &image(4000, 2));
    let (plan, report) = run(
        &verified(),
        &mut flash,
        confirmed_slot0_and_candidate_slot1(),
        &mut bootable,
    );
    let plan = plan.unwrap();
    assert!(matches!(
        plan.boot,
        Boot::Slot {
            slot: 1,
            seq: 2,
            ..
        }
    ));
    assert_eq!(writes(&plan).len(), 1);
    assert_eq!(writes(&plan)[0].entry.state, state::PENDING_VERIFY);
    assert!(report.is_empty());
}

#[test]
fn a_tampered_candidate_is_rejected_and_the_valid_confirmed_slot_boots() {
    let mut flash = MemFlash::new();
    good_slot(&mut flash, SLOTS[0], &image(4000, 1));
    good_slot(&mut flash, SLOTS[1], &image(4000, 2));
    flash.flip(SLOTS[1].offset + 100, 0x01);
    let (plan, report) = run(
        &verified(),
        &mut flash,
        confirmed_slot0_and_candidate_slot1(),
        &mut bootable,
    );
    let plan = plan.unwrap();
    assert!(matches!(
        plan.boot,
        Boot::Slot {
            slot: 0,
            seq: 1,
            ..
        }
    ));
    // The candidate's refusal is recorded (it is certain), the confirmed entry is untouched.
    assert_eq!(writes(&plan).len(), 1);
    assert_eq!(
        (writes(&plan)[0].entry.seq, writes(&plan)[0].entry.state),
        (2, state::INVALID)
    );
    assert_eq!(report, vec![(1, Outcome::Proof(Refusal::DigestMismatch))]);
}

#[test]
fn a_candidate_without_proof_is_rejected_whatever_its_structure() {
    let mut flash = MemFlash::new();
    good_slot(&mut flash, SLOTS[0], &image(4000, 1));
    flash.poke(SLOTS[1].offset, &image(4000, 2));
    let (plan, report) = run(
        &verified(),
        &mut flash,
        confirmed_slot0_and_candidate_slot1(),
        &mut bootable,
    );
    assert!(matches!(plan.unwrap().boot, Boot::Slot { slot: 0, .. }));
    assert_eq!(report, vec![(1, Outcome::Proof(Refusal::ProofAbsent))]);
}

#[test]
fn no_authentic_slot_halts_and_never_boots_an_unverified_one() {
    let mut flash = MemFlash::new();
    // Both slots hold bootable-looking images, neither has a proof.
    flash.poke(SLOTS[0].offset, &image(4000, 1));
    flash.poke(SLOTS[1].offset, &image(4000, 2));
    let (plan, _) = run(
        &verified(),
        &mut flash,
        confirmed_slot0_and_candidate_slot1(),
        &mut bootable,
    );
    assert_eq!(plan.unwrap().boot, Boot::Halt(Halt::NoUsableEntry));
    // The unsigned profile, by contrast, boots the candidate: the profile is the whole difference.
    let (plan, _) = run(
        &UnsignedBoot,
        &mut flash,
        confirmed_slot0_and_candidate_slot1(),
        &mut bootable,
    );
    assert!(matches!(plan.unwrap().boot, Boot::Slot { slot: 1, .. }));
}

#[test]
fn an_altered_confirmed_slot_alone_halts() {
    let mut flash = MemFlash::new();
    good_slot(&mut flash, SLOTS[0], &image(4000, 1));
    flash.flip(SLOTS[0].offset + 7, 0x40);
    let otadata = entries(Some(Entry::new(1, state::VALID)), None);
    let (plan, report) = run(&verified(), &mut flash, otadata, &mut bootable);
    assert_eq!(plan.unwrap().boot, Boot::Halt(Halt::NoUsableEntry));
    assert_eq!(report, vec![(0, Outcome::Proof(Refusal::DigestMismatch))]);
}

#[test]
fn a_flash_read_error_discards_the_plan_instead_of_recording_invalid() {
    for (what, fail) in [
        ("candidate proof", SLOTS[1].proof_offset()),
        ("candidate image", SLOTS[1].offset + 10),
        ("confirmed proof", SLOTS[0].proof_offset()),
        ("confirmed image", SLOTS[0].offset + 10),
    ] {
        let mut flash = MemFlash::new();
        good_slot(&mut flash, SLOTS[0], &image(4000, 1));
        good_slot(&mut flash, SLOTS[1], &image(4000, 2));
        flash.fail_reads = Some((fail, fail + 4));
        let (plan, report) = run(
            &verified(),
            &mut flash,
            confirmed_slot0_and_candidate_slot1(),
            &mut bootable,
        );
        // Whether the failing slot is read depends on which is checked first; if it is read,
        // the whole plan is dropped. The candidate is checked first, then the confirmed one.
        let read_it = report.iter().any(|(_, o)| *o == Outcome::Io);
        if read_it {
            assert_eq!(plan, Err(Interrupted), "{what}");
        } else {
            // Not reached: a good candidate boots without looking at the confirmed slot.
            assert!(plan.is_ok(), "{what}");
        }
    }
}

#[test]
fn a_read_error_on_the_candidate_never_marks_it_invalid_nor_falls_back_silently() {
    let mut flash = MemFlash::new();
    good_slot(&mut flash, SLOTS[0], &image(4000, 1));
    good_slot(&mut flash, SLOTS[1], &image(4000, 2));
    flash.fail_reads = Some((SLOTS[1].proof_offset(), SLOTS[1].proof_offset() + 4));
    let (plan, report) = run(
        &verified(),
        &mut flash,
        confirmed_slot0_and_candidate_slot1(),
        &mut bootable,
    );
    assert_eq!(plan, Err(Interrupted));
    assert_eq!(report, vec![(1, Outcome::Io)]);
    // Same flash, transient fault gone: the candidate is simply authentic.
    flash.fail_reads = None;
    let (plan, _) = run(
        &verified(),
        &mut flash,
        confirmed_slot0_and_candidate_slot1(),
        &mut bootable,
    );
    assert!(matches!(plan.unwrap().boot, Boot::Slot { slot: 1, .. }));
}

#[test]
fn a_structure_read_error_is_an_interruption_and_a_structure_refusal_skips_the_proof() {
    let mut flash = MemFlash::new();
    good_slot(&mut flash, SLOTS[0], &image(4000, 1));
    good_slot(&mut flash, SLOTS[1], &image(4000, 2));
    let (plan, _) = run(
        &verified(),
        &mut flash,
        confirmed_slot0_and_candidate_slot1(),
        &mut |slot| {
            if slot == 1 {
                Structure::Io
            } else {
                Structure::Bootable
            }
        },
    );
    assert_eq!(plan, Err(Interrupted));
    flash.reads.clear();
    let (plan, report) = run(
        &verified(),
        &mut flash,
        confirmed_slot0_and_candidate_slot1(),
        &mut |slot| {
            if slot == 1 {
                Structure::Refused(5)
            } else {
                Structure::Bootable
            }
        },
    );
    assert!(matches!(plan.unwrap().boot, Boot::Slot { slot: 0, .. }));
    assert_eq!(report, vec![(1, Outcome::Structure(5))]);
    assert_eq!(
        flash.proof_reads(SLOTS[1]),
        0,
        "no cryptography on a slot that is not even an image"
    );
}

#[test]
fn a_blank_otadata_seeds_slot_zero_only_with_a_valid_proof() {
    let mut flash = MemFlash::new();
    flash.poke(SLOTS[0].offset, &image(4000, 1));
    let (plan, _) = run(&verified(), &mut flash, entries(None, None), &mut bootable);
    assert_eq!(plan.unwrap().boot, Boot::Halt(Halt::NoImage));
    good_slot(&mut flash, SLOTS[0], &image(4000, 1));
    let (plan, _) = run(&verified(), &mut flash, entries(None, None), &mut bootable);
    let plan = plan.unwrap();
    assert!(matches!(
        plan.boot,
        Boot::Slot {
            slot: 0,
            seq: 1,
            ..
        }
    ));
    assert_eq!(writes(&plan)[0].entry.state, state::VALID);
}
