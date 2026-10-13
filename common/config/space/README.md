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


## Existing persistent records

`record` owns the storage-independent legacy CSM1 envelope (generation, presence
flag, opaque payload). Its byte format is unchanged; legacy NVS callers keep
working through re-exports from `iobewi-nvs-core`. This codec does not provide
checksums or power-cut atomicity. Those remain the responsibility of the storage
engine. See `docs/config/issue-68-audit.md` for the migration and qualification plan.
