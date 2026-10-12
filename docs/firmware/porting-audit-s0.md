# S0 — Firmware/OTA porting inventory and parity gates

## Frozen baselines (2026-10-12)
- Target: [rust-mcu-libs `43f068951f9f92e070c9d57336b40f4037d5e861`](https://github.com/iobewi/rust-mcu-libs/tree/43f068951f9f92e070c9d57336b40f4037d5e861)
- Source: [iobewi_old `345378ed1ae11a4814c32ee513f20438291cfba7`](https://github.com/iobewi/iobewi_old/tree/345378ed1ae11a4814c32ee513f20438291cfba7)

**Scope:** documentation-only inventory. This does not assert functional parity or hardware qualification.

## Target architecture

```text
common/firmware/{image,slots,boot,update}
arch/esp32/flash/{partitions}
arch/esp32/firmware/{storage,boot,supervisor(optional)}
```

- One portable update engine; no firmware-domain-specific update engines.
- One ESP physical flash owner and one partition discovery implementation.
- Domain A and B remain independent logical firmware domains (A0/A1 and B0/B1). A-only remains supported.
- Bootloader-specific `otadata` is not the common artifact store; B does not inherit `otadata`.
- Legacy on-flash bytes (including the v1 magic) must not change during structural migration. Use descriptive *transactional A/B boot metadata* nomenclature rather than the old acronym.
- Port existing code and its tests first; redesign only where a documented incompatibility requires it.

## Source-to-target traceability

Status terminology: **present** = source-equivalent file or component exists, NOT proven behavioral parity; **partial** = some mechanism exists but integration/contracts are missing; **missing** = not located in target tree. Every partial/missing row requires source/API/test parity review in subsequent stages.

| Historical source in iobewi_old | Current target location | Status | S-stage / parity gate |
| --- | --- | --- | --- |
| `firmware/image/src/{lib,esp,digest}.rs` and `esp_tests.rs` | `common/firmware/image/` | Present | S0 compare semantics and historical tests |
| `firmware/slots/src/lib.rs`, `firmware/model/` | `common/firmware/slots/` | Partial | S1 compare domain IDs, slot roles, and model API |
| `firmware/boot/src/{lib,tests}.rs` | `common/firmware/boot/` | Present | S3 golden metadata bytes, power-cut transitions |
| `firmware/update/src/{artifact,bootstrap,state,storage,transaction,error}.rs` | `common/firmware/update/src/` | Present | S1 verify API/test equivalence |
| `firmware/update/src/metadata.rs` | `common/firmware/update/src/compat/otm1.rs` | Partial | S1 old record bytes and atomicity |
| `firmware/update/src/config_space.rs` | Not present in portable update crate | Missing | S1 preserve storage integration and lock discipline |
| `firmware/update/src/service.rs` and `service/{upload,boot}.rs` | Not present in portable update crate | Missing | S1 restore portable service contracts, watchdog/self-check |
| `firmware/esp32/src/lib.rs` | `arch/esp32/firmware/a/` plus `arch/esp32/flash/partitions` | Partial | S2 port full partition mapping, image writer and erase behavior into shared storage |
| `firmware/esp32/src/shared_flash.rs` | `arch/esp32/firmware/a/{flash,otadata_esp}.rs` | Partial | S2/S3 compare buffered streaming writer, readback digest, activate/confirm/reject and one-lock ownership |
| `firmware/esp32/src/otadata.rs` | `arch/esp32/firmware/a/{otadata,selection,running}.rs` | Partial | S3 compare read/write target, state transitions, confirmation, failures |
| `firmware/esp32/src/service.rs` | Not present | Missing | S3 restore TargetSelection/BootActivation/BootState integration |
| `bootloader/esp/src/main.rs` and `build.rs` | Not present | Missing | S3 preserve custom boot flow; compile and image-boot testing |
| `workload/update/src/otm2.rs`, `store.rs` | `common/firmware/update/src/compat/{otm2,otm2_store}.rs` | Partial | S4 binary fixture equivalence and interrupted-write tests |
| `workload/update/src/{layout,nor,flash,probe}.rs` | `arch/esp32/flash/` (generic access only) | Partial | S2/S4 domain B placement and bounded NOR writer |
| `workload/update/src/{machine,service}.rs` | Not present as full integrated service | Missing | S4 state/prepare/upload/reconcile/activation parity |
| `workload/update/src/supervisor.rs` | Not present | Missing | S4 optional supervisor/runtime API/rollback |
| `workload/update/src/{tests,tests_nor,tests_service,tests_supervisor,testing}.rs` | Only some codec tests ported | Partial | S4 restore behavioral, fault-injection and service coverage |
| `net/http/ota/` | Not included in this firmware-core comparison | Not assessed | Later transport boundary check; do not conflate HTTP with engine |

## Current implementation requiring correction

`arch/esp32/firmware/a` was incrementally built in PRs #46–#52. It contains preflight partition layout, raw bounded flash operations, metadata executor, locking adapter, and selection-report boundary, but **does not demonstrate equivalence** with historical `firmware/esp32`, nor implement the actual bootloader. It must be split by responsibility into `storage/` and `boot/`, retaining only proven logic, after tests have been carried over.

The current `docs/firmware/dual-ota.md` still shows proposed `update/core`, `update/a` and `update/b` paths that do not match the single-engine workspace. Replace after architecture sign-off; do not create those crates.

## Required gates

1. Before porting any source: record exact historical file, API, behavioral invariants, and original tests.
2. Preserve binary records OTM1/OTM2 and transactional boot metadata v1 byte-for-byte; add golden fixtures.
3. Preserve the single `SharedFlash` physical mutex; never hold a flash lock across a ConfigSpace operation that might acquire it.
4. Run formatting, host tests and Clippy **locally when toolchain available**, then CI; C3/S3 builds for hardware adapters.
5. Test repeated interrupted writes, reset during activation, confirm, reject, and fallback; distinguish host simulations from on-card qualification.
6. Keep A-only independent of B. B composition must reuse the common engine and ESP storage while retaining its distinct runtime activation.
7. Do not delete `firmware/a` before replacement adapters and regression tests are ready.
8. Close S0 only after each historical source has a destination or justified exclusion and a concrete parity gate.

## Recommended execution

- **S0** (this file): freeze baselines and traceability; no runtime modifications.
- **S1**: compare and restore portable service/contracts, with tests.
- **S2**: move/port legacy ESP storage and NOR writer without a parallel flash stack.
- **S3**: move/port ESP boot integration and custom bootloader.
- **S4**: port B persistence, state machine, service and optional supervisor.
- **S5**: A-only and A+B examples, real hardware interruption qualification.

**S0 state:** inventory prepared, but detailed function-by-function and test-by-test equivalence remains to be established before claiming S0 PASS.
