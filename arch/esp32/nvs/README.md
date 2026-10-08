# iobewi-esp-nvs

## Summary

ESP NVS view over an exclusively borrowed existing flash owner.

## Responsibilities

Bridge esp-nvs NOR operations and ROM CRC to EspFlash without constructing another physical flash instance.

## Non-responsibilities

Namespaces, ConfigSpace keys/framing, quotas, migrations, health policy and ownership of the physical flash.

## Integration

Integration between esp-nvs and drivers/flash/esp32. The caller acquires SharedFlash access before opening the view.

## Public API

`NvsPartition { offset, size }`, `NvsPartition::new`, `NvsFlash` and `open(&mut EspFlash, NvsPartition)` returning a borrowed `Nvs<NvsFlash>`.

## Known limitations

The NVS view must not outlive the exclusive flash borrow. Partition discovery and validation are caller responsibilities; open does not discover partitions. No host execution of the ESP ROM CRC path.
