# iobewi-config-space

## Summary

Portable `no_std`, hardware-agnostic configuration ownership, quota and opaque replacement-commit layer.

## Responsibilities

- Own unique configuration-space ownership, reservation/admission control, per-space payload limits, generations and capability-handle isolation.
- Define the generic `ConfigBackend` persistence contract while leaving schemas opaque.

## Non-responsibilities

- Does not know Wi-Fi SSIDs, certificates, tokens, GPIOs, flash sectors or NVS namespaces.
- Does not own component serialization schemas/migrations or provisioning policy.
- Does not own backend-specific capacity accounting and physical atomicity.

## Integration

Portable configuration component. Components claim an opaque space with a budget; platform backends translate logical reservations into storage-specific capacity and atomic replacement guarantees.

## Public API

Callers claim a space, then load/commit an opaque value through the returned `ConfigSpace` capability. A successful claim is a boot-lifetime reservation guarantee.

## Known limitations

A logical payload byte is not assumed to equal one physical storage byte; backend reservation accounting determines whether a claim can be guaranteed.


## Persistence direction — issue #68

EKV is the selected foundation for the next portable persistence backend.
The project is in development: no NVS migration or CSM1 compatibility is part
of the target. The previously published `record` module remains in the current
NVS implementation until its replacement passes qualification.

`examples/qualify_ekv.rs` exercises EKV directly on persistent simulated NOR
flash: large cold TLS values, hot OTA updates, per-sector wear, read-only boots
and partial write/erase failures. EKV is currently a dev-dependency only.

```sh
cargo run -p iobewi-config-space --example qualify_ekv -- wear
cargo run -p iobewi-config-space --example qualify_ekv -- cuts
cargo run -p iobewi-config-space --example qualify_ekv -- reproduce
```

**Power-fail qualification currently fails with EKV 1.0.0**: a torn metadata
chunk prevents remounting and makes the previous acknowledged value inaccessible.
The example is also a test target; `cargo test` enforces this failing gate.
The ESP integration is blocked until recovery is qualified. See
[the measured results and reproducer](../../../docs/config/issue-68-audit.md).
