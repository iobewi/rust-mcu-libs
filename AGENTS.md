# Agent rules — iOBEWi Rust MCU Library

These rules apply to every change in this repository, regardless of the AI agent or development tool. Read [README.md](README.md) and [ARCHITECTURE.md](ARCHITECTURE.md) first. Their philosophy and ownership rules are binding; this file translates them into working rules.

## Objective

Build **small, reusable, high-quality Rust MCU building blocks and reference compositions**. This is a library, **not an application framework**.

## Non-negotiable rules

1. **Reuse first.** Before implementing a feature, inspect existing code and suitable maintained upstream Rust crates. Use an existing crate directly when it already solves the problem. If an integration is missing, implement only that integration.
2. **No speculative abstraction.** Do not add traits, facades, wrappers, compatibility layers, service managers, or generic backends solely to conceal a dependency or accommodate hypothetical future engines/platforms. Every abstraction needs a concrete use case.
3. **Keep bricks small and composable.** Each component must have a clear responsibility, a minimal API, and independently useful behavior. Do not make applications adopt unrelated components.
4. **Application owns composition.** Applications choose and assemble bricks. Reference compositions are welcome when they demonstrate real use cases or enforce critical safety invariants; they are not mandatory framework infrastructure.
5. **Protect hardware independence of business logic.** Keep MCU-specific HALs and peripherals at the integration boundary. Reuse upstream embedded traits and drivers when appropriate; do not invent interfaces merely for symmetry.
6. **Embassy is the chosen async ecosystem.** Use Embassy when async execution or embedded integration needs it. Do not abstract it away or introduce a competing runtime without an explicit requirement. Pure synchronous code should remain independent of Embassy when possible.
7. **Latest-first.** Target current **stable, mutually compatible** Rust toolchains and upstream crates, especially Embassy, `esp-hal`, and embedded dependencies. Verify compatibility by building and testing; never claim that independent latest releases are compatible without evidence. If a pin is necessary, document the blocker and upgrade path.
8. **Never commit `Cargo.lock`.** It is ignored throughout this library repository. Do not add it by force or relax the ignore rule. Downstream firmware applications may adopt their own independent lockfile policies.
9. **Safe and constrained by design.** Prefer safe Rust and `no_std` where appropriate; make allocations, memory limits, ownership, error handling, and failure behavior explicit. Any `unsafe` needs documented invariants, justification, and targeted review.
10. **No unrelated work.** Change only what the task requires. Do not migrate old iOBEWi code, reorganize directories, add crates, or change public APIs without concrete justification. Existing code is a source to evaluate, not a migration mandate.
11. **Test before claiming success.** Run the most relevant local checks (formatting, tests, linting, target builds), then hardware tests when required by the risk. Never present unrun tests or unavailable hardware qualification as passed.
12. **Every workspace crate owns CI metadata.** Any new member under `common/`, `arch/`, or `examples/` MUST include a `ci.json` alongside `Cargo.toml`, with a supported validation profile as specified by [.github/README.md](.github/README.md). Update the profile when targets or test needs change; never exclude a crate from CI to avoid a failure. The mandatory discovery gate checks all `[workspace].members` and fails on missing or invalid metadata. When adding a new MCU/profile, extend the shared CI executor and its documentation, rather than hardcoding a crate into the workflow.
13. **Report facts, not promises.** Summarize the use case, reused crates, newly added value, files changed, versions actually tested, checks and their results, and any limitations or unverified assumptions.

## Required decision process for a new brick

Before adding code, be able to answer:

- What concrete application problem does this solve?
- Which upstream crate(s) already provide most of the functionality?
- What is the **smallest** missing behavior we must implement?
- Why is this independently reusable rather than application-specific?
- How will its behavior and resource constraints be verified?

If an existing crate already meets the use case, **recommend using it directly instead of creating an iOBEWi component**.

## Universal crate delivery checklist

For any crate—portable library, platform-specific adapter, or reference example—apply the same process:

1. Declare its workspace membership and choose an appropriate `ci.json` profile; consult [.github/README.md](.github/README.md).
2. Keep portable behavior under `common/`, MCU integration under `arch/`, and scenario/platform composition under `examples/` as documented in [ARCHITECTURE.md](ARCHITECTURE.md).
3. Add focused tests and practical examples appropriate to its responsibilities; verify locally before using the CI as a gate.
4. Run the crate's configured checks and applicable global checks (Rustfmt, no tracked `Cargo.lock`), and inspect every failed job rather than disabling a gate.
5. Record what was actually verified, including target, build/test outcome and unverified hardware behavior; a green cross-build does not replace a physical hardware test.

## Definition of done

A change is complete only when its scope is justified, its public API is no larger than needed, its examples/documentation reflect actual behavior, and appropriate local checks have been run and reported. If target hardware or upstream compatibility was not verified, state that limitation clearly; do not silently weaken the requirement.

**Guiding test: if removing the new layer loses no real functionality, do not add that layer.**
