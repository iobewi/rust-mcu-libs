# ESP32 system firmware A

The production adapter re-exports the sole host-tested A layout validator from
`iobewi-firmware-layout::domain_a`; there is no copied validation algorithm.
Labels remain `ota_0`, `ota_1`, `otadata`, with 64 KiB application-start and 4 KiB
erase alignment. The crate has both host and C3/S3 CI checks (`host-esp32`).

## SystemBoot composition

With an MCU feature, `platform::SystemBoot::discover(shared_flash)` validates the
entire partition table against the driver's capacity. `reserve(shared_flash)`
reads the actual mapped application through the upstream `booted_partition()`
API already used by the historical implementation, plus transactional metadata
under the same flash lock. It checks the historical inactive target against the
observed image before returning the portable exclusive write guard.

Use `storage::shared_flash::ArtifactWriteSession::begin(guard.permit(), size,
digest)` to stream the artifact. Keep the guard through verification and metadata
publication. The physical flash mutex is released after each operation, before
ConfigSpace accesses. `activate`, `confirm` and `reject` require mutable access
to the same `SystemBoot`, so they cannot execute while its write guard exists.
Each metadata read/plan/execute sequence keeps one physical flash lock.

The application must own one `SystemBoot` for this physical domain, route all
activation through it, publish only a verified artifact through the common
service, perform the self-check, and arrange reboot/watchdog handling. Public
low-level flash/metadata access is not an application authorization system.

No rescue image or automatic configuration reset is added. Rollback policy and
binary metadata remain historical. The custom bootloader and full product
composition still require porting/integration and on-board qualification;
compiling this library does not establish end-to-end OTA readiness.
