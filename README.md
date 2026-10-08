# iOBEWi Rust MCU Library

**Small Rust building blocks. Proven compositions. No framework to adopt.**

**iOBEWi Rust MCU Library** (`rust-mcu-libs`) is a collection of reusable Rust implementations and practical reference examples for microcontrollers.

The goal is simple: **build applications by assembling existing, well-designed crates and a few focused pieces of code that solve the missing use cases.**

We do not aim to replace the Rust embedded ecosystem, wrap every dependency behind our own API, or impose a universal application architecture.

> **Use the bricks. Build only the missing piece. Keep the application in control.**

## Philosophy

### 1. Reuse before writing

Before adding a component, look for an existing Rust crate that already does the job.

- If it fits, **use it directly**.
- If it almost fits, implement the smallest useful integration or adapter.
- If the capability is genuinely missing, implement a reusable, focused building block.
- Do not create a wrapper merely to hide a dependency, rename its API, or speculate about a future replacement.

**Example — HTTP:** [picoserve](https://github.com/sammhicks/picoserve) already provides an HTTP server. An iOBEWi component may adapt an existing connection to picoserve, or demonstrate a carefully tested streaming use case. It should not reimplement routing, requests, and responses just to claim ownership of the HTTP API.

### 2. Compose rather than prescribe

Each building block should solve **one identifiable problem**, expose an understandable API, and be usable independently of unrelated components.

Applications select their dependencies and assemble them according to their needs. Validated examples may demonstrate how multiple bricks fit together; they do not become mandatory services or an application framework.

**Example — OTA:** an update flow may compose download/streaming, integrity and authenticity checks, inactive-slot writing, boot selection, confirmation, and rollback. The library can provide the required bricks and a reference composition that enforces safety-critical ordering. It does not require every application to adopt a monolithic OTA service.

### 3. Keep business logic hardware-agnostic

**The closer code is to application behavior, the less it should know about a specific MCU.**

Keep hardware-specific code at the boundary where it is needed. Favor the existing Rust embedded traits, drivers, and HALs when they cover the use case.

Hardware portability matters. Arbitrary interchangeability of every third-party software crate is **not** a goal in itself.

### 4. Embassy is an explicit choice

[Embassy](https://embassy.dev/) is our chosen async embedded execution ecosystem. We embrace it where asynchronous execution, timing, synchronization, or device integration calls for it.

We do **not** build a replacement runtime or abstraction layer solely to hide Embassy.

Conversely, a synchronous algorithm or pure data structure should not depend on Embassy without a concrete reason.

### 5. Quality is part of the brick

A reusable component is not just code that compiles. It should have clearly stated behavior, constraints, and evidence.

We strive for:

- **Safe Rust by default.** Any `unsafe` must have a concrete justification, documented invariants, and focused review.
- **`no_std` where appropriate**, without making it a blanket requirement for host tools or tests.
- **Predictable resource usage** on constrained MCUs: bounded buffers, explicit allocation choices, and controlled error paths.
- **Small, honest APIs** with explicit failure modes and no unnecessary indirection.
- **Tests proportional to the risk:** host tests, integration tests, and hardware qualification where applicable.
- **Runnable usage examples** and documentation of assumptions and limitations.

Neither Rust nor a successful test suite can guarantee a component is “100% safe.” We prefer explicit guarantees supported by design, testing, and review over absolute claims.

### 6. Stay current with the Rust embedded ecosystem

**The latest stable, compatible upstream releases are our default target**, especially for Rust, Embassy, `esp-hal`, HALs, embedded traits, and related crates.

- **Latest-first development:** use current stable releases when implementing or updating a component. Do not pin an old version just because it was used in a previous project.
- **Compatibility must be demonstrated:** check the versions together through real compilation, tests, and target-specific builds. "Latest" is not a claim that every independently released crate is automatically compatible.
- **Library-first dependency resolution:** `Cargo.lock` is not committed in this repository and is ignored by Git. Dependency compatibility is checked against current upstream releases in CI.
- **Explicit exceptions:** if the newest release is incompatible, document the blocker, the tested working version, and the path to upgrade. Temporary pins must not silently become permanent.
- **Continuous maintenance:** regularly review upstream releases, Rust toolchains, deprecations, and security advisories. Test dependency updates before adopting them; require hardware validation when behavior depends on peripherals, timing, flash, or radio.

**We intentionally do not version `Cargo.lock` in this library repository.** Released firmware or downstream applications may define their own independent reproducibility policies.

**We do not maintain compatibility with obsolete dependency versions by default.** Supporting older versions requires an explicit, justified use case.

## What belongs in this library?

A candidate component should answer four questions:

1. **What concrete use case is not adequately served by existing crates?**
2. **Which existing crates will it reuse?**
3. **What precise value does this implementation add?**
4. **How can we demonstrate and test that value?**

If removing a proposed component would lose nothing beyond a renamed API or an extra layer of indirection, it probably does not belong here.

An adapter can depend directly on the technology it adapts. A picoserve integration **may expose picoserve types**; that is not an architectural failure.

## First areas of focus

Two families of bricks have particular value in embedded applications:

| Area | Intended value |
| --- | --- |
| **Configuration space** | Versioned, consistent, recoverable persistent configuration while reusing existing storage implementations. |
| **Logging** | Controlled log filtering, bounded capture, and optional output/stream integrations while building on Rust's existing logging ecosystem. |

These are **initial priorities**, not claims that crates have already been implemented or migrated to this repository.

Other domains (networking, file systems, device integration, firmware update, etc.) will grow **from concrete applications and missing use cases**, not from a prefilled architecture diagram.

## How an application uses the library

An application should be able to:

1. Choose established ecosystem crates directly.
2. Add only the iOBEWi building blocks it needs.
3. Supply platform-specific implementations at the hardware boundary.
4. Compose a complete feature, following reference examples where useful.

**No required global service manager. No mandatory wrapper around every dependency. No forced adoption of the whole repository.**

## Repository direction

We may retain a familiar, discoverable organization such as `config/`, `log/`, `net/`, `fs/`, `drivers/`, and `examples/`.

These names are **organizational aids, not architecture layers to fill in advance**. Directories and crates are added when a real, reusable implementation warrants them.

Prior implementations from the former iOBEWi project are a source of proven ideas and code, **not an automatic migration list**. Each component must earn its place under the principles above.

## Non-goals

- Reimplementing mature Rust crates.
- A universal API that hides every third-party library.
- An application framework with mandatory lifecycle, runtime services, or orchestration.
- Abstractions designed exclusively for hypothetical portability.
- A large collection of empty crates or speculative interfaces.

## Guiding rule

> **A new iOBEWi brick must solve a real problem, reuse what already works, remain easy to assemble, and make its guarantees verifiable.**

The measure of success is not how much code the library contains. **It is how little application-specific infrastructure developers need to write to build something reliable.**
