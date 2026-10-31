//! A proof kept in step with its image across an OTA: invalidate, rewrite the image, publish and
//! re-check the proof, gate the activation. A power cut at every flash command, at several byte
//! offsets inside the interrupted command, never leaves an activatable slot whose proof is
//! absent, partial, stale or foreign, and never touches the confirmed slot.

mod common;

use common::*;
use iobewi_firmware_auth::Algorithm;
use iobewi_firmware_boot::transaction::Flash;
use iobewi_firmware_boot::{Boot, Entry, state};
use iobewi_firmware_bootauth::provision::{self, GateError, PublishError};
use iobewi_firmware_bootauth::select::{Structure, plan};
use iobewi_firmware_bootauth::{BootPolicy, Refusal, Verdict, iap1, writer};

const CONFIRMED: usize = 0;
const INACTIVE: usize = 1;
const IMAGE_LEN: usize = 4000;
/// invalidate (1) + erase body (1) + 4 body programs + publish: erase, head, payload, commit (4).
const COMMANDS: usize = 10;

/// What the OTA does to the inactive slot, in the order it does it.
fn ota(
    flash: &mut MemFlash,
    slot: iobewi_firmware_layout::domain_a::Partition,
    image: &[u8],
    envelope: &[u8],
    key: &[u8],
) -> Result<(), String> {
    provision::invalidate(flash, slot).map_err(|()| "invalidate")?;
    Flash::erase(flash, slot.offset, slot.usable_size()).map_err(|()| "erase body")?;
    for (i, chunk) in image.chunks(image.len() / 4).enumerate() {
        let at = slot.offset + (i * (image.len() / 4)) as u32;
        Flash::program(flash, at, chunk).map_err(|()| "body")?;
    }
    provision::publish(
        flash,
        slot,
        Algorithm::RsaPss3072Sha256,
        envelope,
        key,
        &verified(),
        image.len() as u32,
    )
    .map_err(|e| format!("{e:?}"))
}

fn gate(flash: &mut MemFlash, idx: usize, size: u32) -> Result<(), GateError> {
    provision::gate(flash, SLOTS[idx], &verified(), size)
}

fn confirmed_and_inactive() -> (MemFlash, Vec<u8>, Vec<u8>) {
    let mut flash = MemFlash::new();
    let confirmed = image(IMAGE_LEN, 1);
    good_slot(&mut flash, SLOTS[CONFIRMED], &confirmed);
    // The inactive slot holds an older, validly signed image with its proof.
    good_slot(&mut flash, SLOTS[INACTIVE], &image(IMAGE_LEN, 99));
    let incoming = image(IMAGE_LEN, 2);
    (flash, confirmed, incoming)
}

fn slot_bytes(flash: &MemFlash, idx: usize) -> Vec<u8> {
    flash
        .peek(SLOTS[idx].offset, SLOTS[idx].size as usize)
        .to_vec()
}

#[test]
fn a_power_cut_at_any_command_never_leaves_an_activatable_slot_without_its_proof() {
    let signer = key_a();
    let incoming = image(IMAGE_LEN, 2);
    let envelope = signer.envelope(&manifest(signer, &incoming));
    for cut in 0..COMMANDS {
        for torn in [0usize, 1, 3, 4, 100, 999, 1500] {
            let (mut flash, confirmed, _) = confirmed_and_inactive();
            let before = slot_bytes(&flash, CONFIRMED);
            flash.budget = Some(cut);
            flash.torn = torn;
            let outcome = ota(
                &mut flash,
                SLOTS[INACTIVE],
                &incoming,
                &envelope,
                &signer.public_struct,
            );
            flash.budget = None;
            let label = format!("cut {cut} torn {torn}");

            // The confirmed slot is neither reachable nor changed.
            assert_eq!(slot_bytes(&flash, CONFIRMED), before, "{label}");
            assert!(
                matches!(
                    verified().admit(&mut flash, SLOTS[CONFIRMED]),
                    Verdict::Authentic(_)
                ),
                "{label}"
            );

            // The gate passes only for a coherent pair: the new image with its new proof, or
            // -- when the power went before the first command -- the old pair, untouched.
            let activatable = gate(&mut flash, INACTIVE, IMAGE_LEN as u32).is_ok();
            let untouched = cut == 0 && torn == 0;
            if activatable {
                let bytes = flash.peek(SLOTS[INACTIVE].offset, IMAGE_LEN);
                assert!(
                    bytes == &incoming[..] || (untouched && bytes == &image(IMAGE_LEN, 99)[..]),
                    "{label}: gate passed on bytes that are neither the new image nor the intact old one"
                );
            }
            if outcome.is_ok() {
                assert!(activatable, "{label}");
            }
            if !activatable {
                assert!(
                    outcome.is_err(),
                    "{label}: success reported but the gate refuses"
                );
            }
            // A cut that lets the 4-byte commit word land completes the proof (never published).
            if cut == COMMANDS - 1 && torn >= 4 {
                assert!(activatable, "{label}: the commit word was complete");
            }
            // Any earlier cut after the first command leaves the slot not activatable.
            if (1..COMMANDS - 1).contains(&cut) {
                assert!(!activatable, "{label}: complete too early");
            }

            // The boot stage agrees with the gate, and falls back on the confirmed image.
            let otadata = [
                Entry::new(1, state::VALID).encode(),
                Entry::new(2, state::NEW).encode(),
            ];
            let mut report = |_, _| {};
            let decided = plan(
                otadata,
                2,
                &verified(),
                &mut flash,
                SLOTS,
                &mut |_| Structure::Bootable,
                &mut report,
            )
            .unwrap();
            match decided.boot {
                Boot::Slot { slot, .. } => assert_eq!(slot == 1, activatable, "{label}"),
                other => panic!("{label}: {other:?}"),
            }
            let _ = confirmed;
        }
    }
}

#[test]
fn the_old_proof_is_gone_before_the_first_byte_of_the_image_changes() {
    let signer = key_a();
    let incoming = image(IMAGE_LEN, 2);
    let envelope = signer.envelope(&manifest(signer, &incoming));
    let (mut flash, _, _) = confirmed_and_inactive();
    // One command: only the proof sector is erased; the old image is still there.
    flash.budget = Some(1);
    flash.torn = 0;
    assert!(
        ota(
            &mut flash,
            SLOTS[INACTIVE],
            &incoming,
            &envelope,
            &signer.public_struct
        )
        .is_err()
    );
    flash.budget = None;
    assert_eq!(
        flash.peek(SLOTS[INACTIVE].offset, IMAGE_LEN),
        &image(IMAGE_LEN, 99)[..],
        "image untouched so far"
    );
    assert_eq!(
        gate(&mut flash, INACTIVE, IMAGE_LEN as u32),
        Err(GateError::Refused(Refusal::ProofAbsent)),
        "yet the slot is already not bootable by a verified stage"
    );
}

#[test]
fn an_uninterrupted_ota_makes_the_slot_activatable_and_leaves_the_confirmed_one_alone() {
    let signer = key_a();
    let (mut flash, _, incoming) = confirmed_and_inactive();
    let before = slot_bytes(&flash, CONFIRMED);
    let envelope = signer.envelope(&manifest(signer, &incoming));
    ota(
        &mut flash,
        SLOTS[INACTIVE],
        &incoming,
        &envelope,
        &signer.public_struct,
    )
    .unwrap();
    assert_eq!(gate(&mut flash, INACTIVE, IMAGE_LEN as u32), Ok(()));
    assert_eq!(slot_bytes(&flash, CONFIRMED), before);
}

#[test]
fn a_proof_for_another_image_is_not_published_and_not_left_valid() {
    let signer = key_a();
    let (mut flash, _, incoming) = confirmed_and_inactive();
    // Signed for one image, received another.
    let envelope = signer.envelope(&manifest(signer, &image(IMAGE_LEN, 77)));
    let outcome = ota(
        &mut flash,
        SLOTS[INACTIVE],
        &incoming,
        &envelope,
        &signer.public_struct,
    );
    assert!(outcome.unwrap_err().contains("DigestMismatch"));
    assert!(
        gate(&mut flash, INACTIVE, IMAGE_LEN as u32).is_err(),
        "the slot must not look valid"
    );
}

#[test]
fn activation_is_refused_for_an_absent_partial_stale_or_foreign_proof() {
    let signer = key_a();
    let incoming = image(IMAGE_LEN, 2);
    let envelope = signer.envelope(&manifest(signer, &incoming));
    let proof_at = SLOTS[INACTIVE].proof_offset();

    // absent
    let (mut flash, _, _) = confirmed_and_inactive();
    flash.poke(SLOTS[INACTIVE].offset, &incoming);
    provision::invalidate(&mut flash, SLOTS[INACTIVE]).unwrap();
    assert_eq!(
        gate(&mut flash, INACTIVE, IMAGE_LEN as u32),
        Err(GateError::Refused(Refusal::ProofAbsent))
    );

    // partial: commit word missing
    ota(
        &mut flash,
        SLOTS[INACTIVE],
        &incoming,
        &envelope,
        &signer.public_struct,
    )
    .unwrap();
    flash.poke(proof_at + iap1::OFF_COMMIT as u32, &[0xFF; 4]);
    assert_eq!(
        gate(&mut flash, INACTIVE, IMAGE_LEN as u32),
        Err(GateError::Refused(Refusal::ProofTruncated))
    );

    // stale: the proof of an older image next to new bytes
    let (mut flash, _, _) = confirmed_and_inactive();
    flash.poke(SLOTS[INACTIVE].offset, &incoming);
    assert_eq!(
        gate(&mut flash, INACTIVE, IMAGE_LEN as u32),
        Err(GateError::Refused(Refusal::DigestMismatch))
    );

    // foreign: the other slot's proof, valid for its own image only
    let (mut flash, _, _) = confirmed_and_inactive();
    let confirmed_proof = flash
        .peek(SLOTS[CONFIRMED].proof_offset(), iap1::SECTOR)
        .to_vec();
    flash.poke(SLOTS[INACTIVE].proof_offset(), &confirmed_proof);
    assert_eq!(
        gate(&mut flash, INACTIVE, IMAGE_LEN as u32),
        Err(GateError::Refused(Refusal::DigestMismatch))
    );

    // size: authentic, but not the size the OTA metadata publishes
    let (mut flash, _, _) = confirmed_and_inactive();
    ota(
        &mut flash,
        SLOTS[INACTIVE],
        &incoming,
        &envelope,
        &signer.public_struct,
    )
    .unwrap();
    assert_eq!(
        gate(&mut flash, INACTIVE, IMAGE_LEN as u32 + 4),
        Err(GateError::WrongSize)
    );
}

#[test]
fn a_proof_that_does_not_read_back_is_an_error_and_never_valid() {
    struct Deaf(MemFlash);
    impl Flash for Deaf {
        type Error = ();
        fn read(&mut self, a: u32, o: &mut [u8]) -> Result<(), ()> {
            Flash::read(&mut self.0, a, o)
        }
        fn erase(&mut self, a: u32, l: u32) -> Result<(), ()> {
            Flash::erase(&mut self.0, a, l)
        }
        fn program(&mut self, a: u32, b: &[u8]) -> Result<(), ()> {
            // The payload programs are lost; the rest lands.
            if a >= SLOTS[INACTIVE].proof_offset() + iap1::HEADER_LEN as u32 {
                Ok(())
            } else {
                Flash::program(&mut self.0, a, b)
            }
        }
    }
    impl iobewi_firmware_bootauth::SlotRead for Deaf {
        fn read(
            &mut self,
            o: u32,
            out: &mut [u8],
        ) -> Result<(), iobewi_firmware_bootauth::IoError> {
            iobewi_firmware_bootauth::SlotRead::read(&mut self.0, o, out)
        }
    }
    let signer = key_a();
    let incoming = image(IMAGE_LEN, 2);
    let envelope = signer.envelope(&manifest(signer, &incoming));
    let mut inner = MemFlash::new();
    inner.poke(SLOTS[INACTIVE].offset, &incoming);
    let mut flash = Deaf(inner);
    let outcome = provision::publish(
        &mut flash,
        SLOTS[INACTIVE],
        Algorithm::RsaPss3072Sha256,
        &envelope,
        &signer.public_struct,
        &verified(),
        IMAGE_LEN as u32,
    );
    assert!(matches!(
        outcome,
        Err(PublishError::Write(writer::Error::ReadbackMismatch))
    ));
    assert!(provision::gate(&mut flash, SLOTS[INACTIVE], &verified(), IMAGE_LEN as u32).is_err());
}

#[test]
fn a_p256_proof_is_never_taken_for_an_rsa_one() {
    // A record carrying a P-256 envelope is well-formed IAP1, but a bootloader (and the
    // OTA gate) verifying RSA refuses it: no signature is converted into another algorithm.
    let mut flash = MemFlash::new();
    flash.poke(SLOTS[INACTIVE].offset, &image(IMAGE_LEN, 2));
    let a = Algorithm::EcdsaP256Sha256;
    let envelope = vec![7u8; iap1::envelope_len(a)];
    let key = vec![0x04u8; a.public_key_len()];
    let outcome = provision::publish(
        &mut flash,
        SLOTS[INACTIVE],
        a,
        &envelope,
        &key,
        &verified(),
        IMAGE_LEN as u32,
    );
    assert_eq!(
        outcome,
        Err(PublishError::NotAuthentic(Refusal::AlgorithmNotAllowed))
    );
    // And an RSA-sized publication with the wrong lengths is refused before any write.
    let before = flash
        .peek(SLOTS[INACTIVE].proof_offset(), iap1::SECTOR)
        .to_vec();
    let short = provision::publish(
        &mut flash,
        SLOTS[INACTIVE],
        Algorithm::RsaPss3072Sha256,
        &envelope,
        &key,
        &verified(),
        IMAGE_LEN as u32,
    );
    assert_eq!(short, Err(PublishError::Length));
    assert_eq!(
        flash.peek(SLOTS[INACTIVE].proof_offset(), iap1::SECTOR),
        &before[..]
    );
}
