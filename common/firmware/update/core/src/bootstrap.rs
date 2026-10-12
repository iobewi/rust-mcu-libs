//! Persistent first-install bootstrap lifecycle for OTA-capable devices.
//!
//! This state machine is deliberately separate from the normal OTA
//! transaction state. It describes only the one-time path from a fresh
//! device to its first confirmed production agent:
//!
//! Factory -> Provisioning -> ReadyForAgent -> Production.
//!
//! Persistence is supplied through a small opaque store boundary so this
//! module has no knowledge of ConfigSpace, NVS, flash layout, ESP, or any
//! other platform mechanism.

extern crate alloc;

use alloc::vec::Vec;

const MAGIC: &[u8; 4] = b"LFC2";
const ENCODED_LEN: usize = 5;

/// Maximum durable payload required by the bootstrap lifecycle.
pub const MAX_BYTES: usize = ENCODED_LEN;

/// Persistent byte store for the bootstrap lifecycle.
///
/// Implementations decide where and how the record is stored. The store must
/// publish each replacement atomically from the reader's point of view.
#[allow(async_fn_in_trait)]
pub trait BootstrapStore {
    type Error;

    async fn load_raw(&self) -> Result<Option<Vec<u8>>, Self::Error>;
    async fn commit_raw(&self, bytes: &[u8]) -> Result<(), Self::Error>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BootstrapState {
    Factory = 0,
    Provisioning = 1,
    ReadyForAgent = 2,
    Production = 3,
}

impl BootstrapState {
    fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Factory),
            1 => Some(Self::Provisioning),
            2 => Some(Self::ReadyForAgent),
            3 => Some(Self::Production),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Factory => "factory",
            Self::Provisioning => "provisioning",
            Self::ReadyForAgent => "ready_for_agent",
            Self::Production => "production",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootstrapError {
    Persistence,
    Corrupt,
    InvalidTransition {
        from: BootstrapState,
        to: BootstrapState,
    },
}

fn decode(raw: &[u8]) -> Option<BootstrapState> {
    if raw.len() != ENCODED_LEN || &raw[..4] != MAGIC {
        return None;
    }
    BootstrapState::from_u8(raw[4])
}

fn encode(state: BootstrapState) -> [u8; ENCODED_LEN] {
    let mut encoded = [0u8; ENCODED_LEN];
    encoded[..4].copy_from_slice(MAGIC);
    encoded[4] = state as u8;
    encoded
}

/// Load the bootstrap lifecycle. Absence means a genuinely fresh device.
/// A present but malformed record is never treated as Factory.
pub async fn state<S: BootstrapStore>(
    store: &S,
) -> Result<BootstrapState, BootstrapError> {
    match store.load_raw().await {
        Ok(None) => Ok(BootstrapState::Factory),
        Ok(Some(raw)) => decode(&raw).ok_or(BootstrapError::Corrupt),
        Err(_) => Err(BootstrapError::Persistence),
    }
}

async fn commit<S: BootstrapStore>(
    store: &S,
    next: BootstrapState,
) -> Result<(), BootstrapError> {
    store
        .commit_raw(&encode(next))
        .await
        .map_err(|_| BootstrapError::Persistence)
}

async fn transition<S: BootstrapStore>(
    store: &S,
    expected: BootstrapState,
    next: BootstrapState,
) -> Result<(), BootstrapError> {
    let current = state(store).await?;
    if current == next {
        return Ok(());
    }
    if current != expected {
        return Err(BootstrapError::InvalidTransition {
            from: current,
            to: next,
        });
    }
    commit(store, next).await
}

/// Start or resume the one-time provisioning phase.
pub async fn begin_provisioning<S: BootstrapStore>(
    store: &S,
) -> Result<(), BootstrapError> {
    transition(
        store,
        BootstrapState::Factory,
        BootstrapState::Provisioning,
    )
    .await
}

/// Mark all durable prerequisites ready so the first production agent may be
/// activated through the normal OTA transaction path.
pub async fn ready_for_agent<S: BootstrapStore>(
    store: &S,
) -> Result<(), BootstrapError> {
    transition(
        store,
        BootstrapState::Provisioning,
        BootstrapState::ReadyForAgent,
    )
    .await
}

/// Complete first-install bootstrap after the first agent image has been
/// confirmed by the normal OTA validation path.
pub async fn production<S: BootstrapStore>(
    store: &S,
) -> Result<(), BootstrapError> {
    transition(
        store,
        BootstrapState::ReadyForAgent,
        BootstrapState::Production,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::cell::RefCell;
    use core::future::Future;
    use core::task::{Context, Poll, Waker};

    #[derive(Default)]
    struct MemoryStore(RefCell<Option<Vec<u8>>>);

    impl BootstrapStore for MemoryStore {
        type Error = ();

        async fn load_raw(&self) -> Result<Option<Vec<u8>>, Self::Error> {
            Ok(self.0.borrow().clone())
        }

        async fn commit_raw(&self, bytes: &[u8]) -> Result<(), Self::Error> {
            *self.0.borrow_mut() = Some(bytes.to_vec());
            Ok(())
        }
    }

    fn ready<F: Future>(future: F) -> F::Output {
        let waker = Waker::noop();
        let mut future = core::pin::pin!(future);
        loop {
            match future.as_mut().poll(&mut Context::from_waker(waker)) {
                Poll::Ready(value) => return value,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    #[test]
    fn fresh_device_is_factory() {
        let store = MemoryStore::default();
        assert_eq!(ready(state(&store)), Ok(BootstrapState::Factory));
    }

    #[test]
    fn normal_first_install_path_reaches_production() {
        let store = MemoryStore::default();
        ready(begin_provisioning(&store)).unwrap();
        assert_eq!(ready(state(&store)), Ok(BootstrapState::Provisioning));

        ready(ready_for_agent(&store)).unwrap();
        assert_eq!(ready(state(&store)), Ok(BootstrapState::ReadyForAgent));

        ready(production(&store)).unwrap();
        assert_eq!(ready(state(&store)), Ok(BootstrapState::Production));
    }

    #[test]
    fn transitions_are_idempotent_but_cannot_skip_states() {
        let store = MemoryStore::default();
        ready(begin_provisioning(&store)).unwrap();
        ready(begin_provisioning(&store)).unwrap();

        assert_eq!(
            ready(production(&store)),
            Err(BootstrapError::InvalidTransition {
                from: BootstrapState::Provisioning,
                to: BootstrapState::Production,
            })
        );
    }

    #[test]
    fn corrupt_record_is_not_treated_as_factory() {
        let store = MemoryStore(RefCell::new(Some(b"bad".to_vec())));
        assert_eq!(ready(state(&store)), Err(BootstrapError::Corrupt));
    }
}
