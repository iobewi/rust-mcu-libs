# GitHub Actions

The workflow `rust-mcu.yml` runs for pull requests, pushes to `main`, and manual dispatch.

- **Portable:** formatting, host unit tests and Clippy for the four portable crates.
- **ESP32-C3:** cross-target checks for the ESP ConfigSpace adapter, Wi-Fi driver and both examples (including all three Wi-Fi binaries).
- **ESP32-S3:** equivalent checks using the Espressif Xtensa Rust toolchain.
- **Lockfile policy:** reject committed `Cargo.lock` files. Cargo may generate ignored lockfiles on a runner, but none are checked in.

## Boundaries

This CI checks source compilation; it does not flash hardware, verify AP clients receive DHCP leases, verify persistence after power interruption, or establish runtime readiness.

The workflow has **not been executed yet**. Initial runs may reveal upstream dependency incompatibilities or target-specific build requirements, which should be fixed in focused follow-ups rather than silently weakening checks.

Rust and dependency versions are resolved from the current compatible ecosystem; no `Cargo.lock` is committed.
