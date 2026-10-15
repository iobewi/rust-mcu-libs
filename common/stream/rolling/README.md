# Rolling byte stream

## Summary

A bounded, allocation-free rolling byte window with absolute producer progress and
one rebased consumer session. Extracted from USB Radio golden c118e0c.

## Responsibilities

- Retain the latest bytes in caller-selected static capacity.
- Rebase a new session to configurable retained history.
- Bound producer lead relative to the highest successful consumer read.
- Report Ready, Pending or Expired without advancing consumption on unavailable reads.

## Non-responsibilities

- No network, USB, FAT, MP3, synchronization, timers or reconnection.
- No product prebuffer threshold, far-ahead probe policy or session identifiers.

## Architecture

Portable service at `common/stream/rolling`, usable by any byte-stream composition. The
caller wraps mutable access in its own synchronization primitive when shared.

## Public API

`RollingStream<N>::new(retain, max_lead)` requires nonzero capacity and
`retain <= max_lead <= N`. `push` accepts a prefix bounded by `writable`; the caller
must preserve any unaccepted suffix. `written` is monotonic across sessions and
network reconnections. `begin_session` replaces the current session; `end_session`
removes it. `read(offset, out)` uses session-relative byte offsets and zero-fills
unavailable output. `Session` exposes the origin and highest consumed position.

## Lifecycle

Without a session, the producer advances freely in chunks up to the ring capacity.
During a session, consumer reads release producer allowance. Ending a session
resumes unrestricted rolling; the next session starts near the then-current edge.
Product composition decides when to expose a consumer after initial buffering.

## Validation

`cargo check -p iobewi-rolling-stream` and `cargo test -p iobewi-rolling-stream`.
Tests cover wrap, backpressure, backward reads, unavailable data, rebase and progress
beyond the former POC file extent. Hardware acceptance belongs to the consumer.

## Known limitations

One active session; no internal locking or notification. Old data expires. Position
exhaustion at u64::MAX stops production. Reads requesting future data remain Pending;
applications may choose a separate far-ahead probe policy.

## Migration provenance

Ported from `iobewi/iobewi_old` at `345378e`, `stream/rolling`.
See [issue #66 parity and evidence](../../../docs/migrations/66-usb-stream.md).
Historical tests and behavior are retained; upstream Embassy and HAL types remain exposed.

## Resource bounds

Storage is an inline `[u8; N]` plus fixed-size counters/configuration/session state
(`core::mem::size_of::<RollingStream<N>>()` gives the exact target size).
No heap, internal mutex or timer is used.

## Example

```rust
use iobewi_rolling_stream::{ReadStatus, RollingStream};
let mut stream = RollingStream::<1024>::new(512, 768);
assert_eq!(stream.push(&[0x5a; 512]), 512);
stream.begin_session();
let mut sector = [0; 512];
assert_eq!(stream.read(0, &mut sector), ReadStatus::Ready);
assert_eq!(sector, [0x5a; 512]);
stream.end_session();
```
