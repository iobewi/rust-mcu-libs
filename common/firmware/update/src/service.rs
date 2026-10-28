//! Durable OTA workflow shared by platform adapters.
//!
//! Each operation publishes metadata before changing a boot slot. A platform
//! must reject invalid target names before the activating record is persisted.

use alloc::string::String;

#[cfg(feature = "runtime")]
pub mod boot;
#[cfg(feature = "runtime")]
pub mod upload;

use crate::metadata::{
    MemoryTransactionMetadata, MetadataError, MetadataStore, PrepareRefusal, SessionParams,
    can_supersede, check_compatibility, check_staged, check_target, commit_transaction,
    firmware_record, load_transaction, parse_digest,
};
use crate::{Committed, Digest, Error, TransactionState, activate};
use iobewi_firmware_auth::{AuthError, AuthProfile, StagedClaim, UploadClaim};

#[allow(async_fn_in_trait)]
pub trait TargetSelection {
    /// Return the inactive target name and its capacity; release all backend
    /// locks before returning so the service may inspect ConfigSpace.
    async fn write_target(&self) -> Result<(&'static str, u64), ()>;
}

/// Check compatibility before selecting a target, then inspect persistence
/// only after the platform has released its flash lock.
pub async fn prepare<S: MetadataStore, P: TargetSelection>(
    store: &S,
    platform: &P,
    chip: &str,
    layout: &str,
    platform_chip: &str,
    platform_layout: &str,
    size: u64,
) -> Result<&'static str, PrepareRefusal> {
    check_compatibility(chip, layout, platform_chip, platform_layout)?;
    let (target, capacity) = platform
        .write_target()
        .await
        .map_err(|_| PrepareRefusal::Busy)?;
    check_target(size, Some(capacity))?;
    let staged = load_transaction(store)
        .await
        .map_err(|_| PrepareRefusal::Busy)?;
    check_staged(staged.as_ref())?;
    Ok(target)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BeginError {
    BadDigest,
    Conflict,
    Storage(MetadataError),
    /// The proof does not describe the image being uploaded.
    Unauthorized(AuthError),
}

/// Resolve the previous staged transaction before writing any new flash bytes.
/// An activating transaction can never be overwritten by an upload.
pub(crate) async fn begin<S: MetadataStore>(
    store: &S,
    params: &SessionParams,
) -> Result<crate::Digest, BeginError> {
    let digest = parse_digest(&params.digest).ok_or(BeginError::BadDigest)?;
    match load_transaction(store).await.map_err(BeginError::Storage)? {
        None => {}
        Some(record) if can_supersede(Some(&record)) => {
            commit_transaction(store, None)
                .await
                .map_err(BeginError::Storage)?;
        }
        Some(_) => return Err(BeginError::Conflict),
    }
    Ok(digest)
}

/// Publish only an artifact whose bytes and digest were already verified.
pub(crate) async fn publish<S: MetadataStore>(
    store: &S,
    deployment_id: String,
    artifact: Committed,
    target: String,
) -> Result<(), MetadataError> {
    let record = firmware_record(deployment_id, artifact.size, artifact.digest, target);
    commit_transaction(store, Some(&record)).await
}

/// Validate the target without touching boot state, then activate it.
#[allow(async_fn_in_trait)]
pub trait BootActivation {
    fn valid_target(&self, target: &str) -> bool;
    async fn activate(&self, target: &str) -> Result<(), ()>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivateError {
    NotStaged,
    DeploymentMismatch,
    Storage(MetadataError),
}

/// Publish `Activating` before programming the boot entry. If the boot entry
/// fails, restore `Staged` so activation can be retried. A failed restore is
/// still recoverable: the next boot reconciles the persisted state.
pub(crate) async fn activate_staged<S: MetadataStore, B: BootActivation>(
    store: &S,
    boot: &B,
    deployment_id: &str,
) -> Result<String, ActivateError> {
    let current = load_transaction(store)
        .await
        .map_err(ActivateError::Storage)?;
    let target = current
        .as_ref()
        .and_then(|record| record.artifacts.first())
        .map(|artifact| artifact.target.clone())
        .filter(|target| boot.valid_target(target))
        .ok_or(ActivateError::NotStaged)?;
    let mut memory = MemoryTransactionMetadata { record: current };
    let activating =
        activate(&mut memory, &String::from(deployment_id)).map_err(|error| match error {
            Error::IdentityMismatch => ActivateError::DeploymentMismatch,
            _ => ActivateError::NotStaged,
        })?;
    commit_transaction(store, memory.record.as_ref())
        .await
        .map_err(ActivateError::Storage)?;
    if boot.activate(&target).await.is_err() {
        let reverted = activating.with_state(TransactionState::Staged);
        let _ = commit_transaction(store, Some(&reverted)).await;
        return Err(ActivateError::NotStaged);
    }
    Ok(target)
}

/// The slot's bytes as they are in flash *now*: the signed profile hashes them
/// again before activating, so an authorization covers what will really boot.
#[allow(async_fn_in_trait)]
pub trait SlotDigest {
    /// SHA-256 of the first `size` bytes of `target`.
    async fn slot_digest(&self, target: &str, size: u64) -> Result<Digest, ()>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmitError {
    BadDigest,
    Auth(AuthError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishError {
    Auth(AuthError),
    Storage(MetadataError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivationError {
    Auth(AuthError),
    /// Nothing is staged, or it is not in a state that can be activated.
    NotStaged,
    DeploymentMismatch,
    /// The slot could not be read back for the authorization check.
    SlotUnreadable,
    Storage(MetadataError),
}

/// The one OTA engine, configured by an authentication profile.
///
/// Publication and activation are only reachable through this type, so the
/// profile's own proof is always required: [`UnsignedAllowed`] and
/// [`SignatureRequired`] share every transfer, publication, activation,
/// confirmation and rollback mechanism and differ only in what they accept
/// as authorization. The proof type is fixed by the profile, so a signed
/// service cannot be handed an unsigned proof:
///
/// ```compile_fail
/// use iobewi_firmware_auth::{Admitted, AuthProfile, UnsignedAllowed};
/// use iobewi_firmware_update::service::OtaService;
/// fn needs_signed<P: AuthProfile<Proof = Admitted>>(_: &OtaService<P>) {}
/// needs_signed(&OtaService::new(UnsignedAllowed));
/// ```
///
/// The raw operations are not public: there is no way around the service.
///
/// ```compile_fail
/// use iobewi_firmware_update::service::publish;
/// ```
///
/// ```compile_fail
/// use iobewi_firmware_update::service::activate_staged;
/// ```
///
/// ```compile_fail
/// use iobewi_firmware_update::service::begin;
/// ```
///
/// [`UnsignedAllowed`]: iobewi_firmware_auth::UnsignedAllowed
/// [`SignatureRequired`]: iobewi_firmware_auth::SignatureRequired
pub struct OtaService<P: AuthProfile> {
    profile: P,
}

impl<P: AuthProfile> OtaService<P> {
    pub const fn new(profile: P) -> Self {
        Self { profile }
    }

    pub fn profile(&self) -> &P {
        &self.profile
    }

    /// Authorizes an upload from its headers, before anything is erased.
    /// `presented` is the decoded manifest envelope, if the client sent one.
    pub fn admit(
        &self,
        presented: Option<&[u8]>,
        params: &SessionParams,
    ) -> Result<P::Proof, AdmitError> {
        let digest = parse_digest(&params.digest).ok_or(AdmitError::BadDigest)?;
        self.profile
            .admit_upload(
                presented,
                &UploadClaim {
                    size: u64::from(params.total),
                    digest,
                },
            )
            .map_err(AdmitError::Auth)
    }

    /// Resolves the previous staged transaction, only for an admitted upload.
    pub async fn begin<S: MetadataStore>(
        &self,
        store: &S,
        params: &SessionParams,
        proof: &P::Proof,
    ) -> Result<Digest, BeginError> {
        let digest = parse_digest(&params.digest).ok_or(BeginError::BadDigest)?;
        self.profile
            .check_binding(proof, u64::from(params.total), &digest)
            .map_err(BeginError::Unauthorized)?;
        begin(store, params).await
    }

    /// Publishes a verified artifact only if the proof describes exactly it.
    pub async fn publish<S: MetadataStore>(
        &self,
        store: &S,
        deployment_id: String,
        artifact: Committed,
        target: String,
        proof: &P::Proof,
    ) -> Result<(), PublishError> {
        self.profile
            .check_binding(proof, artifact.size, &artifact.digest)
            .map_err(PublishError::Auth)?;
        publish(store, deployment_id, artifact, target)
            .await
            .map_err(PublishError::Storage)
    }

    /// Activates the staged deployment `deployment_id`.
    ///
    /// The authorization is evaluated again here, under the current policy,
    /// from `presented` (the envelope the client sends again) against the
    /// persisted record. A signed profile also hashes the slot again, so a
    /// candidate staged without a signature, or altered since, is refused.
    pub async fn activate<S: MetadataStore, B: BootActivation, D: SlotDigest>(
        &self,
        store: &S,
        boot: &B,
        slots: &D,
        deployment_id: &str,
        presented: Option<&[u8]>,
    ) -> Result<String, ActivationError> {
        let record = load_transaction(store)
            .await
            .map_err(ActivationError::Storage)?
            .ok_or(ActivationError::NotStaged)?;
        if record.id != deployment_id {
            return Err(ActivationError::DeploymentMismatch);
        }
        if record.state != TransactionState::Staged {
            return Err(ActivationError::NotStaged);
        }
        let artifact = record.artifacts.first().ok_or(ActivationError::NotStaged)?;
        let (size, digest) = (artifact.size, artifact.digest);
        let proof = self
            .profile
            .admit_activation(presented, &StagedClaim { size, digest })
            .map_err(ActivationError::Auth)?;
        if P::REHASH_SLOT_ON_ACTIVATION {
            let actual = slots
                .slot_digest(&artifact.target, size)
                .await
                .map_err(|()| ActivationError::SlotUnreadable)?;
            self.profile
                .check_binding(&proof, size, &actual)
                .map_err(|_| ActivationError::Auth(AuthError::SlotDigestMismatch))?;
        }
        activate_staged(store, boot, deployment_id)
            .await
            .map_err(|error| match error {
                ActivateError::NotStaged => ActivationError::NotStaged,
                ActivateError::DeploymentMismatch => ActivationError::DeploymentMismatch,
                ActivateError::Storage(error) => ActivationError::Storage(error),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::load_metadata;
    use alloc::vec::Vec;
    use core::cell::{Cell, RefCell};
    use core::future::Future;
    use core::task::{Context, Poll, Waker};

    fn ready<F: Future>(future: F) -> F::Output {
        let waker = Waker::noop();
        let mut future = core::pin::pin!(future);
        match future.as_mut().poll(&mut Context::from_waker(waker)) {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("synchronous test backend must not suspend"),
        }
    }

    struct MemoryStore(RefCell<Option<Vec<u8>>>);

    impl MetadataStore for MemoryStore {
        type Error = ();
        async fn load_raw(&self) -> Result<Option<Vec<u8>>, ()> {
            Ok(self.0.borrow().clone())
        }
        async fn commit_raw(&self, bytes: &[u8]) -> Result<(), ()> {
            *self.0.borrow_mut() = Some(bytes.to_vec());
            Ok(())
        }
    }

    struct FailingBoot<'a> {
        store: &'a MemoryStore,
        observed_activating: Cell<bool>,
    }

    impl BootActivation for FailingBoot<'_> {
        fn valid_target(&self, target: &str) -> bool {
            target == "ota_1"
        }
        async fn activate(&self, _target: &str) -> Result<(), ()> {
            let raw = self.store.0.borrow();
            let metadata = crate::metadata::Metadata::decode(raw.as_ref().unwrap()).unwrap();
            self.observed_activating
                .set(metadata.staged.stage == crate::metadata::Stage::Activating);
            Err(())
        }
    }

    #[test]
    fn activation_publishes_intent_before_boot_and_restores_staged_on_failure() {
        let store = MemoryStore(RefCell::new(None));
        ready(publish(
            &store,
            String::from("deploy"),
            Committed {
                size: 1024,
                digest: crate::Digest([0x42; 32]),
            },
            String::from("ota_1"),
        ))
        .unwrap();
        let boot = FailingBoot {
            store: &store,
            observed_activating: Cell::new(false),
        };
        assert_eq!(
            ready(activate_staged(&store, &boot, "deploy")),
            Err(ActivateError::NotStaged)
        );
        assert!(boot.observed_activating.get());
        assert!(
            ready(load_metadata(&store)).unwrap().staged.stage == crate::metadata::Stage::Written
        );
    }
}
