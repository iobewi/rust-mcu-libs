# Transactional firmware boot

Portable, allocation-free A/B boot policy: decode historical v1 otadata entries, select a boot candidate, activate, confirm and reject while preserving the last validated fallback.

The on-flash magic `EWBT` and entry layout remain unchanged for compatibility. This crate is a **boot decision engine**, not a flash driver or the bootloader itself. It is typically used for firmware domain A on ESP32; it does **not** impose a system/workload role on A or B, nor a second firmware domain.

The adapter must execute erase/body/readback/commit in the declared order and provide image validation. Host tests reproduce interrupted operations and recovery behavior; hardware qualification on ESP32 remains required.

See `docs/firmware/transactional-ab-boot.md` and `docs/firmware/dual-ota.md`.
