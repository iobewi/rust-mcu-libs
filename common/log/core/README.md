---
layer: portable-contract
status: implemented
invariants:
  - INV-001
gates: []
---

# iobewi-log

## Summary

Local, process-wide log capture with a bounded runtime policy, without a network dependency.

## Responsibilities

Install the global logger once, filter synchronously, invoke the supplied console callback for accepted records, and capture level/target/message in a critical-section protected FIFO. `Off` rejects before console output, formatting or capture. No async loop is required.

## Non-responsibilities

Console hardware, persistence, network activation/authorization and delivery. Capture policy never grants permission to stream logs.

## Architecture

Portable capture layer consumed by future log transport. The target supplies the console callback and application prefix; future log config binds ConfigSpace to this policy without introducing platform dependencies (INV-001).

## Public API

- `install(print, application_target)` once during single-threaded startup. Without an explicit policy, raw prefixes `application_target` and `iobewi_log` accept Info and above, other targets Warn and above, exactly as before. Legacy empty/long application prefixes remain supported for filtering.
- `LogPolicy::new(default_level)` supports Off, Error, Warn, Info, Debug and Trace. `add_target(prefix, level)` adds at most eight unique, nonempty UTF-8 prefixes, each at most 64 bytes. Longest matching raw prefix wins, independently of rule order; otherwise default applies. This preserves historical prefix semantics (including `iobewi_log_stream`). `targets()` exposes immutable rules; malformed/duplicate/overflow additions return `PolicyError` without changing the policy.
- `default_policy(application_target)` constructs the bounded equivalent of the legacy fallback, returning an error for an unrepresentable empty/long prefix. Installation itself does not have this restriction.
- `apply_policy(policy)` atomically replaces runtime filtering without reinstalling the logger. Installation keeps the facade maximum at Info for the legacy fallback, so Debug/Trace macros are rejected before invoking the logger. `LogPolicy::max_level()` computes the maximum of default and all overrides; apply updates this facade ceiling in the same critical section as the policy, serializing concurrent writers. A policy applied before installation is retained with its ceiling. Calls overlapping an update may observe the old or new ceiling. In-flight records that passed `enabled()` before replacement may complete; queued records are retained. Call `discard()` explicitly if required.
- `pop_record()` removes a `CapturedRecord` preserving `level`, `target` and `message`. `pop_line()` remains a compatibility API returning only its message; both consume the same FIFO.
- `LINE_MAX = 160`, `TARGET_MAX = 64`, `RING_CAPACITY = 24`. Oversized messages/targets and incoming records at a full ring are dropped, never truncated or used to evict older entries. Console callback still precedes capture attempts.
- Network delivery metadata is out of scope for this crate.

## Memory budget

Captured storage is inline, with no heap allocation per record or rule. Measured with `size_of` on x86_64: record 248 bytes, deque 5,976 bytes (old text deque 4,056; +1,920), policy 656 bytes. A compile-only `thumbv7em-none-eabi` 32-bit layout probe reports: record 236 bytes, deque 5,676 bytes (old text deque 3,948; +1,728), policy 584 bytes. These exclude mutex/RefCell/Option bookkeeping and are compiler layout measurements, not ESP linker or hardware RAM qualification. The binding additionally needs a 1024-byte inline YAML buffer while encoding; ConfigSpace snapshots use their existing allocated byte vector. Target and message capacities remain independent. Existing LogMetadata consumers may allocate strings.

## Invariants

- `INV-001`

## Validation

`cargo test -p iobewi-log` covers legacy filtering, Off before formatting/console, runtime facade updates and max-level transitions (including override ceilings), prefix precedence, fallback, rule bounds, original metadata, ring overflow, oversized records, boundary-sized records and discard. `cargo check --workspace --all-features` and `--no-default-features` preserve portability. BG-ESP-S3 is the downstream hardware gate when qualifying changed console/WebSocket behavior; host tests do not declare it passed.

## Known limitations

Installation is not repeatable. Runtime filtering cannot restore levels removed by product `release_max_level_*` features. Rules are raw prefixes, not glob/regex expressions. Storage is best effort; full rings retain oldest records. `Off` does not erase records already queued or stop a transport. No dedicated worker, persistence or notification watcher exists in core.

## Related components

`log/config` owns the portable persisted schema/binding; future consumers may read structured records; `platform console` supplies the console callback.
