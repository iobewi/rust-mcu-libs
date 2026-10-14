# iobewi-config-space

Portable `no_std` configuration ownership, quotas and EKV persistence. Components
claim one opaque space and own their schema. The ConfigManager/ConfigSpace API
is unchanged; component serialization and provisioning remain outside storage.

## Persistent store

`storage::Store<F>` accepts the upstream `ekv::flash::Flash` implementation,
a per-boot random allocation seed and the complete static `SpaceSpec` manifest.
One Store owns each region. `backend()` produces handles for ConfigManager.
The writer serializes generation read, EKV transaction and readback; an
acknowledged commit increments generation and verifies the exact stored bytes.
`clear` persists a tombstone so the next generation survives reboot.

The new envelope is CSP2: magic4, generation u64 LE, presence byte, three zero
reserved bytes, then opaque payload. Zero generations, invalid flags/reserved
bytes and payloads on tombstones are rejected. EKV protects integrity with CRC.
There is no NVS or CSM1 compatibility/import path. A failed mount never formats.
`format()` is an explicit destructive operation for composition roots.
`self_check()` reads all live records/CRCs and validates the manifest without
programming a canary on every boot.

## Fixed storage profile and admission

EKV =1.0.0, CRC, ALIGN4, page4096, key64, value8192, chunk256, branching2,
scratch4, erased0xff, at most256 pages. Changing the version/profile requires
storage-format qualification; Store rejects compiled configuration overrides
before I/O. Maximum component payload is 8176 bytes (16-byte envelope).

32–256 physical pages; retain 16 global reserve pages. Each manifest space
reserves `4 * (1 + ceil((16 + name_bytes + payload_budget) / 2048))` pages.
This conservative profile allocates replacement/compaction headroom before any
write. Duplicate/invalid claims and overcommit fail at construction. Only
manifest names are accessible, and budgets cannot exceed their declared maxima.

A 64-page/256-KiB region admits Wi-Fi128, TLS6154, log1024 and OTA2048 budgets
(48 reservation pages +16 reserve). Host tests exercise all maximum values,
repeated hot OTA/TLS commits and clear/recommit across recycling and reboots.
This bounds admission for this profile; hardware resource/timing validation
and additional product workloads remain necessary. ESP examples use 64 pages.
Other MCU adapters must meet the upstream NOR contract and the fixed profile;
EEPROM/FRAM and other erase geometries are not implicitly supported.

## Qualification and known limitation

The host example `qualify_ekv` tests both the direct upstream engine and the
actual ConfigSpace backend. Nominal persistence, tombstone generations,
concurrent writers, quotas, maximum TLS values, recycling, bounds, read-only
self-check and corruption refusal are automatic tests.

**Power-fail recovery remains defective** in EKV 1.0.0/main tested revision,
tracked in [#70](https://github.com/iobewi/rust-mcu-libs/issues/70). The maintainer
chose to continue development. Only the known `power_fail_qualification` test
is ignored automatically. It remains runnable, and CLI reproducers still fail:

```sh
cargo test -p iobewi-config-space
cargo run -p iobewi-config-space --example qualify_ekv -- wear
cargo run -p iobewi-config-space --example qualify_ekv -- cuts
cargo run -p iobewi-config-space --example qualify_ekv -- reproduce
cargo test -p iobewi-config-space --example qualify_ekv power_fail_qualification -- --ignored --nocapture
```

A green CI does not qualify power-fail recovery. See
[measured results and decisions](../../../docs/config/issue-68-audit.md).
