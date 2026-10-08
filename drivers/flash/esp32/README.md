---
layer: platform-adapter
status: implemented
invariants:
  - INV-004
  - INV-005
gates:
  - BG-STORAGE
  - BG-ESP-S3
---

# iobewi-esp-flash

## Summary

Process-wide ESP physical flash ownership and serialized raw access

## Responsibilities

- Own the capability, policy or platform mechanism described in the summary.
- Keep that responsibility inside the `platform-adapter` layer.

## Non-responsibilities

- Does not redefine portable policy that belongs in platform-independent contracts.
- Does not own unrelated product/application composition.

## Architecture

This crate lives at `drivers/flash/esp32` and is classified as **platform-adapter**. It implements platform-specific behaviour behind IOBEWI boundaries.

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

## Invariants

- [INV-004](../../../INVARIANTS.md)
- [INV-005](../../../INVARIANTS.md)

## Validation

- `BG-STORAGE`
- `BG-ESP-S3`
- `python3 tools/ci/check_esp_storage_features.py --locked` checks isolated ESP32-S3 consumer graphs; CI also runs it with newly resolved dependencies.
- [Flash regression and human hardware procedure](../../../docs/validation/esp-flash-critical-section.md)

## Known limitations

ROM calls are synchronous and postpone interrupts while protected. Erase latency can affect Wi-Fi, USB and watchdog service; a human diagnostic campaign observed maxima of 44.6 ms for ROM writes and 37.5 ms for one sector erase at 240 MHz. These are observed values, not a guaranteed maximum; the worst-case bound remains unknown. Small-write provisioning success reported on ESP32-S3 is not evidence for erase/GC or OTA durability. These require the human hardware procedure. The separate second-stage boot ROM primitives do not use esp-storage and are outside this runtime protection.

Repeated initialization is unsupported. Nested acquisition of the shared mutex cannot complete; callers must preserve the lock ordering described above.

## Related components

- [Repository architecture](../../../ARCHITECTURE.md)
- [Repository invariants](../../../INVARIANTS.md)
- `Cargo.toml` for package features and dependency facts.
