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

# iobewi-esp-nvs

## Summary

ESP NVS view over an exclusively borrowed existing flash owner.

## Responsibilities

Bridge esp-nvs NOR operations and ROM CRC to EspFlash without constructing another physical flash instance.

## Non-responsibilities

Namespaces, ConfigSpace keys/framing, quotas, migrations, health policy and ownership of the physical flash.

## Architecture

Platform adapter between esp-nvs and drivers/flash/esp32. The caller acquires SharedFlash access before opening the view.

## Public API

`NvsPartition { offset, size }`, `NvsPartition::new`, `NvsFlash` and `open(&mut EspFlash, NvsPartition)` returning a borrowed `Nvs<NvsFlash>`.

## Invariants

- `INV-004`
- `INV-005`

## Validation

Build through targets/esp32 for the selected chip; BG-STORAGE validates the shared flash path and hardware NVS operations when implementation changes.

## Known limitations

The NVS view must not outlive the exclusive flash borrow. Partition discovery and validation are caller responsibilities; open does not discover partitions. No host execution of the ESP ROM CRC path.

## Related components

`drivers/flash/esp32`, `fs/nvs/core`, `fs/nvs/config-esp32`.
