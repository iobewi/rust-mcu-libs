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
- `esp32`: cross-compile the library for ESP32-C3 and ESP32-S3. ESP32-S3 also receives an independent latest-Xtensa build.
- `esp32-bins`: cross-compile all firmware binaries for ESP32-C3 and ESP32-S3, including the independent latest-Xtensa build.

The workflow also enforces `cargo fmt --all -- --check` across the workspace and prohibits tracked `Cargo.lock` files.

`.github/scripts/discover_ci.py` reads the root workspace manifest and validates each member's configuration. A missing or invalid `ci.json` fails discovery; a new crate cannot silently bypass CI. The discovered jobs run as a GitHub Actions dynamic matrix.

This is an initial, intentionally small profile vocabulary. For a new MCU architecture, add a common execution profile and installer once, rather than duplicating workflow YAML per crate. An ESP32-only profile is not a promise of other MCU support.

## Xtensa baseline and latest compatibility

The normal ESP32-S3 crate job uses the reference Xtensa Rust `1.98.1.0` toolchain to keep a reproducible baseline. A **separate mandatory** `xtensa-latest` job matrix runs every ESP32-S3 crate against the latest toolchain advertised by upstream. Both matrices come from `ci.json`; new ESP32 crates are included automatically, without edits to the workflow.

The latest checks are **not** `continue-on-error`; a Rust compatibility regression fails CI. The action receives GitHub's workflow token to authenticate API calls, reducing anonymous GitHub rate-limit failures. An infrastructure download error still fails the job and must be diagnosed, not misreported as a code incompatibility.

Keep the pinned baseline version current by updating this workflow and documenting the new value. A green reference build alone does not establish latest compatibility.

## Run discovery locally

```sh
python3 .github/scripts/discover_ci.py
```

Python 3.11+ is required for `tomllib`.

## Boundaries

Compilation is not a hardware acceptance gate. AP DHCP leases, AP+STA coexistence, power-cut persistence and boot-time behavior still require physical validation.

Profiles are declarative metadata; they are not independently executable shell scripts. This keeps the toolchain setup centralized and avoids executing arbitrary per-crate commands.
