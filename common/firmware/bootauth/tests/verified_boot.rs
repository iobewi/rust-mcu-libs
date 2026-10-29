//! VerifiedBoot decisions, with real RSA-PSS 3072 signatures and a simulated flash.

mod common;

use common::*;
use iobewi_firmware_auth::{AuthError, DecodeError, Mcu, Product};
use iobewi_firmware_bootauth::{BootPolicy, Refusal, UnsignedBoot, Verdict, iap1};

const SLOT: usize = 0;

fn refused(verdict: Verdict) -> Refusal {
    match verdict {
        Verdict::Refused(refusal) => refusal,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn admit(flash: &mut MemFlash) -> Verdict {
    verified().admit(flash, SLOTS[SLOT])
}

fn flash_with(image: &[u8]) -> MemFlash {
    let mut flash = MemFlash::new();
    good_slot(&mut flash, SLOTS[SLOT], image);
    flash
}

#[test]
fn a_valid_proof_for_the_image_in_the_slot_is_authentic() {
    let img = image(10_000, 1);
    let mut flash = flash_with(&img);
    match admit(&mut flash) {
        Verdict::Authentic(a) => {
            assert_eq!(a.signed_size(), Some(10_000));
            assert!(a.key_id().is_some());
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_image_filling_the_usable_capacity_is_authentic_and_one_byte_more_is_not() {
    let usable = SLOTS[SLOT].usable_size() as usize;
    let full = image(usable, 2);
    let mut flash = flash_with(&full);
    assert!(matches!(admit(&mut flash), Verdict::Authentic(_)));
    // A manifest that claims more than the usable capacity would reach into the proof sector.
    let over = image(usable + 1, 2);
    let mut flash = MemFlash::new();
    let signer = key_a();
    let m = manifest(signer, &over);
    // The proof is written, but the image cannot be: only `usable` bytes fit before the proof.
    install(
        &mut flash,
        SLOTS[SLOT],
        &over[..usable],
        &m,
        signer,
        &signer.public_struct,
    );
    assert_eq!(refused(admit(&mut flash)), Refusal::BadSize);
}

#[test]
fn an_erased_proof_sector_is_absent() {
    let mut flash = MemFlash::new();
    flash.poke(SLOTS[SLOT].offset, &image(1000, 3));
    assert_eq!(refused(admit(&mut flash)), Refusal::ProofAbsent);
}

#[test]
fn a_proof_without_its_commit_word_is_truncated() {
    let mut flash = flash_with(&image(2000, 4));
    let commit = SLOTS[SLOT].proof_offset() + iap1::OFF_COMMIT as u32;
    flash.poke(commit, &[0xFF; 4]);
    assert_eq!(refused(admit(&mut flash)), Refusal::ProofTruncated);
}

#[test]
fn a_damaged_proof_is_corrupt_wherever_the_damage_is() {
    let base = SLOTS[SLOT].proof_offset();
    let total = iap1::payload_len(iobewi_firmware_auth::Algorithm::RsaPss3072Sha256);
    for (what, offset, mask) in [
        ("magic", 0u32, 0x01u8),
        ("version", 4, 0x01),
        ("length", 8, 0x04),
        ("crc", 12, 0x80),
        ("commit word", 16, 0x01),
        ("envelope", 20 + 10, 0x01),
        ("signature", 20 + 100, 0x10),
        ("key", 20 + 460 + 200, 0x02),
        ("last payload byte", 20 + total as u32 - 1, 0x01),
    ] {
        let mut flash = flash_with(&image(3000, 5));
        flash.flip(base + offset, mask);
        assert_eq!(refused(admit(&mut flash)), Refusal::ProofCorrupt, "{what}");
    }
}

#[test]
fn a_truncated_sector_tail_is_corrupt_not_accepted() {
    // Payload cut short (rest erased) but the commit word present: the CRC catches it.
    let mut flash = flash_with(&image(3000, 6));
    let base = SLOTS[SLOT].proof_offset();
    flash.poke(base + 20 + 700, &[0xFF; 400]);
    // Bits can only be cleared by programming, but an erase/cut can set them: model that.
    assert_eq!(refused(admit(&mut flash)), Refusal::ProofCorrupt);
}

/// Flips a bit of the signature and fixes the CRC, so the record reaches the cryptography.
fn resealed(flash: &mut MemFlash, offset: u32, mask: u8) {
    let base = SLOTS[SLOT].proof_offset();
    flash.flip(base + offset, mask);
    let total = iap1::payload_len(iobewi_firmware_auth::Algorithm::RsaPss3072Sha256);
    let head = flash.peek(base, iap1::OFF_CRC).to_vec();
    let payload = flash.peek(base + 20, total).to_vec();
    let crc = iap1::crc(&head, &payload);
    let at = (base - BASE) as usize + iap1::OFF_CRC;
    flash.data[at..at + 4].copy_from_slice(&crc.to_le_bytes());
}

#[test]
fn a_flipped_signature_bit_is_refused_by_the_cryptography() {
    let mut flash = flash_with(&image(4000, 7));
    resealed(&mut flash, 20 + 76 + 5, 0x01);
    assert_eq!(refused(admit(&mut flash)), Refusal::BadSignature);
}

#[test]
fn a_signature_by_a_key_that_is_not_pinned_is_refused() {
    let img = image(4000, 8);
    let mut flash = MemFlash::new();
    let other = key_b();
    install(
        &mut flash,
        SLOTS[SLOT],
        &img,
        &manifest(other, &img),
        other,
        &other.public_struct,
    );
    assert_eq!(refused(admit(&mut flash)), Refusal::UnknownKey);
}

#[test]
fn a_manifest_naming_the_pinned_key_but_signed_by_another_is_refused() {
    let img = image(4000, 9);
    let mut flash = MemFlash::new();
    let (pinned, attacker) = (key_a(), key_b());
    // The proof carries the pinned public key and a manifest with the pinned key id, but the
    // signature comes from the attacker's key.
    install(
        &mut flash,
        SLOTS[SLOT],
        &img,
        &manifest(pinned, &img),
        attacker,
        &pinned.public_struct,
    );
    assert_eq!(refused(admit(&mut flash)), Refusal::BadSignature);
}

#[test]
fn every_byte_of_the_pinned_key_encoding_matters() {
    // The pin covers the whole 776-byte structure, including the precomputed rinv and mdash.
    let img = image(4000, 10);
    let signer = key_a();
    for (what, at) in [
        ("modulus", 3usize),
        ("exponent", 384),
        ("rinv", 388 + 7),
        ("mdash", 772),
    ] {
        let mut key = signer.public_struct;
        key[at] ^= 1;
        let mut flash = MemFlash::new();
        install(
            &mut flash,
            SLOTS[SLOT],
            &img,
            &manifest(signer, &img),
            signer,
            &key,
        );
        let verdict = refused(admit(&mut flash));
        assert!(
            matches!(verdict, Refusal::UnknownKey | Refusal::InvalidKey),
            "{what}: {verdict:?}"
        );
    }
}

#[test]
fn a_proof_for_another_target_is_refused() {
    let img = image(4000, 11);
    let signer = key_a();
    let base = manifest(signer, &img);
    for (what, m, expected) in [
        (
            "product",
            iobewi_firmware_auth::Manifest {
                product: Product::new("someone-else").unwrap(),
                ..base
            },
            AuthError::WrongProduct,
        ),
        (
            "mcu",
            iobewi_firmware_auth::Manifest {
                mcu: Mcu::Esp32C3,
                ..base
            },
            AuthError::WrongMcu,
        ),
        (
            "variant",
            iobewi_firmware_auth::Manifest {
                hw_variant: 7,
                ..base
            },
            AuthError::WrongVariant,
        ),
        (
            "security version",
            iobewi_firmware_auth::Manifest {
                security_version: 0,
                ..base
            },
            AuthError::SecurityVersionTooLow,
        ),
    ] {
        let mut flash = MemFlash::new();
        install(
            &mut flash,
            SLOTS[SLOT],
            &img,
            &m,
            signer,
            &signer.public_struct,
        );
        assert_eq!(
            refused(admit(&mut flash)),
            Refusal::Target(expected),
            "{what}"
        );
    }
}

#[test]
fn an_image_modified_after_signing_is_refused() {
    let img = image(6000, 12);
    for at in [0u32, 1, 255, 256, 5999] {
        let mut flash = flash_with(&img);
        flash.flip(SLOTS[SLOT].offset + at, 0x80);
        assert_eq!(
            refused(admit(&mut flash)),
            Refusal::DigestMismatch,
            "byte {at}"
        );
    }
}

#[test]
fn an_old_proof_next_to_a_new_image_is_refused() {
    // A proof interrupted before its erase took effect still describes the previous image.
    let mut flash = flash_with(&image(5000, 13));
    flash.poke(SLOTS[SLOT].offset, &image(5000, 14));
    assert_eq!(refused(admit(&mut flash)), Refusal::DigestMismatch);
}

#[test]
fn a_proof_with_the_wrong_algorithm_is_refused() {
    // Hand-built record claiming P-256 (identifier 1) to an RSA-only bootloader.
    let mut flash = flash_with(&image(3000, 15));
    let base = SLOTS[SLOT].proof_offset();
    let mut head = flash.peek(base, iap1::HEADER_LEN).to_vec();
    head[6] = 1;
    head[8..12].copy_from_slice(
        &(iap1::payload_len(iobewi_firmware_auth::Algorithm::EcdsaP256Sha256) as u32).to_le_bytes(),
    );
    flash.poke(base, &head);
    assert_eq!(refused(admit(&mut flash)), Refusal::AlgorithmNotAllowed);
}

#[test]
fn a_non_canonical_envelope_is_refused_after_a_valid_crc() {
    let mut flash = flash_with(&image(3000, 16));
    // Manifest magic broken, CRC fixed: the record is intact but the envelope is not IAM1.
    resealed(&mut flash, 20, 0x01);
    assert_eq!(
        refused(admit(&mut flash)),
        Refusal::Malformed(DecodeError::Magic)
    );
}

#[test]
fn a_flash_read_error_is_not_a_refusal() {
    let img = image(6000, 17);
    let slot = SLOTS[SLOT];
    for (what, range) in [
        ("header", (slot.proof_offset(), slot.proof_offset() + 20)),
        (
            "payload",
            (slot.proof_offset() + 100, slot.proof_offset() + 120),
        ),
        ("image body", (slot.offset + 3000, slot.offset + 3010)),
    ] {
        let mut flash = flash_with(&img);
        flash.fail_reads = Some(range);
        assert_eq!(admit(&mut flash), Verdict::Io, "{what}");
    }
}

#[test]
fn the_unsigned_profile_reads_no_proof_and_admits() {
    let mut flash = MemFlash::new();
    flash.poke(SLOTS[SLOT].offset, &image(1000, 18));
    assert!(matches!(
        UnsignedBoot.admit(&mut flash, SLOTS[SLOT]),
        Verdict::Authentic(a) if a.signed_size().is_none() && a.key_id().is_none()
    ));
    assert!(
        flash.reads.is_empty(),
        "no read at all, hence no cryptographic cost"
    );
}

#[test]
fn a_verified_profile_needs_pins_and_variants() {
    use iobewi_firmware_bootauth::ConfigError;
    use iobewi_firmware_bootauth::host::{HostRsaVerifier, HostSha256};
    type V<'a> = iobewi_firmware_bootauth::VerifiedBoot<'a, HostRsaVerifier, HostSha256>;
    assert!(matches!(
        V::new(target(), &[], HostRsaVerifier),
        Err(ConfigError::NoPins)
    ));
    assert!(matches!(
        V::new(target(), &[[0; 32]], HostRsaVerifier),
        Err(ConfigError::ZeroPin)
    ));
    let no_variants = iobewi_firmware_auth::Target {
        hw_variants: &[],
        ..target()
    };
    assert!(matches!(
        V::new(no_variants, pins(), HostRsaVerifier),
        Err(ConfigError::NoVariants)
    ));
}

#[test]
fn the_reference_verifier_is_as_strict_as_the_rom_about_the_salt_length() {
    // The ROM refuses salt lengths other than 32 (measured on an ESP32-S3); the host reference
    // used by these tests must agree, or they would accept proofs the bootloader refuses.
    use iobewi_firmware_auth::{SignatureVerifier, manifest::signed_bytes};
    use iobewi_firmware_bootauth::host::HostRsaVerifier;
    use rsa::{Pss, sha2::Sha256};
    let signer = key_a();
    let m = manifest(signer, &image(100, 1));
    let signed = signed_bytes(&m.encode());
    let digest = <Sha256 as sha2::Digest>::digest(signed);
    for salt in [0usize, 20, 32, 48] {
        let be = signer
            .private
            .sign_with_rng(
                &mut rand_core::OsRng,
                Pss::new_with_salt::<Sha256>(salt),
                &digest,
            )
            .unwrap();
        let le: Vec<u8> = be.iter().rev().copied().collect();
        let accepted = HostRsaVerifier
            .verify(&signer.public_struct, &signed, &le)
            .is_ok();
        assert_eq!(accepted, salt == 32, "salt {salt}");
    }
}
