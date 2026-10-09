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

`.github/scripts/discover_ci.py` reads the root workspace manifest and validates each member's configuration. A missing or invalid `ci.json` fails discovery; a new crate cannot silently bypass CI. Discovery produces a host-test matrix and per-chip lists of affected MCU crates. Host tests remain a matrix; each MCU list is compiled sequentially in one runner per target/toolchain variant.

This is an initial, intentionally small profile vocabulary. For a new MCU architecture, add a common execution profile and installer once, rather than duplicating workflow YAML per crate. An ESP32-only profile is not a promise of other MCU support.

## Xtensa baseline and latest compatibility

For each event, the ESP32-S3 job installs each required Xtensa compiler **once per runner**, then checks all preselected ESP32-S3 crates sequentially on that same runner. No toolchain archive is uploaded, downloaded, or distributed to individual crate jobs. The pinned baseline is `1.98.1.0`. PRs run affected ESP32-C3 and pinned ESP32-S3 checks; pushes to `main` include `latest` for affected crates; daily schedules and manual dispatch run **all workspace crates** with both pinned and latest S3 toolchains.

The ESP32-C3 job similarly checks all selected C3 crates on one runner. Each package invocation keeps its `--lib` or `--bins` mode, selected from `ci.json`, and each compilation gets a separate collapsible GitHub log group. The jobs stop on the first failed check. Rust's build directory is reused across checks on the same runner.

Toolchain installation failures and compiler compatibility regressions fail the required final gate. The Xtensa install action receives the workflow token for authenticated GitHub API access. No upstream toolchain distribution is attempted per crate.

## Fail-fast quality gates

The workflow runs in ordered stages. **Gate 0:** workspace/CI metadata discovery, tracked lockfile policy and Rust formatting. **Gate 1:** host unit tests and strict Clippy for affected portable crates (`fail-fast: true`). **Gate 2:** only after successful earlier stages, install the Xtensa toolchains and cross-check affected MCU crates against C3 and pinned S3 (plus latest S3 on main and scheduled/manual runs). A host failure prevents expensive MCU jobs from being scheduled. If no host crate is affected, the skipped host stage is treated as a valid empty stage; missing required checks or actual failures still block the final `affected-gate`.

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
