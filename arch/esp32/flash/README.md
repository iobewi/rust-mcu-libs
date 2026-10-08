# iobewi-esp-flash

## Summary

Process-wide ESP physical flash ownership and serialized raw access

## Responsibilities

- Provide a single shared physical flash owner with serialized access.

## Non-responsibilities

## Public API

- `init(FLASH)` creates the process-wide flash owner and returns `&'static SharedFlash`.
- `SharedFlash` is an asynchronous Embassy mutex around `EspFlash`.
- `EspFlash` implements the synchronous `ReadNorFlash`, `NorFlash` and `MultiwriteNorFlash` traits. `storage()` exposes the underlying ESP driver while the caller holds exclusive access.

Package features and dependency declarations are canonical in `Cargo.toml`.

## Lifecycle

The composition root must call `init` exactly once per firmware image. All flash consumers share the returned mutex. It is not reentrant: release the guard before invoking another subsystem that locks the same flash, including ConfigSpace.

With the S3 feature, initialization enables `multicore_auto_park`. In esp-storage 0.10, the driver checks whether the other core is running and parks it around writes/erases, then resumes it. This is independent of whether that core runs an IOBEWI Workload; a non-running core needs no parking.

The dependency explicitly enables `esp-storage/critical-section`. Its hardware wrappers execute each ROM read, unlock, write and sector/block erase under `esp_sync::RawMutex`, which masks calling-core interrupts and provides mutual exclusion across cores. On S3, esp-sync 0.3 raises the Xtensa interrupt level to 5 (`rsil ..., 5`) and restores the previous processor state on exit; this is not a guarantee against higher-level/NMI handlers accessing flash. This protection is distinct from second-core parking. The asynchronous `SharedFlash` mutex serializes consumers; its raw mutex only protects lock bookkeeping and does not mask interrupts throughout the awaited guard's lifetime.

Partition and OTA operations borrowing `storage()` retain this driver-level protection. The partition adapter also enables the feature explicitly, so an OTA build without `shared-flash` does not rely on feature unification with this owner.

## Known limitations

ROM calls are synchronous and postpone interrupts while protected. Erase latency can affect Wi-Fi, USB and watchdog service; a human diagnostic campaign observed maxima of 44.6 ms for ROM writes and 37.5 ms for one sector erase at 240 MHz. These are observed values, not a guaranteed maximum; the worst-case bound remains unknown. Small-write provisioning success reported on ESP32-S3 is not evidence for erase/GC or OTA durability. These require the human hardware procedure. The separate second-stage boot ROM primitives do not use esp-storage and are outside this runtime protection.

Repeated initialization is unsupported. Nested acquisition of the shared mutex cannot complete; callers must preserve the lock ordering described above.
