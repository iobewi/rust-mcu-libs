//! A proof write interrupted at every flash command, at several byte offsets inside the
//! interrupted command, never leaves a slot that verifies unless the write completed.

mod common;

use common::*;
use iobewi_firmware_auth::{Algorithm, Manifest};
use iobewi_firmware_bootauth::{BootPolicy, Refusal, Verdict, iap1, writer};
use sha2::{Digest as _, Sha256};

const SLOT: usize = 0;

fn proof_for(img: &[u8]) -> (Manifest, Vec<u8>) {
    let m = manifest(key_a(), img);
    let envelope = key_a().envelope(&m);
    (m, envelope)
}

/// erase, program head, program payload, program commit.
const COMMANDS: usize = 4;

#[test]
fn a_power_cut_at_any_command_and_any_byte_never_yields_an_authentic_slot() {
    let img = image(7000, 3);
    let (_, envelope) = proof_for(&img);
    let torn_points = [0usize, 1, 2, 3, 4, 7, 15, 16, 19, 100, 460, 1000, 1235];
    for cut in 0..COMMANDS {
        for torn in torn_points {
            let mut flash = MemFlash::new();
            flash.poke(SLOTS[SLOT].offset, &img);
            // The sector held the proof of an older image of the same slot.
            good_slot(&mut flash, SLOTS[SLOT], &image(7000, 99));
            flash.poke(SLOTS[SLOT].offset, &img);
            flash.budget = Some(cut);
            flash.torn = torn;
            let mut scratch = [0u8; iap1::MAX_PAYLOAD];
            let write = iap1::Write::new(
                Algorithm::RsaPss3072Sha256,
                &envelope,
                &key_a().public_struct,
                &mut scratch,
            )
            .unwrap();
            let outcome = writer::execute(&mut flash, SLOTS[SLOT], &write);
            flash.budget = None; // power is back
            let verdict = verified().admit(&mut flash, SLOTS[SLOT]);
            // The only way for a "cut" write to be whole is a cut that lets the last command,
            // the 4-byte commit word, finish before power goes.
            let whole = cut == COMMANDS - 1 && torn >= 4;
            if whole {
                let _ = outcome;
                assert!(
                    matches!(verdict, Verdict::Authentic(_)),
                    "cut {cut} torn {torn}: {verdict:?}"
                );
                continue;
            }
            assert!(outcome.is_err(), "cut {cut} torn {torn} must fail");
            match verdict {
                Verdict::Refused(
                    Refusal::ProofAbsent
                    | Refusal::ProofTruncated
                    | Refusal::ProofCorrupt
                    | Refusal::DigestMismatch,
                ) => {}
                other => panic!("cut {cut} torn {torn}: {other:?}"),
            }
        }
    }
}

#[test]
fn an_uninterrupted_write_verifies_and_a_retry_after_a_cut_succeeds() {
    let img = image(7000, 4);
    let (_, envelope) = proof_for(&img);
    let mut flash = MemFlash::new();
    flash.poke(SLOTS[SLOT].offset, &img);
    let mut scratch = [0u8; iap1::MAX_PAYLOAD];
    let write = iap1::Write::new(
        Algorithm::RsaPss3072Sha256,
        &envelope,
        &key_a().public_struct,
        &mut scratch,
    )
    .unwrap();
    flash.budget = Some(2); // erase and head programmed, payload cut
    flash.torn = 300;
    assert!(writer::execute(&mut flash, SLOTS[SLOT], &write).is_err());
    flash.budget = None;
    assert!(matches!(
        verified().admit(&mut flash, SLOTS[SLOT]),
        Verdict::Refused(Refusal::ProofTruncated | Refusal::ProofCorrupt)
    ));
    writer::execute(&mut flash, SLOTS[SLOT], &write).unwrap();
    assert!(matches!(
        verified().admit(&mut flash, SLOTS[SLOT]),
        Verdict::Authentic(_)
    ));
}

#[test]
fn the_commit_word_is_programmed_last_and_alone() {
    let img = image(5000, 5);
    let (_, envelope) = proof_for(&img);
    let mut flash = MemFlash::new();
    flash.poke(SLOTS[SLOT].offset, &img);
    let mut scratch = [0u8; iap1::MAX_PAYLOAD];
    let write = iap1::Write::new(
        Algorithm::RsaPss3072Sha256,
        &envelope,
        &key_a().public_struct,
        &mut scratch,
    )
    .unwrap();
    // Three commands succeed (erase, head, payload), the fourth (the commit word) is cut:
    // everything but the commit word is on flash, and the record is not committed.
    flash.budget = Some(3);
    flash.torn = 0;
    assert!(writer::execute(&mut flash, SLOTS[SLOT], &write).is_err());
    let base = SLOTS[SLOT].proof_offset();
    assert_eq!(flash.peek(base + iap1::OFF_COMMIT as u32, 4), [0xFF; 4]);
    assert_eq!(flash.peek(base, iap1::OFF_COMMIT), write.head);
    flash.budget = None;
    assert_eq!(
        verified().admit(&mut flash, SLOTS[SLOT]),
        Verdict::Refused(Refusal::ProofTruncated)
    );
}

#[test]
fn a_partly_programmed_commit_word_is_never_the_commit_value() {
    // The commit value has balanced bits: clearing any proper subset of its zero bits
    // from an erased word gives another value. Every such partial state is refused.
    let img = image(5000, 6);
    let (_, envelope) = proof_for(&img);
    let zero_bits: Vec<u32> = (0..32).filter(|b| iap1::COMMIT >> b & 1 == 0).collect();
    assert!(
        zero_bits.len() >= 8,
        "enough zero bits for the pattern to be informative"
    );
    for subset in 0u32..(1 << zero_bits.len().min(12)) {
        if subset == (1 << zero_bits.len().min(12)) - 1 && zero_bits.len() <= 12 {
            continue; // all zero bits cleared: that is the commit value itself
        }
        let mut word = u32::MAX;
        for (i, bit) in zero_bits.iter().take(12).enumerate() {
            if subset >> i & 1 == 1 {
                word &= !(1 << bit);
            }
        }
        if word == iap1::COMMIT {
            continue;
        }
        let mut flash = MemFlash::new();
        flash.poke(SLOTS[SLOT].offset, &img);
        let mut scratch = [0u8; iap1::MAX_PAYLOAD];
        let write = iap1::Write::new(
            Algorithm::RsaPss3072Sha256,
            &envelope,
            &key_a().public_struct,
            &mut scratch,
        )
        .unwrap();
        flash.budget = Some(3);
        let _ = writer::execute(&mut flash, SLOTS[SLOT], &write);
        flash.budget = None;
        flash.poke(
            SLOTS[SLOT].proof_offset() + iap1::OFF_COMMIT as u32,
            &word.to_le_bytes(),
        );
        match verified().admit(&mut flash, SLOTS[SLOT]) {
            Verdict::Refused(Refusal::ProofTruncated | Refusal::ProofCorrupt) => {}
            other => panic!("commit word {word:#010x}: {other:?}"),
        }
    }
}

#[test]
fn the_writer_refuses_to_commit_what_does_not_read_back() {
    // A flash that silently drops programming must not get a commit word.
    struct Deaf(MemFlash);
    impl iobewi_firmware_boot::transaction::Flash for Deaf {
        type Error = ();
        fn read(&mut self, a: u32, o: &mut [u8]) -> Result<(), ()> {
            self.0.read(a, o)
        }
        fn erase(&mut self, a: u32, l: u32) -> Result<(), ()> {
            self.0.erase(a, l)
        }
        fn program(&mut self, _: u32, _: &[u8]) -> Result<(), ()> {
            Ok(())
        }
    }
    let (_, envelope) = proof_for(&image(4000, 7));
    let mut scratch = [0u8; iap1::MAX_PAYLOAD];
    let write = iap1::Write::new(
        Algorithm::RsaPss3072Sha256,
        &envelope,
        &key_a().public_struct,
        &mut scratch,
    )
    .unwrap();
    let mut flash = Deaf(MemFlash::new());
    assert_eq!(
        writer::execute(&mut flash, SLOTS[SLOT], &write),
        Err(writer::Error::ReadbackMismatch)
    );
}

#[test]
fn the_proof_record_is_bound_to_nothing_but_the_signed_image() {
    // Moving a valid sector to the other slot proves nothing about that slot's bytes.
    let img = image(5000, 8);
    let mut flash = MemFlash::new();
    good_slot(&mut flash, SLOTS[0], &img);
    flash.poke(SLOTS[1].offset, &image(5000, 9));
    let sector = flash.peek(SLOTS[0].proof_offset(), iap1::SECTOR).to_vec();
    flash.poke(SLOTS[1].proof_offset(), &sector);
    assert_eq!(
        verified().admit(&mut flash, SLOTS[1]),
        Verdict::Refused(Refusal::DigestMismatch)
    );
    // The same image in the other slot with the copied proof *is* authentic: an identical
    // image is the same image, whichever slot holds it.
    flash.poke(SLOTS[1].offset, &img);
    assert!(matches!(
        verified().admit(&mut flash, SLOTS[1]),
        Verdict::Authentic(_)
    ));
    let _ = Sha256::digest(b"");
}

#[test]
fn a_p256_sized_record_is_word_aligned_and_round_trips() {
    // The record format is generic over the algorithm; a 205-byte payload is padded with
    // erased bytes to a whole number of words, outside the CRC.
    use iobewi_firmware_auth::Algorithm::EcdsaP256Sha256 as A;
    let envelope = vec![7u8; iap1::envelope_len(A)];
    let key = vec![9u8; A.public_key_len()];
    let mut scratch = [0u8; iap1::MAX_PAYLOAD];
    let write = iap1::Write::new(A, &envelope, &key, &mut scratch).unwrap();
    assert_eq!(iap1::payload_len(A), 205);
    assert_eq!(write.payload.len(), 208);
    assert_eq!(write.payload[205..], [0xFF; 3]);
    let mut head = [0xFFu8; iap1::HEADER_LEN];
    head[..iap1::OFF_COMMIT].copy_from_slice(&write.head);
    head[iap1::OFF_COMMIT..].copy_from_slice(&iap1::COMMIT.to_le_bytes());
    assert!(matches!(
        iap1::classify(&head),
        iap1::Header::Committed { algorithm: A, crc } if crc == iap1::crc(&write.head, &write.payload[..205])
    ));
    // Wrong lengths are refused when building.
    assert!(iap1::Write::new(A, &envelope[1..], &key, &mut scratch).is_none());
}
