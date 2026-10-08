# iobewi-esp-config-space

## Summary

ESP NVS adapter for IOBEWI ConfigSpace

## Responsibilities

- Implement the portable ConfigSpace backend contract using ESP NVS and the shared ESP flash path.
- Use portable NVS record and reservation calculations from `iobewi-nvs-core`.

## Non-responsibilities

- Does not create or own an independent physical flash instance.
- Does not define product configuration policy.

## Public API

`NvsConfigBackend::from_flash(flash, label)` provides one-step flash ownership initialization and NVS discovery for a standalone firmware. Call it once per firmware image. A product already sharing flash with other components must initialize the owner at its composition root and use `NvsConfigBackend::from_label(shared_flash, label)` instead; never initialize a second physical owner.


`NvsConfigBackend` implements `ConfigBackend`. Construction receives an existing `&'static SharedFlash` and `NvsPartition`; flash access is serialized through that shared owner. `is_healthy` and `self_check` expose backend health checks. `NvsPartition` and NVS capacity constants are re-exported for composition.

Package features and dependency declarations are canonical in `Cargo.toml`.

### Discovered startup

NvsConfigBackend::from_label(shared_flash, label).await discovers a DATA/NVS partition using the existing partition helper, validates erase geometry against actual flash capacity, releases the shared lock and reuses new(). Errors distinguish discovery, geometry and backend initialization. There is no hardcoded address fallback, new physical flash owner or explicit erase/reformat-on-failure path. Existing esp-nvs open/recovery semantics still apply; initialization is not a guarantee of read-only flash access. Missing partitions/backend failures stop Board startup before any native USB constructor.

## Known limitations

Platform-specific runtime behavior and flash durability require validation on the target ESP32 hardware.
