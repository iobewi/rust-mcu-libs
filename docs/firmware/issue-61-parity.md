# Issue 61 — parity and shared write reservations

Reference: `iobewi_old@345378ed1ae11a4814c32ee513f20438291cfba7`.
This is the software integration increment on PR #65, not a declaration of full
A/B or hardware qualification. Decisions are recorded in issues #50, #54, #61.

## Old → new parity matrix

| Historical contract | Current implementation and evidence | Remaining boundary |
| --- | --- | --- |
| Boot v1 records, CRC, commit word, selection, activate/confirm/reject | Unchanged `common/firmware/boot`; old/current comparison: 30 byte vectors, 1024 metadata pairs, 4096 image-validity scenarios identical | Real bootloader/image handover |
| Transactional otadata executor with readback after erase/body/commit | `firmware/a/otadata`; restored erased-state readback and final decode verification; host activation/confirmation/rollback and failed-erase tests | Power interruption on real flash |
| Actual booted partition observation | `flash/partitions::booted_partition` uses the same upstream API as old `otadata::booted_slot`; `SystemBoot` reads it plus metadata under SharedFlash | C3/S3 mapped-image observation on board |
| Inactive target from historical `update_target` | Historical selector retained; `running::reserve_inactive` compares it with the observed partition | Product service integration |
| No upload during activating/probation state | Standalone system reservation additionally refuses any PendingVerify entry, protecting the confirmed fallback; does not change the selector/record codec | Service composition and hardware probation |
| Shared flash streaming writer, 64 KiB erase ahead, sector durability | Sole ESP `ArtifactWriteSession` owns the engine and writer state; mutable permit borrow; no B writer copy | Flash upload/hash failure behavior on board |
| Release flash before ConfigSpace | Writer returns after dropping the physical mutex; portable historical service tests retained (6 upload and 9 boot test names) | Actual product wiring |
| OTM1 persistence | Historical/current codecs compared over 24 vectors; frozen byte fixtures committed and exercised by ordinary host CI | End-to-end ConfigSpace persistence on board |
| OTM2 binary records | Historical/current codecs compared over 24 vectors across all states/sequence boundaries; frozen byte fixtures committed | Actual metadata backend composition |
| B layout: wl_meta, equal slots, min 256 KiB, independent copies | Historical constraints retained; explicit address/metadata-size overflow checks; validated capacity when constructing SlotLayout | Real partition table B qualification |
| B stopped/first-install and actual execution | Portable `DomainState` distinguishes Unknown/Stopped/Running; stopped permits installation; unknown and active target refuse | Runtime must supply real execution observations |
| B activation/reconcile/rollback | Historical persistence policy ported to `common/firmware/update::supervised`, reusing the same WriteSession; 6 host tests cover first install, supersession, activation/rollback phases, compatibility, corrupt metadata and 180 torn-copy prefixes | Concrete metadata backend, digest readback, runtime start/stop/health and product wiring remain pending |
| Domain extension | Application-assigned Domain ID, generic checked SlotLayout and reservation; A/B/C tested on a 16 MiB map | No concrete C product policy claimed |

## Intentional changes

- A's duplicated layout validator is replaced by a re-export of the tested
  implementation. `discover` uses the common validation algorithm and enforces
  64 KiB application-start alignment. Ambiguous same-label entries are rejected
  even if their subtypes differ.
- A/B separate authorization functions and B's separate borrow state are replaced
  by one checked layout and one portable domain reservation. There is no public
  stand-alone permit factory.
- Writers exclusively borrow the reservation permit. Raw streaming writer/engine
  coupling is internal; the public session keeps its matching engine and erase
  watermark together and checks size/geometry before any erase.
- The existing `firmware/a` path remains; relocating/removing its compatibility
  surface is deferred until replacement/product integration is qualified.
- Domain is an application-assigned u8 identity (A=0, B=1). It has no wire codec;
  no persisted OTM1/OTM2/boot byte is migrated.
- The historical supervised persistence policy is reused in `update::supervised`:
  it is not a second streaming engine. Product-specific UpdateRequest/target
  coupling is replaced by typed SlotMeta, synchronous loader-effect wrappers are
  omitted, and Prepared is opaque with an active-slot consistency check. No codec
  changes. Execution occurs between persisted begin/complete phases.
- The portable reservation does not impose A's boot policy on supervised domains.
  A's actual observer and PendingVerify restriction are in the system adapter.
- Full partition geometry is checked, including unrelated/future domains and
  configuration, rather than only required labels.
- A `host-esp32` CI profile executes actual A adapter tests on host as well as C3/S3
  compilation. Boot CI also runs release fault-injection tests that debug skips.

## Verification run locally

- Stable Rust 1.99.0: firmware host tests, legacy binary fixtures and strict Clippy
  with update `runtime config-space`; compile-fail reservation doctests.
- Boot release: **37 passed, 0 ignored**, including the 8 exhaustive fault tests.
- C3: nightly 1.101.0 (2026-10-13), riscv32imc-unknown-none-elf, core/alloc checks.
- S3: esp toolchain 1.98.1.0, xtensa-esp32s3-none-elf, core/alloc checks.
- Workspace CI discovery validates every one of the 44 crates.
- `cargo fmt --all -- --check`, no tracked Cargo.lock.

Independent comparison command (no engine code copied into the repository):

```sh
python3 docs/firmware/check_parity.py /path/to/frozen/iobewi_old
```

Output SHA256 (encoded records plus canonical decision/decode output):

- boot: `1e8fb7cc3625f7d4103c9d5ebd45b4d6855408b22f031a3704e67eafb6381e11`
- OTM1: `a44b74a2bb0df44aac27d60796c7bb29cb5072c1233be9803f36a2dd0cc16f21`
- OTM2: `5d035904b47b8361e3638f74b28f8c0fccc34ab947bd85c2dcd99c40153789b5`

## Acceptance limits

No device is attached in this environment. Bootloader port/composition is now
build/link/size checked; product OTA endpoints, actual supervised activation/recovery and physical interruption
qualification are **not completed** by this increment. No rescue image or
configuration reset is implemented. Keep #61 open and #65 Draft until the remaining
software compositions and separate hardware gates are satisfied.


## Second-stage port (2026-10-13)

The real executable now composes current bricks in examples/firmware-bootloader.
ROM bindings, common MemoryMap constants, original linkers and tested discovery
are in arch/esp32/firmware/boot. C3/S3 ROM duplication is removed while RTC offsets
are preserved. Boot/application share one actual metadata executor with separate
application fallback protection. Raw entry iteration is shared with runtime;
upstream driver, typed accessors and MMU observation remain reused.

Intentional changes: missing/duplicate/overlapping/unaligned/out-of-capacity table
layouts halt before writes instead of historical last-match-wins. Label matching
follows the current library A contract. Valid-layout slot/record encodings and
Verify::Structure, seed/rollback and RAM/MMU handover are retained. The executable
is not byte-identical to the frozen original.

Local links: esp-hal 1.2.2, esp-println 0.18.0, C3 nightly 1.101.0 and S3 esp
1.98.1.0 + GCC 16.1.0. ESP images with 16MB headers: C3 22800 bytes / 9968 margin;
S3 29744 bytes / 3024 margin, valid checksum/footer. A 16MB header is not physical
16MB evidence. Stack linker guards are retained; actual use needs measurement.
The new CI profile gates real links and packaged size. Full verification,
product watchdog/OTA/supervised wiring and hardware faults remain separate gates.
