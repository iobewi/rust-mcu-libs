# ESP second-stage bootloader composition

ROM -> this bootloader -> selected system A image. ota_0/ota_1 are its physical
ESP slots; supervised B/C domains do not use this boot selection.

This ports the audited iobewi_old second stage, reusing current common boot
selection, image validation and metadata transactions. ROM primitives, memory
maps and unchanged historical linkers live in arch/esp32/firmware/boot. Boot
and application share the actual erase/body/readback/commit-last/readback
executor; the application additionally protects its sole confirmed fallback.
Boot must remain able to invalidate that record when the image cannot boot.

The executable clears flashboot watchdog bits, initializes esp-hal, validates
the full table, plans boot, writes the transitions, copies validated RAM
segments, maps DROM/IROM and jumps. Failure halts. The `otadata` record format is version 2 (magic `BOOT`);
OTM1/2 bytes are unchanged. `otadata` that is neither blank nor a valid record
halts (reason 3) instead of being re-seeded: see
`docs/firmware/boot-metadata-reprovisioning.md`. Standard ESP-IDF boot does not
implement this contract.

## Building

Use cargo +nightly build for C3 (riscv32imc-unknown-none-elf) or cargo +esp build
for S3 (xtensa-esp32s3-none-elf), with -Z build-std=core,alloc,
--profile bootloader, -p iobewi-esp-bootloader, --bins and --features esp32c3 or
esp32s3. S3 linking requires Xtensa GCC on PATH.

esptool 5.5.0 elf2image packages the ELF (ESP32-S3: `--flash-mode dio`, the default QIO
image is not loadable by the ROM and the CI rejects it). Boot image is flashed at **0**, table
at the configured offset (**0x8000** by default, **0x10000** for the wide ESP32-S3 geometry, see
`docs/firmware/flash-geometry.md`); do not use an ESP32 classic 0x1000 offset. The offset is
`ESP_BOOTLOADER_ESP_IDF_CONFIG_PARTITION_TABLE_OFFSET`, the same variable the application build
reads: set it for every binary of a flash. The declarative CI esp32-bootloader profile links,
packages, inspects the ESP checksum/footer and checks the image against the limits of the
geometry (`.github/scripts/esp_geometry.py`: the image must end before the table, carry the
stamp of that geometry and fit its RAM windows). Historical S3 ROM/cache/stack
link assertions remain. A 16MB packaging header tests encoding, not physical
16MB qualification; downstream flashing needs the actual capacity/layout.

## Remaining gates

Verify::Structure preserves historical application image checks; checksum,
SHA and authenticity verification are not provided by this setting. The
bootloader image's own ESP checksum/footer is a separate packaging check.
Full verification exceeded 32 KiB in the audit and needs a separate decision.

Application startup must arm its probation watchdog after HAL/TIMG0/runtime
init, then self-check and confirm/reject. A hang before arming remains a
qualification boundary. No rescue image or configuration reset is added.
No hardware is attached: first/confirmed boot, unconfirmed reboot, image
corruption, power cuts, stack use and MMU/cache handover remain device gates.
This composition does not complete product OTA endpoints or supervised
runtime/backend wiring.

## Console

Diagnostics go through `iobewi-esp-console`'s primary console (UART0 by default, `IOBEWI_CONSOLE_PRIMARY`, features `console-*`), shared with the application and stamped in the image; the second stage links no logger. See `docs/firmware/console.md`.
