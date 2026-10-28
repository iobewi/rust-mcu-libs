# Transactional firmware boot

Portable, allocation-free A/B boot policy: decode version 2 otadata entries, select a boot candidate, activate, confirm and reject while preserving the last validated fallback.

The on-flash record is format version 2 (magic `BOOT`); metadata of any other format is never read and never silently replaced (see `docs/firmware/boot-metadata-reprovisioning.md`). This crate is a **boot decision engine**, not a flash driver or the bootloader itself. It is typically used for firmware domain A on ESP32; it does **not** impose a system/workload role on A or B, nor a second firmware domain.

The adapter must execute erase/body/readback/commit in the declared order and provide image validation. Host tests reproduce interrupted operations and recovery behavior; hardware qualification on ESP32 remains required.

See `docs/firmware/transactional-ab-boot.md` and `docs/firmware/dual-ota.md`.

## Shared physical transaction executor

transaction shares erase/body/readback/commit-last/readback between ROM boot
and the application adapter. The existing Flash trait moved here with no HAL
dependency. Callers exclusively own flash for the call and select transitions.
Application sole-Valid fallback protection remains in firmware/a::otadata;
boot must be able to invalidate that record if the image cannot boot. Tests
execute every partial body/commit prefix and check reboot selection.
