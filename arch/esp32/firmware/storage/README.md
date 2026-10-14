# Shared ESP firmware storage

A single physical writer for every firmware domain, using `SharedFlash` and
`iobewi-firmware-update::WriteSession`. No boot decisions or second OTA engine.

`shared_flash::ArtifactWriteSession::begin(guard.permit(), total, digest)` holds an
exclusive permit borrow and owns the matching engine/erase-watermark state.
It rejects zero/oversized artifacts and non-erase-aligned regions before writing.
`append` and `finish` acquire the physical flash mutex per operation and release
it before the caller publishes metadata in ConfigSpace. Historical 64 KiB erase
ahead and sector-sized durability are preserved.

The guard must outlive verification and publication. Activation is a separate
policy action after dropping the reservation. Slot safety depends on a real
execution observation and a single domain owner; low-level raw flash access is
not a security boundary against application code deliberately bypassing it.

`partitions::discover` reuses the existing parser and checks all physical regions,
including unrelated/future domains and configuration, against flash capacity.
Host tests validate the portable geometry and reservation logic; the ESP flash
writer and actual boot observations still require on-board qualification.
