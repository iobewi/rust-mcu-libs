//! The OTA engine's authentication boundary: publication and activation
//! under both profiles, with real ECDSA P-256 signatures.

use iobewi_firmware_auth::manifest::{Algorithm, ImageFormat, KeyId, Manifest, Mcu, Product};
use iobewi_firmware_auth::verify::host::{p256::ecdsa::SigningKey, public_key, sign};
use iobewi_firmware_auth::{
    AuthError, P256Verifier, SignatureRequired, TrustPolicy, TrustedKey, UnsignedAllowed,
};
use iobewi_firmware_update::metadata::{
    MetadataStore, SessionParams, Stage, format_digest, load_metadata,
};
use iobewi_firmware_update::service::{
    ActivationError, AdmitError, BeginError, BootActivation, OtaService, PublishError, SlotDigest,
};
use iobewi_firmware_update::{Committed, Digest};
use std::cell::{Cell, RefCell};
use std::future::Future;
use std::task::{Context, Poll, Waker};

fn ready<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    match future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("synchronous test backend must not suspend"),
    }
}

#[derive(Default)]
struct Store(RefCell<Option<Vec<u8>>>);
impl MetadataStore for Store {
    type Error = ();
    async fn load_raw(&self) -> Result<Option<Vec<u8>>, ()> {
        Ok(self.0.borrow().clone())
    }
    async fn commit_raw(&self, bytes: &[u8]) -> Result<(), ()> {
        *self.0.borrow_mut() = Some(bytes.to_vec());
        Ok(())
    }
}

#[derive(Default)]
struct Boot {
    activations: Cell<u32>,
}
impl BootActivation for Boot {
    fn valid_target(&self, target: &str) -> bool {
        target == "ota_1"
    }
    async fn activate(&self, _: &str) -> Result<(), ()> {
        self.activations.set(self.activations.get() + 1);
        Ok(())
    }
}

/// The digest the slot "contains" now, and how often it was read.
struct Slot {
    digest: Cell<Option<Digest>>,
    reads: Cell<u32>,
}
impl Slot {
    fn holding(digest: Digest) -> Self {
        Self {
            digest: Cell::new(Some(digest)),
            reads: Cell::new(0),
        }
    }
}
impl SlotDigest for Slot {
    async fn slot_digest(&self, _: &str, _: u64) -> Result<Digest, ()> {
        self.reads.set(self.reads.get() + 1);
        self.digest.get().ok_or(())
    }
}

const SIZE: u32 = 4096;
const IMAGE: Digest = Digest([0xab; 32]);
const OTHER: Digest = Digest([0xcd; 32]);

fn signer(seed: u8) -> SigningKey {
    let mut scalar = [seed; 32];
    scalar[0] = 1;
    SigningKey::from_slice(&scalar).unwrap()
}

fn envelope(key: &SigningKey, digest: Digest, security_version: u32) -> Vec<u8> {
    let manifest = Manifest {
        algorithm: Algorithm::EcdsaP256Sha256,
        image_format: ImageFormat::EspApp,
        mcu: Mcu::Esp32S3,
        key_id: KeyId::of(&public_key(key)),
        product: Product::new("iobewi-ota").unwrap(),
        hw_variant: 0,
        security_version,
        size: SIZE,
        digest,
    };
    let mut out = manifest.encode().to_vec();
    out.extend_from_slice(&sign(key, &manifest));
    out
}

fn params(id: &str, digest: Digest) -> SessionParams {
    SessionParams {
        deployment_id: id.into(),
        digest: format_digest(&digest),
        total: SIZE,
    }
}

fn committed(digest: Digest) -> Committed {
    Committed {
        size: u64::from(SIZE),
        digest,
    }
}

/// Runs `f` with a signed service pinned to `keys`, floor `min_version`.
fn signed<R>(
    keys: &[(&SigningKey, bool)],
    min_version: u32,
    f: impl FnOnce(&OtaService<SignatureRequired<'_, P256Verifier>>) -> R,
) -> R {
    let publics: Vec<[u8; 65]> = keys.iter().map(|(k, _)| public_key(k)).collect();
    let pinned: Vec<TrustedKey<'_>> = publics
        .iter()
        .zip(keys)
        .map(|(public, (_, revoked))| {
            let key = TrustedKey::pin(Algorithm::EcdsaP256Sha256, public);
            if *revoked { key.revoked() } else { key }
        })
        .collect();
    let variants = [0u16];
    let profile = SignatureRequired::new(
        TrustPolicy {
            product: Product::new("iobewi-ota").unwrap(),
            mcu: Mcu::Esp32S3,
            image_format: ImageFormat::EspApp,
            hw_variants: &variants,
            min_security_version: min_version,
            keys: &pinned,
        },
        P256Verifier,
    )
    .unwrap();
    f(&OtaService::new(profile))
}

/// Signed upload: admit, begin, publish. Returns the publication result.
fn publish_signed(
    service: &OtaService<SignatureRequired<'_, P256Verifier>>,
    store: &Store,
    id: &str,
    envelope: Option<&[u8]>,
    artifact: Committed,
) -> Result<(), PublishError> {
    let p = params(id, IMAGE);
    let proof = service.admit(envelope, &p).expect("admitted");
    ready(service.begin(store, &p, &proof)).expect("begin");
    ready(service.publish(store, id.into(), artifact, "ota_1".into(), &proof))
}

fn stage_of(store: &Store) -> &'static str {
    ready(load_metadata(store)).unwrap().staged.stage.as_str()
}

#[test]
fn signed_upload_publishes_and_activation_needs_the_signature_again() {
    let key = signer(7);
    let store = Store::default();
    let boot = Boot::default();
    let slot = Slot::holding(IMAGE);
    let good = envelope(&key, IMAGE, 1);
    signed(&[(&key, false)], 0, |service| {
        publish_signed(service, &store, "d1", Some(&good), committed(IMAGE)).unwrap();
        assert_eq!(stage_of(&store), "written");

        // No envelope at activation: refused, nothing changes.
        assert_eq!(
            ready(service.activate(&store, &boot, &slot, "d1", None)),
            Err(ActivationError::Auth(AuthError::EnvelopeMissing))
        );
        assert_eq!((boot.activations.get(), stage_of(&store)), (0, "written"));
        assert_eq!(slot.reads.get(), 0, "nothing hashed before authorization");

        // Envelope presented again, slot hashes to the authorized digest.
        assert_eq!(
            ready(service.activate(&store, &boot, &slot, "d1", Some(&good))),
            Ok("ota_1".to_string())
        );
        assert_eq!((boot.activations.get(), slot.reads.get()), (1, 1));
        assert_eq!(stage_of(&store), "activating");
    });
}

#[test]
fn a_proof_for_another_image_cannot_publish_or_begin() {
    let key = signer(7);
    let store = Store::default();
    let good = envelope(&key, IMAGE, 1);
    signed(&[(&key, false)], 0, |service| {
        let p = params("d1", IMAGE);
        let proof = service.admit(Some(&good), &p).unwrap();
        // Published bytes hash differently from what the proof covers.
        assert_eq!(
            ready(service.publish(
                &store,
                "d1".into(),
                committed(OTHER),
                "ota_1".into(),
                &proof
            )),
            Err(PublishError::Auth(AuthError::ProofMismatch))
        );
        assert!(ready(load_metadata(&store)).unwrap().staged.stage == Stage::None);
        // The session being opened is not the one that was admitted.
        assert_eq!(
            ready(service.begin(&store, &params("d1", OTHER), &proof)),
            Err(BeginError::Unauthorized(AuthError::ProofMismatch))
        );
        assert_eq!(
            ready(service.begin(
                &store,
                &SessionParams {
                    total: SIZE + 1,
                    ..p.clone()
                },
                &proof
            )),
            Err(BeginError::Unauthorized(AuthError::ProofMismatch))
        );
    });
}

#[test]
fn admission_refuses_before_any_state_changes() {
    let key = signer(7);
    let store = Store::default();
    signed(&[(&key, false)], 0, |service| {
        let p = params("d1", IMAGE);
        assert_eq!(
            service.admit(None, &p),
            Err(AdmitError::Auth(AuthError::EnvelopeMissing))
        );
        let mut bad = envelope(&key, IMAGE, 1);
        bad[100] ^= 1;
        assert!(matches!(
            service.admit(Some(&bad), &p),
            Err(AdmitError::Auth(_))
        ));
        let wrong_image = envelope(&key, OTHER, 1);
        assert_eq!(
            service.admit(Some(&wrong_image), &p),
            Err(AdmitError::Auth(AuthError::DigestMismatch))
        );
        let junk = SessionParams {
            digest: "not-a-digest".into(),
            ..p
        };
        assert_eq!(service.admit(None, &junk), Err(AdmitError::BadDigest));
    });
    assert!(store.0.borrow().is_none(), "no metadata written");
}

#[test]
fn activation_of_another_image_or_tampered_envelope_is_refused() {
    let key = signer(7);
    let store = Store::default();
    let boot = Boot::default();
    let slot = Slot::holding(IMAGE);
    let good = envelope(&key, IMAGE, 1);
    signed(&[(&key, false)], 0, |service| {
        publish_signed(service, &store, "d1", Some(&good), committed(IMAGE)).unwrap();
        // A valid signature, but for a different image than the staged one.
        let other = envelope(&key, OTHER, 1);
        assert_eq!(
            ready(service.activate(&store, &boot, &slot, "d1", Some(&other))),
            Err(ActivationError::Auth(AuthError::DigestMismatch))
        );
        for at in [0usize, 40, 60, 80, 139] {
            let mut tampered = good.clone();
            tampered[at] ^= 0x01;
            assert!(matches!(
                ready(service.activate(&store, &boot, &slot, "d1", Some(&tampered))),
                Err(ActivationError::Auth(_))
            ));
        }
        assert_eq!(
            ready(service.activate(&store, &boot, &slot, "other", Some(&good))),
            Err(ActivationError::DeploymentMismatch)
        );
        assert_eq!((boot.activations.get(), stage_of(&store)), (0, "written"));
    });
}

#[test]
fn the_slot_is_hashed_again_before_activation() {
    let key = signer(7);
    let store = Store::default();
    let boot = Boot::default();
    let good = envelope(&key, IMAGE, 1);
    signed(&[(&key, false)], 0, |service| {
        publish_signed(service, &store, "d1", Some(&good), committed(IMAGE)).unwrap();
        // The bytes in flash changed after publication.
        let altered = Slot::holding(OTHER);
        assert_eq!(
            ready(service.activate(&store, &boot, &altered, "d1", Some(&good))),
            Err(ActivationError::Auth(AuthError::SlotDigestMismatch))
        );
        let unreadable = Slot {
            digest: Cell::new(None),
            reads: Cell::new(0),
        };
        assert_eq!(
            ready(service.activate(&store, &boot, &unreadable, "d1", Some(&good))),
            Err(ActivationError::SlotUnreadable)
        );
        assert_eq!((boot.activations.get(), stage_of(&store)), (0, "written"));
    });
}

#[test]
fn an_image_published_without_signature_needs_full_verification_in_signed_mode() {
    let key = signer(7);
    let store = Store::default();
    let boot = Boot::default();
    let slot = Slot::holding(IMAGE);
    // Published by an unsigned-profile firmware.
    let legacy = OtaService::new(UnsignedAllowed);
    let p = params("legacy", IMAGE);
    let proof = legacy.admit(None, &p).unwrap();
    ready(legacy.begin(&store, &p, &proof)).unwrap();
    ready(legacy.publish(
        &store,
        "legacy".into(),
        committed(IMAGE),
        "ota_1".into(),
        &proof,
    ))
    .unwrap();
    assert_eq!(stage_of(&store), "written");

    signed(&[(&key, false)], 0, |service| {
        assert_eq!(
            ready(service.activate(&store, &boot, &slot, "legacy", None)),
            Err(ActivationError::Auth(AuthError::EnvelopeMissing))
        );
        // A signature over some other image does not authorize this one.
        let other = envelope(&key, OTHER, 1);
        assert_eq!(
            ready(service.activate(&store, &boot, &slot, "legacy", Some(&other))),
            Err(ActivationError::Auth(AuthError::DigestMismatch))
        );
        assert_eq!(boot.activations.get(), 0);
        // Signed afterwards for this very image and verified in flash: allowed.
        let signed_now = envelope(&key, IMAGE, 1);
        assert_eq!(
            ready(service.activate(&store, &boot, &slot, "legacy", Some(&signed_now))),
            Ok("ota_1".to_string())
        );
    });
}

#[test]
fn the_policy_in_force_at_activation_decides() {
    let (old, new) = (signer(7), signer(8));
    let store = Store::default();
    let boot = Boot::default();
    let slot = Slot::holding(IMAGE);
    let by_old = envelope(&old, IMAGE, 3);
    signed(&[(&old, false)], 0, |service| {
        publish_signed(service, &store, "d1", Some(&by_old), committed(IMAGE)).unwrap();
    });
    // The key is revoked after publication.
    signed(&[(&old, true), (&new, false)], 0, |service| {
        assert_eq!(
            ready(service.activate(&store, &boot, &slot, "d1", Some(&by_old))),
            Err(ActivationError::Auth(AuthError::RevokedKey))
        );
    });
    // The floor is raised above the manifest's security version.
    signed(&[(&old, false)], 4, |service| {
        assert_eq!(
            ready(service.activate(&store, &boot, &slot, "d1", Some(&by_old))),
            Err(ActivationError::Auth(AuthError::SecurityVersionTooLow))
        );
    });
    assert_eq!((boot.activations.get(), stage_of(&store)), (0, "written"));
    // Back under a policy that still accepts it.
    signed(&[(&old, false)], 3, |service| {
        assert!(ready(service.activate(&store, &boot, &slot, "d1", Some(&by_old))).is_ok());
    });
}

#[test]
fn authorization_survives_a_restart_because_nothing_is_kept_in_ram() {
    let key = signer(7);
    let store = Store::default();
    let boot = Boot::default();
    let slot = Slot::holding(IMAGE);
    let good = envelope(&key, IMAGE, 1);
    signed(&[(&key, false)], 0, |service| {
        publish_signed(service, &store, "d1", Some(&good), committed(IMAGE)).unwrap();
    });
    // A new service instance over the same persisted record (reboot).
    signed(&[(&key, false)], 0, |service| {
        assert_eq!(
            ready(service.activate(&store, &boot, &slot, "d1", None)),
            Err(ActivationError::Auth(AuthError::EnvelopeMissing))
        );
        assert!(ready(service.activate(&store, &boot, &slot, "d1", Some(&good))).is_ok());
    });
    // Once Activating, a second activation is not a way around anything.
    signed(&[(&key, false)], 0, |service| {
        assert_eq!(
            ready(service.activate(&store, &boot, &slot, "d1", Some(&good))),
            Err(ActivationError::NotStaged)
        );
    });
}

#[test]
fn the_unsigned_profile_keeps_todays_behavior_without_hashing_the_slot() {
    let store = Store::default();
    let boot = Boot::default();
    let slot = Slot::holding(OTHER);
    let service = OtaService::new(UnsignedAllowed);
    let p = params("d1", IMAGE);
    let proof = service.admit(None, &p).unwrap();
    ready(service.begin(&store, &p, &proof)).unwrap();
    ready(service.publish(
        &store,
        "d1".into(),
        committed(IMAGE),
        "ota_1".into(),
        &proof,
    ))
    .unwrap();
    assert_eq!(
        ready(service.activate(&store, &boot, &slot, "d1", None)),
        Ok("ota_1".to_string())
    );
    assert_eq!((boot.activations.get(), slot.reads.get()), (1, 0));
    // An envelope is ignored, not required.
    let again = OtaService::new(UnsignedAllowed);
    assert!(again.admit(Some(b"ignored"), &p).is_ok());
}
