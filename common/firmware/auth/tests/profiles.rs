//! Authentication behavior of the two profiles, with real ECDSA P-256 signatures.
#![cfg(feature = "host-sign")]

use iobewi_firmware_auth::manifest::{
    Algorithm, Envelope, ImageFormat, KeyId, MANIFEST_LEN, Manifest, Mcu, Product, signed_bytes,
};
use iobewi_firmware_auth::verify::host::{p256, public_key, sign};
use iobewi_firmware_auth::{
    AuthError, AuthProfile, DecodeError, KeyState, P256Verifier, PolicyError, SignatureRequired,
    StagedClaim, TrustPolicy, TrustedKey, UnsignedAllowed, UploadClaim,
};
use iobewi_firmware_image::Digest;
use p256::ecdsa::SigningKey;
use sha2::{Digest as _, Sha256};

const SIZE: u32 = 1_025_744;
const DIGEST: Digest = Digest([0xab; 32]);

fn key(seed: u8) -> SigningKey {
    let mut scalar = [seed; 32];
    scalar[0] = 1;
    SigningKey::from_slice(&scalar).unwrap()
}

fn manifest_for(signer: &SigningKey) -> Manifest {
    Manifest {
        algorithm: Algorithm::EcdsaP256Sha256,
        image_format: ImageFormat::EspApp,
        mcu: Mcu::Esp32S3,
        key_id: KeyId::of(&public_key(signer)),
        product: Product::new("iobewi-ota").unwrap(),
        hw_variant: 3,
        security_version: 5,
        size: SIZE,
        digest: DIGEST,
    }
}

fn envelope(manifest: &Manifest, signer: &SigningKey) -> Vec<u8> {
    let mut out = manifest.encode().to_vec();
    out.extend_from_slice(&sign(signer, manifest));
    out
}

fn claim() -> UploadClaim {
    UploadClaim {
        size: u64::from(SIZE),
        digest: DIGEST,
    }
}

struct Fixture {
    signer: SigningKey,
    public: [u8; 65],
}

impl Fixture {
    fn new() -> Self {
        let signer = key(7);
        Self {
            public: public_key(&signer),
            signer,
        }
    }

    fn with<R>(&self, f: impl FnOnce(&SignatureRequired<'_, P256Verifier>) -> R) -> R {
        let keys = [TrustedKey::pin(Algorithm::EcdsaP256Sha256, &self.public)];
        let variants = [2u16, 3];
        let profile = SignatureRequired::new(
            TrustPolicy {
                product: Product::new("iobewi-ota").unwrap(),
                mcu: Mcu::Esp32S3,
                image_format: ImageFormat::EspApp,
                hw_variants: &variants,
                min_security_version: 5,
                keys: &keys,
            },
            P256Verifier,
        )
        .unwrap();
        f(&profile)
    }

    fn good(&self) -> Vec<u8> {
        envelope(&manifest_for(&self.signer), &self.signer)
    }
}

#[test]
fn a_correct_signature_is_admitted_and_the_proof_describes_the_image() {
    let f = Fixture::new();
    f.with(|p| {
        let proof = p.admit_upload(Some(&f.good()), &claim()).unwrap();
        assert_eq!((proof.size(), proof.digest()), (u64::from(SIZE), DIGEST));
        assert_eq!(proof.security_version(), 5);
        assert_eq!(proof.key_id(), KeyId::of(&f.public));
        // The same envelope authorizes the staged record at activation.
        let staged = StagedClaim {
            size: u64::from(SIZE),
            digest: DIGEST,
        };
        assert!(p.admit_activation(Some(&f.good()), &staged).is_ok());
    });
}

#[test]
fn a_proof_is_bound_to_one_image() {
    let f = Fixture::new();
    f.with(|p| {
        let proof = p.admit_upload(Some(&f.good()), &claim()).unwrap();
        assert_eq!(p.check_binding(&proof, u64::from(SIZE), &DIGEST), Ok(()));
        assert_eq!(
            p.check_binding(&proof, u64::from(SIZE) + 1, &DIGEST),
            Err(AuthError::ProofMismatch)
        );
        assert_eq!(
            p.check_binding(&proof, u64::from(SIZE), &Digest([0xcd; 32])),
            Err(AuthError::ProofMismatch)
        );
    });
}

#[test]
fn flipping_any_bit_of_the_envelope_is_refused() {
    let f = Fixture::new();
    let good = f.good();
    f.with(|p| {
        for byte in 0..good.len() {
            for bit in 0..8 {
                let mut tampered = good.clone();
                tampered[byte] ^= 1 << bit;
                assert!(
                    p.admit_upload(Some(&tampered), &claim()).is_err(),
                    "byte {byte} bit {bit} accepted"
                );
            }
        }
    });
}

#[test]
fn truncated_or_extended_envelopes_are_malformed() {
    let f = Fixture::new();
    let good = f.good();
    f.with(|p| {
        for len in 0..good.len() {
            assert!(
                p.admit_upload(Some(&good[..len]), &claim()).is_err(),
                "{len}"
            );
        }
        let mut longer = good.clone();
        longer.push(0);
        assert_eq!(
            p.admit_upload(Some(&longer), &claim()).unwrap_err(),
            AuthError::Malformed(DecodeError::Length)
        );
        assert_eq!(
            p.admit_upload(None, &claim()).unwrap_err(),
            AuthError::EnvelopeMissing
        );
    });
}

#[test]
fn image_and_claim_substitution_are_refused() {
    let f = Fixture::new();
    f.with(|p| {
        let envelope = f.good();
        let other_digest = UploadClaim {
            size: u64::from(SIZE),
            digest: Digest([0xcd; 32]),
        };
        assert_eq!(
            p.admit_upload(Some(&envelope), &other_digest).unwrap_err(),
            AuthError::DigestMismatch
        );
        let other_size = UploadClaim {
            size: u64::from(SIZE) - 1,
            digest: DIGEST,
        };
        assert_eq!(
            p.admit_upload(Some(&envelope), &other_size).unwrap_err(),
            AuthError::SizeMismatch
        );
        // The same applies to the staged record at activation.
        let staged = StagedClaim {
            size: u64::from(SIZE),
            digest: Digest([0xcd; 32]),
        };
        assert_eq!(
            p.admit_activation(Some(&envelope), &staged).unwrap_err(),
            AuthError::DigestMismatch
        );
    });
}

#[test]
fn manifest_substitution_by_another_signer_is_refused() {
    let f = Fixture::new();
    f.with(|p| {
        // Valid signature, but by a key that is not pinned.
        let other = key(9);
        let forged = envelope(&manifest_for(&other), &other);
        assert_eq!(
            p.admit_upload(Some(&forged), &claim()).unwrap_err(),
            AuthError::UnknownKey
        );
        // A pinned key id claimed by a manifest signed with another key.
        let mut impostor = manifest_for(&f.signer);
        impostor.key_id = KeyId::of(&f.public);
        let bad = envelope(&impostor, &other);
        assert_eq!(
            p.admit_upload(Some(&bad), &claim()).unwrap_err(),
            AuthError::BadSignature
        );
    });
}

#[test]
fn the_signature_is_domain_separated() {
    let f = Fixture::new();
    f.with(|p| {
        use p256::ecdsa::signature::hazmat::PrehashSigner;
        let manifest = manifest_for(&f.signer);
        // Signed over the bare manifest, without the protocol prefix.
        let sig: p256::ecdsa::Signature = f
            .signer
            .sign_prehash(&Sha256::digest(manifest.encode()))
            .unwrap();
        let sig = sig.normalize_s().unwrap_or(sig);
        let mut out = manifest.encode().to_vec();
        out.extend_from_slice(&sig.to_bytes());
        assert_eq!(
            p.admit_upload(Some(&out), &claim()).unwrap_err(),
            AuthError::BadSignature
        );
    });
}

#[test]
fn reserved_and_unknown_algorithms_are_refused_even_when_signed() {
    let f = Fixture::new();
    f.with(|p| {
        for id in [3u8, 9] {
            let mut raw = manifest_for(&f.signer).encode();
            raw[5] = id;
            let sig: p256::ecdsa::Signature = {
                use p256::ecdsa::signature::hazmat::PrehashSigner;
                f.signer
                    .sign_prehash(&Sha256::digest(signed_bytes(&raw)))
                    .unwrap()
            };
            let mut out = raw.to_vec();
            out.extend_from_slice(&sig.normalize_s().unwrap_or(sig).to_bytes());
            let error = p.admit_upload(Some(&out), &claim()).unwrap_err();
            let expected = if id == 9 {
                DecodeError::Algorithm
            } else {
                DecodeError::UnsupportedAlgorithm
            };
            assert_eq!(error, AuthError::Malformed(expected), "id {id}");
        }
    });
}

#[test]
fn non_canonical_signatures_are_refused() {
    let f = Fixture::new();
    f.with(|p| {
        let manifest = manifest_for(&f.signer);
        let good = sign(&f.signer, &manifest);
        let (r, s) = p256::ecdsa::Signature::from_slice(&good)
            .unwrap()
            .split_scalars();
        // High-S twin: mathematically valid, but a second encoding.
        let high = p256::ecdsa::Signature::from_scalars(
            *r,
            p256::NonZeroScalar::new(-*s.as_ref()).unwrap(),
        )
        .unwrap();
        let mut out = manifest.encode().to_vec();
        out.extend_from_slice(&high.to_bytes());
        assert_eq!(
            p.admit_upload(Some(&out), &claim()).unwrap_err(),
            AuthError::Malformed(DecodeError::Signature)
        );
        // Zero scalars.
        let mut zero = manifest.encode().to_vec();
        zero.extend_from_slice(&[0u8; 64]);
        assert_eq!(
            p.admit_upload(Some(&zero), &claim()).unwrap_err(),
            AuthError::Malformed(DecodeError::Signature)
        );
    });
}

#[test]
fn policy_refusals_each_have_their_own_error() {
    let f = Fixture::new();
    f.with(|p| {
        let refuse = |edit: &dyn Fn(&mut Manifest)| {
            let mut manifest = manifest_for(&f.signer);
            edit(&mut manifest);
            p.admit_upload(Some(&envelope(&manifest, &f.signer)), &claim())
                .unwrap_err()
        };
        assert_eq!(refuse(&|m| m.mcu = Mcu::Esp32C3), AuthError::WrongMcu);
        assert_eq!(refuse(&|m| m.mcu = Mcu::Rp2350), AuthError::WrongMcu);
        assert_eq!(
            refuse(&|m| m.product = Product::new("other-product").unwrap()),
            AuthError::WrongProduct
        );
        assert_eq!(refuse(&|m| m.hw_variant = 4), AuthError::WrongVariant);
        assert_eq!(refuse(&|m| m.hw_variant = 0), AuthError::WrongVariant);
        assert_eq!(
            refuse(&|m| m.security_version = 4),
            AuthError::SecurityVersionTooLow
        );
        // The floor itself and higher versions are accepted; both variants are allowed.
        for (variant, version) in [(2, 5), (3, 5), (3, 6), (3, u32::MAX)] {
            let mut manifest = manifest_for(&f.signer);
            manifest.hw_variant = variant;
            manifest.security_version = version;
            assert!(
                p.admit_upload(Some(&envelope(&manifest, &f.signer)), &claim())
                    .is_ok()
            );
        }
    });
}

#[test]
fn revoked_unknown_and_invalid_keys() {
    let f = Fixture::new();
    let variants = [3u16];
    let policy = |keys: &[TrustedKey<'_>]| {
        SignatureRequired::new(
            TrustPolicy {
                product: Product::new("iobewi-ota").unwrap(),
                mcu: Mcu::Esp32S3,
                image_format: ImageFormat::EspApp,
                hw_variants: &variants,
                min_security_version: 0,
                keys,
            },
            P256Verifier,
        )
        .map(|p| p.admit_upload(Some(&f.good()), &claim()).map(|_| ()))
    };
    let other = public_key(&key(9));
    let active_other = TrustedKey::pin(Algorithm::EcdsaP256Sha256, &other);
    let revoked = TrustedKey::pin(Algorithm::EcdsaP256Sha256, &f.public).revoked();
    // The signer is pinned but revoked; another key keeps the policy usable.
    assert_eq!(
        policy(&[revoked, active_other]).unwrap(),
        Err(AuthError::RevokedKey)
    );
    // The signer is not pinned at all.
    assert_eq!(policy(&[active_other]).unwrap(), Err(AuthError::UnknownKey));
    // A pinned key that is not a point of the curve fails when used, not silently.
    let mut junk = [0x11u8; 65];
    junk[0] = 0x04;
    let mut manifest = manifest_for(&f.signer);
    manifest.key_id = KeyId::of(&junk);
    let forged = envelope(&manifest, &f.signer);
    let keys = [TrustedKey::pin(Algorithm::EcdsaP256Sha256, &junk)];
    let profile = SignatureRequired::new(
        TrustPolicy {
            product: Product::new("iobewi-ota").unwrap(),
            mcu: Mcu::Esp32S3,
            image_format: ImageFormat::EspApp,
            hw_variants: &variants,
            min_security_version: 0,
            keys: &keys,
        },
        P256Verifier,
    )
    .unwrap();
    assert_eq!(
        profile.admit_upload(Some(&forged), &claim()).unwrap_err(),
        AuthError::InvalidKey
    );
}

#[test]
fn a_signed_profile_cannot_be_built_without_a_usable_policy() {
    let f = Fixture::new();
    let variants = [3u16];
    let build = |keys: &[TrustedKey<'_>], variants: &[u16]| {
        SignatureRequired::new(
            TrustPolicy {
                product: Product::new("iobewi-ota").unwrap(),
                mcu: Mcu::Esp32S3,
                image_format: ImageFormat::EspApp,
                hw_variants: variants,
                min_security_version: 0,
                keys,
            },
            P256Verifier,
        )
        .map(|_| ())
    };
    let good = TrustedKey::pin(Algorithm::EcdsaP256Sha256, &f.public);
    assert_eq!(build(&[], &variants), Err(PolicyError::NoKeys));
    assert_eq!(
        build(&[good.revoked()], &variants),
        Err(PolicyError::NoActiveKey)
    );
    assert_eq!(build(&[good], &[]), Err(PolicyError::NoVariants));
    assert_eq!(
        build(&[good, good], &variants),
        Err(PolicyError::DuplicateKeyId)
    );
    let short = [0x04u8; 64];
    assert_eq!(
        build(
            &[TrustedKey::pin(Algorithm::EcdsaP256Sha256, &short)],
            &variants
        ),
        Err(PolicyError::BadKey)
    );
    let mut compressed = f.public;
    compressed[0] = 0x02;
    assert_eq!(
        build(
            &[TrustedKey::pin(Algorithm::EcdsaP256Sha256, &compressed)],
            &variants
        ),
        Err(PolicyError::BadKey)
    );
    assert_eq!(build(&[good], &variants), Ok(()));
}

#[test]
fn the_unsigned_profile_needs_no_envelope_and_ignores_one() {
    let p = UnsignedAllowed;
    let staged = StagedClaim {
        size: 1,
        digest: DIGEST,
    };
    assert!(p.admit_upload(None, &claim()).is_ok());
    assert!(p.admit_upload(Some(b"garbage"), &claim()).is_ok());
    let proof = p.admit_activation(None, &staged).unwrap();
    assert_eq!(p.check_binding(&proof, 123, &Digest([1; 32])), Ok(()));
    let rehash = |unsigned: bool| {
        std::hint::black_box(if unsigned {
            UnsignedAllowed::REHASH_SLOT_ON_ACTIVATION
        } else {
            SignatureRequired::<P256Verifier>::REHASH_SLOT_ON_ACTIVATION
        })
    };
    assert!(!rehash(true));
    assert!(rehash(false));
}

#[test]
fn envelope_decoding_is_independent_of_the_policy() {
    let f = Fixture::new();
    let good = f.good();
    let envelope = Envelope::decode(&good).unwrap();
    assert_eq!(good.len(), MANIFEST_LEN + 64);
    assert_eq!(envelope.manifest, manifest_for(&f.signer));
    assert_eq!(KeyState::Active, KeyState::Active);
}

/// Policy helper for the regression tests below: pinned keys, floor, variants.
fn profile_with<R>(
    keys: &[TrustedKey<'_>],
    min_security_version: u32,
    f: impl FnOnce(&SignatureRequired<'_, P256Verifier>) -> R,
) -> R {
    let variants = [2u16, 3];
    let profile = SignatureRequired::new(
        TrustPolicy {
            product: Product::new("iobewi-ota").unwrap(),
            mcu: Mcu::Esp32S3,
            image_format: ImageFormat::EspApp,
            hw_variants: &variants,
            min_security_version,
            keys,
        },
        P256Verifier,
    )
    .unwrap();
    f(&profile)
}

#[test]
fn a_proof_is_bound_to_the_policy_that_issued_it() {
    let f = Fixture::new();
    let (k2, k3) = (key(9), key(11));
    let (p2, p3) = (public_key(&k2), public_key(&k3));
    let k1 = TrustedKey::pin(Algorithm::EcdsaP256Sha256, &f.public);
    let proof = profile_with(&[k1], 0, |a| {
        a.admit_upload(Some(&f.good()), &claim()).unwrap()
    });
    let refused = |keys: &[TrustedKey<'_>], floor: u32| {
        profile_with(keys, floor, |b| {
            b.check_binding(&proof, u64::from(SIZE), &DIGEST)
        })
    };
    // The same policy, rebuilt, still honors its own proof.
    assert_eq!(refused(&[k1], 0), Ok(()));
    // Another key set.
    assert_eq!(
        refused(&[TrustedKey::pin(Algorithm::EcdsaP256Sha256, &p2)], 0),
        Err(AuthError::PolicyMismatch)
    );
    // The same identifier bound to another public key.
    let impostor = TrustedKey {
        id: k1.id,
        public_key: &p3,
        ..k1
    };
    assert_eq!(refused(&[impostor], 0), Err(AuthError::PolicyMismatch));
    // The issuing key revoked since: it can no longer back a new publication.
    let other = TrustedKey::pin(Algorithm::EcdsaP256Sha256, &p2);
    assert_eq!(
        refused(&[k1.revoked(), other], 0),
        Err(AuthError::PolicyMismatch)
    );
    // A raised security floor is another policy.
    assert_eq!(refused(&[k1], 6), Err(AuthError::PolicyMismatch));
    // Adding a key changes the policy too.
    assert_eq!(refused(&[k1, other], 0), Err(AuthError::PolicyMismatch));
}

#[test]
fn a_foreign_policy_is_reported_before_the_image_is_compared() {
    let f = Fixture::new();
    let p2 = public_key(&key(9));
    let k1 = TrustedKey::pin(Algorithm::EcdsaP256Sha256, &f.public);
    let proof = profile_with(&[k1], 0, |a| {
        a.admit_upload(Some(&f.good()), &claim()).unwrap()
    });
    profile_with(
        &[TrustedKey::pin(Algorithm::EcdsaP256Sha256, &p2)],
        0,
        |b| {
            // Neither the right image nor another one gets through.
            assert_eq!(
                b.check_binding(&proof, u64::from(SIZE), &DIGEST),
                Err(AuthError::PolicyMismatch)
            );
            assert_eq!(
                b.check_binding(&proof, 1, &Digest([0; 32])),
                Err(AuthError::PolicyMismatch)
            );
        },
    );
}

#[test]
fn each_profile_announces_itself_without_secrets() {
    assert_eq!(UnsignedAllowed::LABEL, "OTA authentication: UNSIGNED");
    assert_eq!(
        SignatureRequired::<P256Verifier>::LABEL,
        "OTA authentication: SIGNATURE_REQUIRED"
    );
}
