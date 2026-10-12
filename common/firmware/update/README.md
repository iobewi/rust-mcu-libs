# Firmware update

Shared, `no_std`, domain-neutral firmware update engine extracted from `iobewi_old/firmware/update`. Can be composed for A only or independent A+B.

- `artifact`: streaming SHA-256 over **durable** bytes, with bounded pending data and resume decisions. An in-memory hash session cannot be reconstructed after process restart from the watermark alone.
- `transaction`, `state`: staged transactions, durable publish, reconciliation and confirmation/rollback decisions.
- `storage`: explicit back-end boundaries for bulk bytes and **atomic** metadata publication.
- `bootstrap`: separate first-install lifecycle.

There is **no bootloader, ESP32 flash driver, reboot, HTTP, workload supervisor or policy of choosing the A/B domain**. The generic engine deliberately does not claim to ensure flash read-back verification or firmware authenticity. A and B adapters/policies must supply activation and storage contracts.

Historic OTM1 and OTM2 on-flash record codecs belong in the appropriate domain components (in domain-specific adapters), rather than duplicating the shared engine, not this core.
