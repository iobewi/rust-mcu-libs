---
layer: platform-adapter
status: implemented
invariants:
  - INV-001
  - INV-020
gates:
  - BG-STORAGE
  - BG-ESP-S3
---

# iobewi-esp-partitions

## Summary

Policy-free ESP-IDF partition table and raw partition helpers

## Responsibilities

- Read and locate ESP-IDF partitions and provide bounded raw partition erase helpers.
- Keep that responsibility inside the `platform-adapter` layer.

## Non-responsibilities

- Does not create an independent physical flash owner.
- Does not select slots or define OTA/rollback policy.
- Does not own product/application composition beyond this crate's stated contract.

## Architecture

This crate lives at `drivers/flash/partitions-esp32` and is classified as **platform-adapter**. Its partition discovery API uses the concrete ESP storage driver; the generic erase helper does not make the crate portable.

## Public API

- `for_each_entry` reads raw partition entries; `find_by_label` matches label, raw type and subtype.
- `find` locates a partition using the ESP-IDF typed partition accessor.
- `erase_range` accepts a `NorFlash` implementation and checks logical range bounds, erase alignment and address overflow.

Partition discovery receives an existing mutable `FlashStorage`; callers obtain it through the shared flash owner. This crate explicitly enables `esp-storage/critical-section`, including when compiled independently of the owner crate (for example OTA without `shared-flash`). ROM flash calls then mask calling-core interrupts; this does not create an additional owner or enable second-core parking. See the flash owner README for the separate parking policy and latency limitations. Package features and dependency declarations are canonical in `Cargo.toml`.

## Invariants

- [INV-001](../../../INVARIANTS.md)
- [INV-020](../../../INVARIANTS.md)

## Validation

- `BG-STORAGE`
- `BG-ESP-S3`

## Known limitations

The ESP-IDF typed accessors used by `find` can panic on unknown partition subtypes. `for_each_entry` and `find_by_label` compare raw entries instead.

## Related components

- [Repository architecture](../../../ARCHITECTURE.md)
- [Repository invariants](../../../INVARIANTS.md)
- `Cargo.toml` for package features and dependency facts.
