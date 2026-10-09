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

A `prepare-xtensa` matrix installs each Xtensa Rust compiler **once per workflow run**: pinned `1.98.1.0` and upstream `latest`. The job measures and bundles the entire installed toolchain into a **Zstandard-compressed** `.tar.zst` archive (`zstd -6` with multithreaded encoding), then uploads a short-lived GitHub Actions artifact (`xtensa-pinned` / `xtensa-latest`). The normal ESP32-S3 and mandatory `xtensa-latest` build jobs download and decompress their respective artifact and register it using `rustup toolchain link esp`. Neither consumer invokes `espup` nor queries the upstream API. Both matrices come from `ci.json`; new ESP32 crates are included automatically, without edits to the workflow.

The setup and latest checks are **not** `continue-on-error`; a Rust compatibility regression fails CI. The preparation action receives GitHub's workflow token to authenticate API calls. One upstream query per toolchain per run (instead of one per crate) lowers rate-limit pressure. Preparation failures block dependent ESP32-S3 checks. Artifacts are run-scoped and retained for one day. An infrastructure download error still fails the job and must be diagnosed, not misreported as a code incompatibility.

Keep the pinned baseline version current by updating this workflow and documenting the new value. A green reference build alone does not establish latest compatibility.

## Fail-fast quality gates

The workflow runs in ordered stages. **Gate 0:** workspace/CI metadata discovery, tracked lockfile policy and Rust formatting. **Gate 1:** host unit tests and strict Clippy for affected portable crates (`fail-fast: true`). **Gate 2:** only after successful earlier stages, install the Xtensa toolchains and cross-check affected MCU crates against C3, pinned S3 and latest S3. A host failure prevents expensive MCU jobs from being scheduled. If no host crate is affected, the skipped host stage is treated as a valid empty stage; missing required checks or actual failures still block the final `affected-gate`.

## Impact-aware validation

For pull requests and pushes, discovery compares the relevant Git revisions and runs checks only for changed workspace members and their transitive dependents. Cargo's resolved dependency graph is authoritative, including renamed dependencies. Unknown changed files or changes in global CI/architecture files fail closed to full validation. Every workspace member still requires valid `ci.json` regardless of selection.

A scheduled daily workflow and manual dispatch always test **all crates**, including the pinned and latest Xtensa checks, catching upstream changes even if source has not changed (this repository does not commit `Cargo.lock`). The always-running `affected-gate` status succeeds for genuinely empty affected matrices but fails if any selected check fails or discovery breaks. Format and lockfile checks remain global.

The CI configuration intentionally triggers on all PRs; do not add root `paths` filters that leave required GitHub checks pending.

## Run discovery locally

```sh
python3 .github/scripts/discover_ci.py
```

Python 3.11+ is required for `tomllib`.

## Boundaries

Compilation is not a hardware acceptance gate. AP DHCP leases, AP+STA coexistence, power-cut persistence and boot-time behavior still require physical validation.

Profiles are declarative metadata; they are not independently executable shell scripts. This keeps the toolchain setup centralized and avoids executing arbitrary per-crate commands.
