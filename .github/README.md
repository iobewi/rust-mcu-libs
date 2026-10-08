# GitHub Actions: per-crate CI

Every `[workspace].members` package must contain **`ci.json`** beside its `Cargo.toml`. The root workflow never lists package names or chip combinations.

Example for a portable crate:

```json
{"profile": "host"}
```

Example for an ESP32 driver:

```json
{"profile": "esp32"}
```

Example for firmware binaries:

```json
{"profile": "esp32-bins"}
```

## Profiles and coverage

- `host`: host unit tests and Clippy on all targets with warnings denied.
- `esp32`: cross-compile the library for ESP32-C3 and ESP32-S3.
- `esp32-bins`: cross-compile all firmware binaries for ESP32-C3 and ESP32-S3.

The workflow also enforces `cargo fmt --all -- --check` across the workspace and prohibits tracked `Cargo.lock` files.

`.github/scripts/discover_ci.py` reads the root workspace manifest and validates each member's configuration. A missing or invalid `ci.json` fails discovery; a new crate cannot silently bypass CI. The discovered jobs run as a GitHub Actions dynamic matrix.

This is an initial, intentionally small profile vocabulary. For a new MCU architecture, add a common execution profile and installer once, rather than duplicating workflow YAML per crate. An ESP32-only profile is not a promise of other MCU support.

## Run discovery locally

```sh
python3 .github/scripts/discover_ci.py
```

Python 3.11+ is required for `tomllib`.

## Boundaries

Compilation is not a hardware acceptance gate. AP DHCP leases, AP+STA coexistence, power-cut persistence and boot-time behavior still require physical validation.

Profiles are declarative metadata; they are not independently executable shell scripts. This keeps the toolchain setup centralized and avoids executing arbitrary per-crate commands.
