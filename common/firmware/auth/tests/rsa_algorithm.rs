//! The RSA-PSS 3072 algorithm identifier in the portable core: encoding, key shape and
//! policy checks. The RSA arithmetic itself is an adapter's job (ROM on ESP32, a host crate
//! in `iobewi-firmware-bootauth`), so a stub verifier stands in here.

use iobewi_firmware_auth::manifest::{
    Algorithm, Envelope, ImageFormat, KeyId, MANIFEST_LEN, Manifest, Mcu, Product, signed_bytes,
};
use iobewi_firmware_auth::{
    AuthError, AuthProfile, DecodeError, PolicyError, SignatureRequired, SignatureVerifier,
    TrustPolicy, TrustedKey, UploadClaim, VerifyError,
};
use iobewi_firmware_image::Digest;
use sha2::{Digest as _, Sha256};

const SIZE: u32 = 1_087_968;
const DIGEST: Digest = Digest([0x5a; 32]);

fn good_key(seed: u8) -> [u8; 776] {
    let mut k = [seed; 776];
    k[0] |= 1; // n odd
    k[383] |= 0x80; // top bit set
    k[384..388].copy_from_slice(&65537u32.to_le_bytes());
    k
}

/// Signature = SHA-256(key || signed) repeated over 384 bytes: no arithmetic, same plumbing.
fn tag(key: &[u8], signed: &[u8]) -> [u8; 384] {
    let mut h = Sha256::new();
    h.update(key);
    h.update(signed);
    let d = h.finalize();
    let mut out = [0u8; 384];
    for (i, b) in out.iter_mut().enumerate() {
        *b = d[i % 32];
    }
    out
}

struct Stub;
impl SignatureVerifier for Stub {
    const ALGORITHM: Algorithm = Algorithm::RsaPss3072Sha256;
    fn verify(&self, key: &[u8], signed: &[u8], sig: &[u8]) -> Result<(), VerifyError> {
        if sig == tag(key, signed) {
            Ok(())
        } else {
            Err(VerifyError::BadSignature)
        }
    }
}

fn manifest(key: &[u8]) -> Manifest {
    Manifest {
        algorithm: Algorithm::RsaPss3072Sha256,
        image_format: ImageFormat::EspApp,
        mcu: Mcu::Esp32S3,
        key_id: KeyId::of(key),
        product: Product::new("iobewi-ota-sys").unwrap(),
        hw_variant: 0,
        security_version: 1,
        size: SIZE,
        digest: DIGEST,
    }
}

fn envelope(m: &Manifest, key: &[u8]) -> Vec<u8> {
    let raw = m.encode();
    let mut out = raw.to_vec();
    out.extend_from_slice(&tag(key, &signed_bytes(&raw)));
    out
}

fn claim() -> UploadClaim {
    UploadClaim {
        size: u64::from(SIZE),
        digest: DIGEST,
    }
}

#[test]
fn the_algorithm_has_the_documented_identifier_and_lengths() {
    let a = Algorithm::RsaPss3072Sha256;
    assert_eq!(
        (a.id(), a.public_key_len(), a.signature_len()),
        (2, 776, 384)
    );
    // The envelope still fits the transport bound.
    assert!(MANIFEST_LEN + a.signature_len() <= iobewi_firmware_auth::MAX_ENVELOPE);
}

#[test]
fn key_shape_rejects_what_cannot_be_a_3072_bit_rsa_key() {
    let a = Algorithm::RsaPss3072Sha256;
    assert!(a.key_shape_ok(&good_key(7)));
    assert!(!a.key_shape_ok(&good_key(7)[..775]), "short");
    let mut k = good_key(7);
    k[0] &= !1;
    assert!(!a.key_shape_ok(&k), "even modulus");
    let mut k = good_key(7);
    k[383] &= 0x7f;
    assert!(!a.key_shape_ok(&k), "modulus is not 3072 bits");
    let mut k = good_key(7);
    k[384..388].copy_from_slice(&3u32.to_le_bytes());
    assert!(!a.key_shape_ok(&k), "exponent other than 65537");
    // A P-256 point is never accepted as an RSA key, and the reverse.
    let mut point = [0u8; 65];
    point[0] = 0x04;
    assert!(!a.key_shape_ok(&point));
    assert!(!Algorithm::EcdsaP256Sha256.key_shape_ok(&good_key(7)));
}

#[test]
fn an_rsa_envelope_has_exactly_manifest_plus_384_signature_bytes() {
    let key = good_key(1);
    let raw = envelope(&manifest(&key), &key);
    assert_eq!(raw.len(), MANIFEST_LEN + 384);
    assert_eq!(
        Envelope::decode(&raw).unwrap().manifest.algorithm,
        Algorithm::RsaPss3072Sha256
    );
    // A P-256-sized signature under the RSA identifier is a length error, not a crash.
    assert_eq!(
        Envelope::decode(&raw[..MANIFEST_LEN + 64]).unwrap_err(),
        DecodeError::Length
    );
    assert_eq!(
        Envelope::decode(&raw[..raw.len() - 1]).unwrap_err(),
        DecodeError::Length
    );
}

#[test]
fn a_signed_profile_over_rsa_keys_admits_and_refuses_like_the_p256_one() {
    let key = good_key(1);
    let keys = [TrustedKey::pin(Algorithm::RsaPss3072Sha256, &key)];
    let policy = TrustPolicy {
        product: Product::new("iobewi-ota-sys").unwrap(),
        mcu: Mcu::Esp32S3,
        image_format: ImageFormat::EspApp,
        hw_variants: &[0, 1],
        min_security_version: 1,
        keys: &keys,
    };
    let profile = SignatureRequired::new(policy, Stub).unwrap();
    let m = manifest(&key);
    assert!(
        profile
            .admit_upload(Some(&envelope(&m, &key)), &claim())
            .is_ok()
    );
    for (what, bad, expected) in [
        (
            "product",
            Manifest {
                product: Product::new("other").unwrap(),
                ..m
            },
            AuthError::WrongProduct,
        ),
        (
            "mcu",
            Manifest {
                mcu: Mcu::Esp32C3,
                ..m
            },
            AuthError::WrongMcu,
        ),
        (
            "variant",
            Manifest { hw_variant: 9, ..m },
            AuthError::WrongVariant,
        ),
        (
            "security version",
            Manifest {
                security_version: 0,
                ..m
            },
            AuthError::SecurityVersionTooLow,
        ),
    ] {
        assert_eq!(
            profile
                .admit_upload(Some(&envelope(&bad, &key)), &claim())
                .unwrap_err(),
            expected,
            "{what}"
        );
    }
    // Signature made over other bytes.
    let mut forged = envelope(&m, &key);
    *forged.last_mut().unwrap() ^= 1;
    assert_eq!(
        profile.admit_upload(Some(&forged), &claim()).unwrap_err(),
        AuthError::BadSignature
    );
}

#[test]
fn a_policy_pinning_a_malformed_rsa_key_is_refused_at_construction() {
    let mut bad = good_key(1);
    bad[384..388].copy_from_slice(&3u32.to_le_bytes());
    let keys = [TrustedKey::pin(Algorithm::RsaPss3072Sha256, &bad)];
    let policy = TrustPolicy {
        product: Product::new("iobewi-ota-sys").unwrap(),
        mcu: Mcu::Esp32S3,
        image_format: ImageFormat::EspApp,
        hw_variants: &[0],
        min_security_version: 0,
        keys: &keys,
    };
    assert!(matches!(
        SignatureRequired::new(policy, Stub),
        Err(PolicyError::BadKey)
    ));
}

#[test]
fn the_raw_target_check_agrees_with_decoding_on_every_single_byte_change() {
    use iobewi_firmware_auth::RawManifest;
    let key = good_key(1);
    let target = TrustPolicy {
        product: Product::new("iobewi-ota-sys").unwrap(),
        mcu: Mcu::Esp32S3,
        image_format: ImageFormat::EspApp,
        hw_variants: &[0, 1],
        min_security_version: 1,
        keys: &[],
    }
    .target();
    let good = manifest(&key).encode();
    let mut cases = vec![good];
    for at in 0..MANIFEST_LEN {
        for value in [
            0u8,
            1,
            2,
            3,
            0x20,
            0x7f,
            0xff,
            good[at] ^ 0x01,
            good[at] ^ 0x80,
        ] {
            let mut raw = good;
            raw[at] = value;
            cases.push(raw);
        }
    }
    for raw in cases {
        // What the slow path says: decodable, right algorithm, and acceptable for the target.
        let slow = Manifest::decode(&raw)
            .ok()
            .filter(|m| m.algorithm == Algorithm::RsaPss3072Sha256)
            .map(|m| target.check(&m));
        let fast = RawManifest::parse(&raw).ok().and_then(|m| {
            (m.algorithm_id() == Algorithm::RsaPss3072Sha256.id()).then(|| target.check_raw(&m))
        });
        // The fast path may only be stricter on framing; it must never accept more.
        match (&slow, &fast) {
            (Some(Ok(())), Some(Ok(()))) => {}
            (_, Some(Ok(()))) => panic!("raw check accepts what decoding refuses: {raw:02x?}"),
            (Some(Ok(())), _) => panic!("raw check refuses what decoding accepts: {raw:02x?}"),
            (Some(Err(a)), Some(Err(b))) => assert_eq!(a, b, "{raw:02x?}"),
            _ => {}
        }
    }
}
