# Firmware update

Shared, `no_std`, domain-neutral firmware update engine extracted from `iobewi_old/firmware/update`. Can be composed for A only or independent A+B.

- `artifact`: streaming SHA-256 over **durable** bytes, with bounded pending data and resume decisions. An in-memory hash session cannot be reconstructed after process restart from the watermark alone.
- `transaction`, `state`: staged transactions, durable publish, reconciliation and confirmation/rollback decisions.
- `storage`: explicit back-end boundaries for bulk bytes and **atomic** metadata publication.
- `bootstrap`: separate first-install lifecycle.

There is **no bootloader, ESP32 flash driver, reboot, HTTP, workload supervisor or policy of choosing the A/B domain**. The generic engine deliberately does not claim to ensure flash read-back verification or firmware authenticity. A and B adapters/policies must supply activation and storage contracts.

Historic OTM1 and OTM2 on-flash record codecs belong in the appropriate domain components (in domain-specific adapters), rather than duplicating the shared engine, not this core.

## Supervised-domain persistence policy

`supervised::SupervisedUpdate` ports the historical OTM2 activation/recovery
phases. It uses `compat::otm2_store::MetadataBackend` and the same `WriteSession`
as every other domain; it is **not another upload engine**. Typed SlotMeta
replaces the old product-specific update order. B/C may choose this policy or
compose a different recovery policy over the common engine.

Order: prepare -> reserve the observed inactive target -> begin_overwrite before
writing -> shared session verification -> commit_staged -> release reservation.
Activation checks compatibility, persists Activating, then the application
verifies stored bytes and performs runtime stop/start before complete_activation.
Confirm only after matching running identity and health checks. Recovery returns
explicit restore/idle/corrupt outcomes; complete_rollback follows actual restoration.
No automatic format/reset and no bootloader/otadata dependency for these domains.

Host tests cover first install, probation/activation interruptions, staged
supersession, incompatible runtime, corrupt records and all 180 torn-copy byte
prefixes. Real metadata-flash and runtime composition remain to be qualified.
