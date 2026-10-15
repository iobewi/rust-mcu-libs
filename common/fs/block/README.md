# Read-only block device

## Summary

Portable, allocation-free contract for non-blocking reads of 512-byte sectors with
explicit consumer session callbacks.

## Responsibilities

- Define the sector size, inclusive capacity and sector read status.
- Define callbacks for the start and end of a consumer session.
- Require zero output when data is unavailable.

## Non-responsibilities

- No writes, physical flash ownership, filesystem format, USB or SCSI.
- No internal synchronization, scheduling, retries, timeout or stale-data policy.
- No platform HAL or product identity.

## Architecture

Portable contract at `common/fs/block`. Filesystem services implement it; protocol adapters
consume it. This Rust API is an in-process contract, not a Workload binary ABI.

## Public API

`SECTOR_SIZE` is 512 bytes. `ReadOnlyBlockDevice::last_lba` returns the inclusive last
address of a nonempty medium, whose capacity remains stable for its lifetime.
`read_sector` must not wait: `Ready` means the complete sector is available, `Pending`
allows retry, and `Expired` means unavailable retained data or an out-of-range address.
The complete output must be zero for `Pending` and `Expired`.
`begin_session` and `end_session` default to no action.

## Lifecycle

The consumer controls session boundaries and can replace a session by starting another
if the implementation supports it. A source may rebase stream content on a new session.
Read-only prohibits writes through the API; it does not imply immutable bytes across
reads or sessions. Consumers own retry, timeout and fallback decisions.

## Validation

`cargo check -p iobewi-block` and `cargo test -p iobewi-block`.
The `iobewi-fat16` host tests exercise capacity, session callbacks, sector mapping and
unavailable output through this trait. Hardware acceptance belongs to consumers; these host checks do not qualify USB hardware.

## Known limitations

Fixed 512-byte sectors, nonempty media and u32 LBA addresses. One call reads one sector.
No distinct error class for expired content versus out-of-range addresses. This API
provides no concurrency or persisted state guarantees.

## Migration provenance

Ported from `iobewi/iobewi_old` at `345378e`, `fs/block`.
See [issue #66 parity and evidence](../../../docs/migrations/66-usb-stream.md).
Historical tests and behavior are retained; upstream Embassy and HAL types remain exposed.

## Resource bounds

The contract itself allocates no memory; the caller supplies one 512-byte sector.
