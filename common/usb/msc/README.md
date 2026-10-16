# Read-only USB Mass Storage

## Summary

Allocation-free Embassy USB Mass Storage class over a portable 512-byte read-only
block device. Extracted from the functional USB Radio POC at `c118e0c` without
extending its intentionally limited BOT/SCSI implementation.

## Responsibilities

- Register a full-speed bulk-only MSC interface (class 08, subclass 06, protocol 50).
- Handle the implemented SCSI command subset and return CBW-tagged CSWs.
- Obtain medium capacity and sectors through `iobewi-block`.
- Forward USB endpoint session boundaries to the block device.
- Apply caller-supplied unavailable-sector decisions; do not invent retry policy.

## Non-responsibilities

- No FAT layout, file name, stream buffering, URL, MP3 processing or radio policy.
- No platform HAL, GPIO selection, physical flash ownership or USB driver wrapper.
- No VID/PID, USB product strings, power declaration or prebuffer-before-exposure policy.
- No complete BOT/SCSI conformance claim, media writes or Workload binary ABI.

## Architecture

Portable service at `common/usb/msc`. Embassy USB's `Driver` supplies hardware
operations; `common/fs/block` supplies medium data. Identity and unavailable-data policy
are supplied by the composition root. Source/API portability does not imply support
for every USB speed or host. Hardware qualification remains required.

## Public API

`State::new` provides caller-owned class control state. `MscClass::new` receives
an Embassy `Builder`, that state, and `InquiryIdentity` (8-byte vendor, 16-byte
product, 4-byte revision). No identity is baked into the service.

`MscClass::run(device, policy)` consumes `ReadOnlyBlockDevice` and `ReadPolicy`.
`ReadPolicy::unavailable(status, elapsed)` is called only for `Pending` or
`Expired`; it returns `RetryAfter(Duration)` or `ZeroFill`. Elapsed time starts
before the first read of each sector. `ZeroFill` sends the block contract's
zero-filled sector and preserves successful-command semantics. No default retry,
poll interval, timeout or stale-data policy is supplied.
`ReadPolicy::command_started()` (default: no-op) is called once at the start of every
READ(10) that passes the range check, before its first sector. A policy that must bound the wait
of a whole command, because the host aborts a command after its own timeout whatever the number
of sectors it asked for, records the instant there and applies its budget to the command instead
of to each sector. `elapsed` keeps its per-sector meaning for policies that do not use it.

`run` returns `Error::UnsupportedCapacity` before starting any session if the
inclusive last LBA is `u32::MAX`, since READ FORMAT CAPACITIES cannot represent
that medium's sector count. Other terminal failures return `Error::Endpoint`.
The capacity must remain stable for the device lifetime as required by `common/fs/block`.

## Lifecycle

The caller runs the Embassy USB device future alongside `run`. After the OUT
endpoint becomes enabled, the class resets diagnostic counters and calls
`begin_session`. Endpoint disable calls `end_session` and waits for a subsequent
enable, allowing a streaming block device to rebase. Other endpoint errors also
call `end_session` before returning. A future cancelled externally does not run
an asynchronous cleanup or guarantee an `end_session` call.

During READ(10), sector reads are non-blocking. The class retries only as directed
by the policy, then sends sector bytes in 64-byte packets. If a policy chooses
unbounded retries, bus disable is observed only when endpoint I/O is next reached;
the class does not impose an independent timeout or cancellation check.

## Validation

Run `cargo fmt -p iobewi-usb-msc --check`, `cargo check -p iobewi-usb-msc` and
`cargo test -p iobewi-usb-msc`. Host tests check golden packet fields and the
supported capacity boundary; fake-driver consumer dialogues exercise actual endpoint
traffic and session callbacks. Supported-target compilation and physical USB-host evidence are required for
complete qualification.

## Bus reset during a command

The class handler counts USB bus resets (`MscClass::bus_reset_counter`). A READ(10) that is
waiting for data when the host resets the bus is abandoned at once: the host has abandoned that
command and re-enumerated, the endpoints are enabled again in the new configuration, and pushing the
old command's data and status there would desynchronise the new session and keep the class from
reading the host's next command. The session ends as for an endpoint disable and restarts.
Commands waiting on an endpoint are not affected: the endpoint returns `Disabled` by itself.
The reference count is taken **per command**, as the command starts: a reset that happened before it (the
host then sent the command on the new bus) does not abandon it.

## Known limitations

- Fixed 512-byte sectors, two bulk endpoints with 64-byte maximum packets, one
  advertised LUN and read-only direct-access removable media.
- Implemented commands: TEST UNIT READY, INQUIRY, REQUEST SENSE, MODE SENSE(6),
  START STOP UNIT, PREVENT ALLOW MEDIUM REMOVAL, READ FORMAT CAPACITIES,
  READ CAPACITY(10), READ(10). START STOP and PREVENT ALLOW are no-ops.
- CBW direction/flags are ignored. LUN 0..15 and command lengths 1..16 are accepted
  by the parser although only LUN zero is advertised and CDB lengths are not
  validated per opcode. Unsupported opcodes report illegal request/invalid opcode.
- Malformed CBWs and non-31-byte packets are ignored without endpoint stall or
  BOT recovery. BULK-ONLY RESET is acknowledged without worker/command-state reset.
- No BOT phase-error matrix, write commands, READ(12), allocation-length handling
  in SCSI CDBs, or terminating data-stage ZLP logic.
- READ(10) sends the requested sector count without bounding it by CBW transfer
  length. CSW residue uses saturating subtraction, preserving the reference.
- Out-of-range READ(10) fails with illegal request/LBA out of range. REQUEST SENSE
  consumes the stored error once. Failed reads do not synthesize an extra data stage.
- Unavailable sectors may be zero-filled with success by caller policy; this is
  not a promise of immutable storage or recovered audio.
- `log` is used for diagnostics. `usb-debug` adds control/command diagnostics and
  Embassy USB logging. The application supplies its logger and filter.

## Migration provenance

Ported from `iobewi/iobewi_old` at `345378e`, `drivers/usb/msc`.
See [issue #66 parity and evidence](../../../docs/migrations/66-usb-stream.md).
Historical tests and behavior are retained; upstream Embassy and HAL types remain exposed.

## Resource bounds

The worker contains a 64-byte packet and a 512-byte sector across awaits, plus
command/session state; exact future size depends on target and monomorphization.
Descriptor, control-state, driver buffers and executor storage are supplied by the
application. No heap is used by this class.

## Composition example

The caller supplies the hardware driver, medium, identity and unavailable-data
policy. Builder buffers and class state must live for the driver lifetime;
executor storage and descriptor sizing remain the application's responsibility.

```rust,no_run
use embassy_usb::{Builder, driver::Driver};
use iobewi_block::ReadOnlyBlockDevice;
use iobewi_usb_msc::{InquiryIdentity, MscClass, ReadPolicy, State};
async fn serve<'d, D: Driver<'d>, B: ReadOnlyBlockDevice, P: ReadPolicy>(
    mut builder: Builder<'d, D>, state: &'d mut State<'d>,
    disk: &mut B, policy: &mut P, identity: InquiryIdentity,
) {
    let mut class = MscClass::new(&mut builder, state, identity);
    let mut usb = builder.build();
    embassy_futures::join::join(usb.run(), class.run(disk, policy)).await;
}
```

`tests/session.rs::rolling_fat_msc_bytes_underrun_recovery_and_new_session`
is an executable host composition of all three bricks with a fake Embassy driver.
It verifies data packets and session rebasing; it does not exercise a PHY.
