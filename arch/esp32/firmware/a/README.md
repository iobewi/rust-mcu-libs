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
binary metadata remain historical. The custom bootloader is ported and build/link/size checked; full product
composition still requires integration and on-board qualification;
compiling this library does not establish end-to-end OTA readiness.


## Optional boot runtime binding

With `runtime` and an MCU feature, `service::BootRuntime` implements the existing
portable BootState/BootSelfCheck/BootWatchdog traits. Use `BootRuntime::new` with
an exclusive mutable SystemBoot borrow, the existing SharedFlash and an
application-provided BootSelfCheck. No dependency on an NVS schema is imposed.
The scoped adapter prevents upload/activation through that owner until dropped.
Its NoopRawMutex serializes the historical &self trait calls on one Embassy
executor; it is not another physical flash mutex or a shared service manager.
The self-check is delegated without holding either mutex.

Construction snapshots actual MMU observation and boot records under one physical
flash lock. Historical Pending-before-Valid classification is retained. As an
explicit integration check, absent records or a state belonging to a different
mapped slot return an error, leaving watchdog handling to the caller. This is
not a claim that the historical bootloader makes that error. During this
exclusive boot phase the slot is fixed; cached outcome is updated only after a
successful confirm/reject. Direct raw flash access outside the owner breaks this
composition contract, as with other SystemBoot operations.

Arm the existing iobewi-esp-watchdog **after** HAL/TIMG0/esp-rtos initialization
and before discovery/NVS/OTM1 awaits. Use common service on_boot, pending_check,
confirm_pending/reject_pending unchanged. Only durable boot confirmation disarms
the watchdog; failed confirmation/rejection or timeout must cause reset with the
watchdog still armed. Do not disable it automatically when dropping the adapter.
The reference composition in examples/firmware-confirmation demonstrates this.
A startup hang before arming is still outside this protection window.

CI metadata explicitly enables runtime on host and MCU: the same classification
and observation checks are tested, and the actual ESP binding is cross-compiled.
