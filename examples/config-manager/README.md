# Config manager — multi-MCU reference example

One hardware-independent scenario with a compile-time selected MCU integration. The current implementation runs on ESP32; future RP2040/RP2350 or Teensy implementations can be added without changing `app.rs`.

The example composes existing iOBEWi building blocks rather than wrapping them:

- `common/config/space`: isolated configuration ownership and quotas;
- `arch/esp32/config/space`: configuration persistence over NVS;
- `arch/esp32/fs/nvs`, `flash` and `flash/partitions`: the existing shared ESP32 storage stack;
- `esp-hal` + `esp-rtos` (Embassy executor): initialization and asynchronous execution.

## Demonstrated behavior

On first boot, discover the ESP-IDF **data/NVS partition labelled `nvs`**, claim a 32-byte space named `demo`, and save `config-ready`. On subsequent boots, read back the persisted value. The value is checked; it is **not erased or reformatted**.

No address is hard-coded. The firmware expects a correctly provisioned ESP-IDF partition table with a sufficiently sized NVS partition. It does not create one.

## Source layout

- `src/app.rs`: portable `ConfigManager` scenario, generic over the existing `ConfigBackend` trait.
- `src/main.rs`: `#[cfg]` target selection, with a compile-time error for unsupported selections.
- `src/platform/esp32.rs`: Embassy entry point, ESP flash initialization and NVS backend.

A new MCU gets its own platform module, selected using a Cargo feature. Its required storage adapter belongs under `arch/`, not inside the application logic. **No RP2040, RP2350 or Teensy implementation is claimed yet.**

## Target selection

The Cargo features `esp32c3` and `esp32s3` select the appropriate hardware support. Choose exactly one.

Example build commands (require the relevant target toolchain and ESP32 linker configuration):

```sh
cargo build --release -p example-config-manager --bin config-manager-esp32 --features esp32c3 --target riscv32imc-unknown-none-elf
cargo +esp build --release -p example-config-manager --bin config-manager-esp32 --features esp32s3 --target xtensa-esp32s3-none-elf
```

Flashing requires an ESP-IDF-compatible bootloader and a partition table containing an NVS partition named `nvs`. Install those through the board's normal provisioning/flash flow; do not blindly erase an existing device.

## Qualification status

**Not yet production-qualified.** The firmware was added as source, but host compilation, cross-target compilation, flash/boot, reset persistence and power-loss fault injection have not been executed in this session. The ESP dependency versions follow the imported stack and require a latest-compatible dependency review.

Before claiming production readiness, run chip-specific builds, confirm the physical partition geometry, test a cold restart, corrupted NVS behavior and interrupted writes, and capture serial/diagnostic evidence. The example deliberately uses assertions and halts on failure rather than implementing a product recovery policy.
