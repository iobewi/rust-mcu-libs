#![no_std]
//! ESP composition for supervised domains (B and additional application IDs).
//! Reservation logic is portable and shared with A; no otadata or second engine.
pub use iobewi_firmware_layout::authorization::SlotLayout;
pub use iobewi_firmware_layout::session::{DomainState, ExecutionState, SessionError, WriteGuard};

/// Start the same ESP streaming session used by every firmware domain.
/// The guard remains borrowed exclusively until the writer is dropped.
#[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
pub fn begin_update<'a>(
    guard: &'a mut WriteGuard<'_>,
    total: u64,
    digest: iobewi_firmware_update::Digest,
) -> Result<
    iobewi_esp_firmware_storage::shared_flash::ArtifactWriteSession<'a>,
    iobewi_esp_firmware_storage::FlashWriteError,
> {
    iobewi_esp_firmware_storage::shared_flash::ArtifactWriteSession::begin(
        guard.permit(),
        total,
        digest,
    )
}
