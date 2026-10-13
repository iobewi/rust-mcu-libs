# iobewi-esp-boot

Second-stage ESP32-C3/S3 integration from frozen iobewi_old commit
345378ed1ae11a4814c32ee513f20438291cfba7, for issue #61.

`hw` holds one ROM flash/cache implementation for both chips. RTC watchdog
register offsets are chip-specific. Its unsafe entry points require exclusive
single-core boot execution from IRAM, with interrupts and concurrent flash
owners absent. It is not a runtime driver. Historical linkers and the common
image validator's MemoryMap constants live here. No platform/board framework
is imported.

`layout::discover` uses the borrowed raw entry view from iobewi-esp-partitions
and common full-region/domain-A validators. It selects only ota_0, ota_1 and
otadata; B/C are checked geometrically, with no imposed boot policy. It rejects
duplicate labels, overlap including unrelated partitions, overflow, capacity
violations, sector misalignment, application starts off 64 KiB and partitions
before 0x9000. This deliberately strengthens old last-match-wins discovery.
Buffers are bounded, without allocation. Host tests cover a 16 MiB A/B/C map
and dangerous layouts. MCU features are mutually exclusive.

The actual composition is examples/firmware-bootloader. Startup, boot policy,
image validation and metadata transactions reuse existing bricks. No physical
flash, cache, watchdog or stack-use qualification is claimed.
