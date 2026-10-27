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

`host-esp32s3-linked-bins` executes host tests/Clippy and links S3 release binaries. Use for the concrete dual-core native composition and its separately linked IWNI workload; it does not claim native execution on C3.

- `host`: host unit tests and Clippy on all targets with warnings denied.
- `esp32s3`: cross-compile an S3-only library, including the independent latest-Xtensa build when required by the event. Native OTG is absent on C3; do not advertise a C3 USB implementation.
- `esp32`: cross-compile the library for ESP32-C3 and ESP32-S3. ESP32-S3 also receives an independent latest-Xtensa build.
- `host-esp32`: execute host unit tests/Clippy without MCU features **and** cross-compile the same library for C3/S3. Use for adapters with host-testable production logic and optional MCU bindings.
- `esp32-bins`: cross-compile all firmware binaries for ESP32-C3 and ESP32-S3, including the independent latest-Xtensa build.

The workflow also enforces `cargo fmt --all -- --check` across the workspace and prohibits tracked `Cargo.lock` files.

`.github/scripts/discover_ci.py` reads the root workspace manifest and validates each member's configuration. A missing or invalid `ci.json` fails discovery; a new crate cannot silently bypass CI. Discovery produces a host-test matrix and per-chip lists of affected MCU crates. Host tests remain a matrix; each MCU list is compiled sequentially in one runner per target/toolchain variant.

This is an initial, intentionally small profile vocabulary. For a new MCU architecture, add a common execution profile and installer once, rather than duplicating workflow YAML per crate. An ESP32-only profile is not a promise of other MCU support.

## Xtensa baseline and latest compatibility

For each event, the ESP32-S3 job installs each required Xtensa compiler **once per runner**, then checks all preselected ESP32-S3 crates sequentially on that same runner. No toolchain archive is uploaded, downloaded, or distributed to individual crate jobs. The pinned baseline is `1.98.1.0`. PRs run affected ESP32-C3 and pinned ESP32-S3 checks; pushes to `main` include `latest` for affected crates; daily schedules and manual dispatch run **all workspace crates** with both pinned and latest S3 toolchains.

The ESP32-C3 job similarly checks all selected C3 crates on one runner. Each package invocation keeps its `--lib` or `--bins` mode, selected from `ci.json`, and each compilation gets a separate collapsible GitHub log group. The jobs stop on the first failed check. Rust's build directory is reused across checks on the same runner.

Toolchain installation failures and compiler compatibility regressions fail the required final gate. The Xtensa install action receives the workflow token for authenticated GitHub API access. No upstream toolchain distribution is attempted per crate.

## Validation by GitHub event

- **Pull request:** Gate 0, host tests/Clippy for affected crates, ESP32-C3 and pinned ESP32-S3 for affected MCU crates. Skip the latest Xtensa compiler download and job; the final gate explicitly accepts that intentional skip.
- **Push to `main`:** validate affected crates, including pinned and latest Xtensa where ESP32-S3 is affected.
- **Nightly schedule / manual dispatch:** validate every workspace crate, including pinned and latest Xtensa.

Adding a crate to `workspace.members` or `default-members` does not alone trigger every MCU build: discovery compares workspace configuration across revisions and selects the changed members and their dependents. Other changes to the root workspace configuration still trigger full verification. Changes to CI files are globally impactful and deliberately run all *event-required* checks. A post-merge failure is detected but cannot prevent an already completed merge; MCU checks on affected code remain mandatory before merging.

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

## Bootloader link and image gate

esp32-bootloader is a declarative profile: build/link with the workspace
bootloader profile, package every binary with esptool 5.5.0, inspect the ESP
image and enforce the historical 32768-byte limit (image at 0, table at 0x8000).
MCU execution is centralized in scripts/compile_mcu.py. Discovery runs
scripts/test_compile_mcu.py for exact-limit, empty and oversized images,
including the historical 33488-byte Full-verification overflow. This does not
qualify hardware or application-image authenticity.

## Configured feature and link checks

ci.json may also declare `"features": ["runtime"]`. Features are validated names,
forwarded to host tests/Clippy and combined with the MCU feature by the common
executor. A's runtime adapter uses this so CI does not only compile its default
layout surface. Impact discovery resolves Cargo metadata with all features to
include optional consumers in reverse dependency selection; builds still use
each crate's declared profile/features, not all features simultaneously.

`esp32-linked-bins` links every binary in release on C3/S3 using its normal
application linker. Unlike `esp32-bootloader`, it has no universal 32 KiB image
limit. The firmware/ota-system reference uses it to catch unresolved timer,
allocator, runtime and linker symbols that a cargo check would miss.

`host-esp32-linked-bins` combines host unit tests/strict Clippy without MCU features with actual C3/S3 release binary links. Firmware dependencies must be optional, enabled by the MCU/firmware features. The OTA reference uses this to execute its request-admission tests without weakening target link gates.

## Validation domains: general CI and firmware/OTA CI

Two workflows share one Cargo workspace, one `ci.json` vocabulary and the same scripts:

| Workflow | Final gate (required-check name) | Scope |
|---|---|---|
| `rust-mcu.yml` | `Affected crate validation` | every package outside the firmware/OTA domain |
| `firmware-ota.yml` | `Firmware OTA validation` | `common/firmware/**`, `arch/esp32/firmware/**`, `examples/firmware/**` and `tools/component-pack` (the OTA component packer, which depends only on firmware crates) |

`discover_ci.py --domain firmware-ota|general|all` (default `all`) partitions the workspace by directory (`FIRMWARE_OTA_DIRS`). **Selection is computed on the whole workspace first** (changed members plus Cargo's reverse dependency graph) and only then restricted to the domain. A change in a shared crate (flash, crypto, net-io, config-space...) therefore selects its consumers in both domains and triggers both workflows; a firmware-only or non-firmware-only change triggers only its own domain's jobs. Global files (`.github/`, `Cargo.lock`, `rust-toolchain.toml`, `.cargo/`, root docs) and unknown root files select every package, so both workflows validate in full. Both workflows run on every pull request (no `paths` filter), so a required gate is never left pending.

Checks specific to the OTA domain run only in `firmware-ota.yml`:

- `cargo test --release -p iobewi-firmware-boot` (exhaustive transactional boot fault injection);
- `iobewi-firmware-update` tests and strict Clippy with `runtime`, then `runtime config-space`;
- the `esp32-bootloader` profile (link, `esptool` image, 32768-byte gate) and `test_compile_mcu.py`;
- the linked-binary profiles of the OTA reference firmware, on ESP32-C3 and ESP32-S3 (pinned Xtensa on pull requests, pinned and latest otherwise).

The workflow runs nightly (03:47 UTC, full) and on `workflow_dispatch` (full). `test_discover_ci.py` checks the domain partition and the selection scenarios; `test_ota_gate.py` checks the gate logic (success, failure, cancellation, skipped-by-mistake, discovery failure, empty matrix).

### Gate semantics

`Firmware OTA validation` always runs. It fails unless discovery and rustfmt succeeded, discovery outputs are well-formed, every selected job succeeded, and every unselected job was skipped (a failed or cancelled job is never accepted as "not selected"). An empty selection is accepted on pull requests and pushes (nothing OTA is affected) but is an error on scheduled and manual full runs. `Affected crate validation` keeps its name and semantics for the general domain.

### Branch protection

Required status checks are a repository setting and are not changed by these workflows. When the OTA workflow is introduced, add `Firmware OTA validation` as a required check next to `Affected crate validation`. Keep both: each reflects only its own domain. Do not require the individual matrix job names.

A green CI proves host tests, Clippy and compilation/link of the selected crates. It is not a hardware qualification (real power cuts, flash wear, Secure Boot).

### Migration status

The OTA-specific checks (boot fault injection, `firmware-update` feature sets, bootloader image gate and its `test_compile_mcu.py` self-test, OTA reference firmware links) run only in `firmware-ota.yml`. `rust-mcu.yml` discovers with `--domain general` and no longer schedules any OTA crate. Sections above that describe the single-workflow behaviour apply to each workflow within its own domain.
