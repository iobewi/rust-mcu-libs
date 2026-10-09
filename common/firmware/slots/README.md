# Firmware slots

Small, allocation-free, hardware-neutral identifiers for optional firmware domains and their independent two-slot A/B update pairs.

- `Domain::A` and `Domain::B` identify **firmware domains**, not mandatory system/workload roles.
- `Slot::Zero` and `Slot::One` identify the positions of each domain.
- `FirmwareSlot { domain, slot }` can represent A0, A1, B0, B1; `other()` remains in the same domain.
- **A-only** configurations depend on no B service or runtime.
- Physical partition naming, discovery, flash, boot policy and update policy are outside this crate.

**Compatibility:** this generic crate intentionally has no `ota_0`/`ota_1` labels or `PARTITION_LAYOUT`. Those belong to an ESP32-specific legacy layout mapping. The existing flash contents, metadata and layout identifier `embewi-ab-v1` **must not change** during porting. The adapter will preserve that mapping when implemented.

Host tests validate both independent pairs and A-only operation. ESP32 mappings and hardware behavior require later qualification.
