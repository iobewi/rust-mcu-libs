# Architecture and coding conventions

This document defines how code is divided, assembled and qualified in **iOBEWi Rust MCU Library**. Read [README.md](README.md) for the project philosophy and [AGENTS.md](AGENTS.md) for the contributor/agent checklist.

## Principle

**Reuse existing crates, place each responsibility where it belongs, and assemble only what the product actually needs.** The directory tree is a map of ownership, **not a mandatory stack of layers**.

The design has three kinds of code:

| Location | Owns | Must not own |
| --- | --- | --- |
| `common/` | MCU-independent, independently useful capability and contracts justified by actual consumers | Chip HAL, flash addresses, partition selection, board initialization, product-specific policies |
| `arch/<family>/` | Real MCU-family implementations and integrations, including hardware-selected bindings between portable capability and physical technology | Product-specific configuration schema, business lifecycle, universal framework |
| `examples/` | Runnable product-like composition, MCU selection, board startup, user-facing example behavior | Reimplementing library internals, reusable backend logic that belongs under `arch/` |

`arch/` is **not just a drivers folder**. It contains the platform implementation of in-house functionality where a concrete hardware choice is made.

Neither these locations nor their subdirectories must be created in advance. Add them only with real code and a reason.

## Dependency direction

The usual dependency direction is:

```text
examples/<use-case>/src/app.rs
      |
      v
common/<capability>/         <-- pure functionality and necessary contracts
      ^
      |
arch/<family>/<capability>/  <-- implements a concrete backend
      ^
      |
examples/<use-case>/src/platform/<family>.rs
      |
      +--> upstream HAL / runtime for bootstrapping
```

The diagram shows **ownership and use**, not a demand that every capability define a generic trait. A module can call a mature upstream crate directly. Avoid indirection that adds no behavior.

### `common/`: portable capability

- Define the core behavior and public API of an in-house capability.
- Define a trait only when a **real, distinct implementation boundary** exists and the trait has useful operations. Reuse upstream embedded traits when adequate.
- Prefer `no_std` for embedded reusable code; add `alloc` only when needed and document resource usage.
- Do not select flash, NVS, storage partitions, clocks, interrupts, GPIOs or specific MCU feature flags.
- Do not hardcode application schemas, credentials or provisioning behavior.
- Do not depend on `arch/` or on an example.

**Example:** `common/config/space` defines `ConfigManager`, `Budget`, `ConfigSpace` and `ConfigBackend`. It handles unique claims, quotas and access handles, not ESP NVS internals.

### `arch/<family>/`: concrete MCU implementation

- Implement only the hardware-specific functionality needed by a real portable component or use case.
- Reuse vendor HALs, Embassy, `embedded-*` traits and maintained crates rather than rebuilding them.
- Own the **hardware-selected technology binding**, not just the bare device driver.
- Hide storage internals from the caller where doing so creates concrete value, without concealing every useful upstream API behind wrappers.
- Do not add business configuration fields or assume all products use the same provisioning policy.
- Manage shared hardware explicitly: one physical flash owner can be used by configuration and OTA; never silently construct competing owners.
- Expose APIs suitable for two real composition patterns: a standalone sample and a product which already owns shared resources. Keep both APIs only when genuinely useful.

**Example:** `arch/esp32/config/space` implements `ConfigBackend` using NVS. It discovers the NVS partition and performs record handling using existing `arch/esp32/fs/nvs`, `flash`, and `flash/partitions` implementations. It may offer a standalone constructor `from_flash(flash, "nvs")` and a shared-owner constructor `from_label(shared_flash, "nvs")`.

**Ownership caveat:** calling `from_flash` initializes the process-wide flash owner; it must only be done once. A firmware with OTA or other flash consumers initializes the owner at the composition root and passes it to `from_label`. Never acquire multiple physical owners just to simplify a call site.

### `examples/`: real compositions, not simulations

- Provide a small runnable example using actual MCU peripherals and the relevant target toolchain.
- Keep the feature/use-case scenario separate from MCU bootstrap where that improves real reuse: `src/app.rs` for the scenario, `src/platform/<family>.rs` for startup and concrete wiring.
- Product/board code owns the startup sequence: chip setup, interrupts, Embassy executor, heap sizing if used, and distributing resource handles.
- The application **uses** a ready platform backend; it should not reimplement NVS records, partition parsing or storage locking.
- It is acceptable for `platform/<family>.rs` to use `esp-hal` or the matching HAL to start the firmware. **Do not move initialization of the entire MCU/Embassy runtime into a configuration-storage crate.**
- Examples may demonstrate real operational constraints, boot behavior and failure paths. They are **not production-qualified** merely because they compile.

**Example:** `examples/config-manager` has a portable scenario, an ESP32 implementation, and feature-selected source. It persists `config-ready` in a named NVS partition across boots.

## Portable logic and conditional MCU builds

Use Rust `#[cfg(...)]` plus **Cargo features** to select concrete implementations at compile time, the equivalent of C conditional compilation:

```rust
#[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
#[path = "platform/esp32.rs"]
mod platform;

#[cfg(feature = "rp2040")]
#[path = "platform/rp2040.rs"]
mod platform;
```

This is an **illustration**: do not advertise `rp2040`, `rp2350` or `teensy41` as supported until their module, dependencies, target and builds are implemented.

Rules:

1. Choose exactly **one MCU target** for a firmware build. Reject incompatible feature combinations with `compile_error!`.
2. Keep platform-only dependencies conditional/optional as necessary, so they are not compiled for unrelated MCUs.
3. Put MCU selection in the entry/composition code; never sprinkle chip conditions throughout the business scenario.
4. Prefer an existing trait when an actual portability seam needs one. Do not create a generic HAL, universal resource manager, macro framework or startup facade for imagined future devices.
5. The actual persistent technology is an `arch/` implementation decision: ESP32 may use NVS, while an eventual RP or Teensy implementation may choose something else without imposing that choice on `common/`.
6. Chip feature flags do not replace correct target triple, linker setup, runtime configuration or hardware qualification.

## Configuration: one backend, multiple spaces

Example flow:

```text
firmware boot (ESP32 + Embassy)
    |
    +--> initialize one shared flash owner
    +--> obtain NvsConfigBackend once
    +--> create ConfigManager once
             |
             +--> claim("wifi", Budget::new(...)) --> Wi-Fi component
             |
             +--> claim("server", Budget::new(...)) --> HTTP/URL component
```

- Make expected claims in a deterministic boot sequence. Claims reserve capacity for **that boot**; persisted values remain stored between boots.
- Each component receives its own `ConfigSpace` handle, owns its schema and decides when to `load`, `commit` or `clear`.
- The backend handles physical capacity accounting, replacement and storage-specific failure semantics; the manager does not know NVS.
- Do **not** overwrite stored configuration with defaults at every startup.
- Choose storage sizes and failure handling from the actual use case. Avoid persisting secrets in cleartext without a threat model and appropriate protection.
- A single backend may serve many spaces; it is not one backend or manager per configuration field.

## New component or existing dependency?

Before adding a crate, ask:

1. Is this capability already provided by a maintained, suitable upstream Rust crate?
2. What **specific missing behavior** will our code contribute?
3. Is that behavior portable logic (`common/`), hardware-specific binding (`arch/`), or product composition (`examples/` or an application repository)?
4. Can it be implemented in an existing crate without creating another dependency boundary?
5. What is the smallest observable test and example that prove it works?

**If a wrapper only renames an upstream API, do not introduce it.** For instance, use `picoserve` directly when it meets the HTTP server need. Add an integration only for a real gap.

Organize subdirectories by **capability** (e.g. `config/space`, `fs/nvs`, `flash/partitions`), not by automatic layer generation. Structural symmetry may improve discovery but does not require fake equivalents on every platform.

## Coding conventions

- **Rust:** safe by default. Any `unsafe` needs a local `// SAFETY:` explanation of concrete invariants, and review.
- **Memory:** prefer bounded structures for constrained devices; document worst-case usage and allocation. `alloc` is allowed when its costs are understood.
- **Concurrency:** use Embassy for async embedded execution, synchronization and timing when appropriate. Do not implement a competing executor or block inside async tasks. Avoid holding a mutex across unrelated `.await` points.
- **Ownership:** peripherals and shared resources have explicit owners. Do not recreate drivers behind a shared owner's back.
- **Persistence:** keep partition discovery and geometry checks in `arch/`, avoid hard-coded flash offsets, do not erase/reformat storage implicitly when opening.
- **Errors:** prefer explicit `Result` and actionable errors for libraries. Examples may fail fast deliberately but must explain consequences and avoid destructive recovery.
- **API:** implement the narrow operation set a consumer needs; keep unnecessary public types, features and generic parameters out of the contract.
- **Dependencies:** use latest stable mutually compatible ecosystem releases; record explicit temporary compatibility pins and their reason. `Cargo.lock` remains ignored for this library repository.
- **Documentation:** describe actual public calls, responsibilities, prerequisites, limits and the verified status. Do not claim unimplemented targets or production readiness.

## Universal CI contract for workspace crates

Every crate in the Cargo workspace—portable capability, MCU-specific adapter or executable reference example—must ship with a `ci.json` alongside its `Cargo.toml`. The shared GitHub Actions workflow discovers all workspace members and uses the per-crate profile to schedule the correct host or MCU checks. Missing/invalid metadata fails CI; crates must never be omitted silently. See [.github/README.md](.github/README.md) for the supported profiles, extension rules and discovery command. This is a verification contract, **not** a new runtime abstraction or a substitute for physical hardware qualification.

## Verification and change discipline

For a meaningful change:

1. Check existing APIs and upstream options before coding.
2. Run formatting and host/unit tests for portable code.
3. Compile all affected chip configurations using appropriate Rust toolchains, linker scripts and feature sets; verify mutual exclusion of MCU features.
4. Check RAM/flash resource assumptions when relevant.
5. Test on physical hardware when touching flash, reset, boot, OTA, radio, interrupts, clocks, timing or other hardware-dependent behavior.
6. For persistent storage, verify first boot, subsequent cold boot, failure paths and interruption/recovery proportionate to the stated guarantees.
7. Report *what actually ran*, the commit/target, outcome, and outstanding qualifications. Never describe compilation or hardware tests as passed if unrun.

The repository examples are **reference implementations whose readiness is earned by evidence**, not tutorials assumed correct by construction.

## Decision rule

**`common/` states what an in-house feature does; `arch/` supplies the MCU-specific implementation and physical integration; `examples/` boots the board and demonstrates a real composition. The product decides which bricks to use.**

When unsure, favor the simpler dependency graph and the least new code.