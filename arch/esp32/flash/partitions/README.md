# iobewi-esp-partitions

## Summary

Policy-free ESP-IDF partition table and raw partition helpers

## Responsibilities

- Read and locate ESP-IDF partitions and provide bounded raw partition erase helpers.

## Non-responsibilities

- Does not create an independent physical flash owner.
- Does not select slots or define OTA/rollback policy.

## Public API

- `for_each_entry` reads raw partition entries; `find_by_label` matches label, raw type and subtype.
- `find` locates a partition using the ESP-IDF typed partition accessor.
- `erase_range` accepts a `NorFlash` implementation and checks logical range bounds, erase alignment and address overflow.

Partition discovery receives an existing mutable `FlashStorage`; callers obtain it through the shared flash owner. This crate explicitly enables `esp-storage/critical-section`, including when compiled independently of the owner crate (for example OTA without `shared-flash`). ROM flash calls then mask calling-core interrupts; this does not create an additional owner or enable second-core parking. See [`iobewi-esp-flash`](../flash/README.md) for parking policy and latency limitations. Package features and dependency declarations are canonical in `Cargo.toml`.

## Known limitations

The ESP-IDF typed accessors used by `find` can panic on unknown partition subtypes. `for_each_entry` and `find_by_label` compare raw entries instead.
