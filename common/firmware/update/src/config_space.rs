//! Optional `ConfigSpace<B>` adapters for the portable OTA metadata and
//! bootstrap stores. Feature-gated (`config-space`) so the OTA core itself
//! stays independent of any particular persistence backend -- these are
//! convenience bindings for callers that do use `iobewi-config-space`, not
//! a new storage abstraction.

use alloc::vec::Vec;
use iobewi_config_space::{ConfigBackend, ConfigSpace};

/// Adapts one `ConfigSpace<B>` claim to the portable
/// [`crate::metadata::MetadataStore`] boundary. Behavior is unchanged from
/// the prior per-platform copies: `load()`'s snapshot bytes pass through
/// as-is (its generation is not part of the OTM1 wire format), and
/// `commit()` discards the returned generation the same way.
pub struct ConfigSpaceMetadataStore<'a, B: ConfigBackend>(pub &'a ConfigSpace<B>);

impl<B: ConfigBackend> crate::metadata::MetadataStore for ConfigSpaceMetadataStore<'_, B> {
    type Error = ();

    async fn load_raw(&self) -> Result<Option<Vec<u8>>, Self::Error> {
        self.0
            .load()
            .await
            .map(|snapshot| snapshot.map(|snapshot| snapshot.data))
            .map_err(|_| ())
    }

    async fn commit_raw(&self, bytes: &[u8]) -> Result<(), Self::Error> {
        self.0.commit(bytes).await.map(|_| ()).map_err(|_| ())
    }
}

/// Adapts one `ConfigSpace<B>` claim to the portable
/// [`crate::bootstrap::BootstrapStore`] boundary. Same `load()`/`commit()`
/// behavior as [`ConfigSpaceMetadataStore`]; kept as a distinct type so
/// each store's claim stays isolated per IOBEWI ConfigSpace's own model.
pub struct ConfigSpaceBootstrapStore<'a, B: ConfigBackend>(pub &'a ConfigSpace<B>);

impl<B: ConfigBackend> crate::bootstrap::BootstrapStore for ConfigSpaceBootstrapStore<'_, B> {
    type Error = ();

    async fn load_raw(&self) -> Result<Option<Vec<u8>>, Self::Error> {
        self.0
            .load()
            .await
            .map(|snapshot| snapshot.map(|snapshot| snapshot.data))
            .map_err(|_| ())
    }

    async fn commit_raw(&self, bytes: &[u8]) -> Result<(), Self::Error> {
        self.0.commit(bytes).await.map(|_| ()).map_err(|_| ())
    }
}
