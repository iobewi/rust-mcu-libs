# State

`iobewi-state` keeps a set of **standard state flags** and tells those who follow it when it
changes. Modules raise and clear the flags they own; consumers (a LED, a screen, a log) read the
result and decide what to do with it. It is `no_std`, allocation-free and knows nothing of any
hardware, of `iobewi-indicator` or of any product.

**It carries states; it never decides which states should be raised.** The application and its
modules choose the flags. What a consumer shows, and in which priority, is the consumer's policy.

```rust,ignore
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use iobewi_state::{Flags, Registry, State};

// Room for 8 sources and 4 observers. It is a `static`: nothing is allocated.
static STATE: Registry<CriticalSectionRawMutex, 8, 4> = Registry::new();

// Producers: each one has its own handle and its own flags.
let wifi = STATE.register()?;
let usb = STATE.register()?;
wifi.raise(State::Connected);
usb.raise(State::Connected);
wifi.clear(State::Connected);        // USB still holds it: the aggregate keeps `Connected`

// A consumer: the current state first, then the latest one after each change.
let mut led = STATE.observer().unwrap();
loop {
    let snapshot = led.changed().await;
    let flags = snapshot.aggregate();            // the union of every source
    // ... map `flags` to a pattern of your own ...
}
```

## The two levels

- **Per source.** Each source holds its own set. Raising `Connected` twice, from two sources, is
  two flags. A source clearing its flag never removes another's.
- **Aggregate.** The union of the sources' sets. `Snapshot::aggregate()`.

A [`Snapshot`] holds both: the revision, the flags of every source and its generation. It is read in
one critical section, so the flags it holds are consistent with one another.

## States

The 35 [`State`]s are flags, **not** a state machine: any number can be raised together. Nothing
orders them or forbids a combination.

| Group | States |
| --- | --- |
| Lifecycle | `Booting`, `Initializing`, `Ready`, `Running`, `Stopping`, `Stopped`, `Restarting`, `ShuttingDown` |
| Operations | `Idle`, `Busy`, `Loading`, `Saving`, `Updating`, `Synchronizing` |
| Connectivity | `Connecting`, `Connected`, `Disconnecting`, `Disconnected`, `Reconnecting` |
| Availability | `Available`, `Unavailable`, `Paused`, `Resuming` |
| Configuration | `Unconfigured`, `Configuring`, `Configured` |
| Health | `Healthy`, `Degraded`, `Warning`, `Error`, `Critical` |
| Maintenance | `Maintenance`, `Recovering`, `Resetting`, `Testing` |

A state is an **observation by a producer**. The negative ones (`Disconnected`, `Unconfigured`,
`Unavailable`, `Stopped`, `Idle`, `Healthy`) are asserted too; they are not the absence of their
positive. The absence of a flag means no source said so. A source that raises `Connected` and
`Disconnected` together contradicts itself: that is the producer's problem, `state` does not
arbitrate, and two *different* sources saying so is legitimate.

Some pairs that are easy to mix up:

| | |
| --- | --- |
| `Ready` / `Available` | `Ready`: the source finished initialising and can do its job. `Available`: its resource can be used by others *right now*. A ready source that is busy or taken is not available. |
| `Healthy` / no `Error` | `Healthy`: a check passed. No `Error`: nobody reported a fault, which a never-checked source does too. |
| `Connected` / `Disconnected` | Each is an observation. Neither is the absence of the other. |
| `Booting` / `Initializing` | `Booting`: the whole product starts. `Initializing`: one module gets ready. |
| `Restarting` / `Resetting` | `Restarting`: stop and start again, settings kept. `Resetting`: return to defaults, settings discarded. |

The discriminants are fixed. Variants are only appended, up to 64 (`Flags` is a `u64`), and the enum
is `non_exhaustive`.

## Sources

`register()` returns a [`Source`], a handle that cannot be cloned. Only the handle raises and clears
the source's flags (`raise`, `clear`, `replace`). Dropping it removes the flags, frees the slot and
tells the observers.

`replace(flags)` sets the whole set in one step: observers never see a state in between (for
instance neither `Connecting` and `Connected` together, nor neither).

A [`SourceId`] is a slot and the **generation** of that slot (`u32`). When a slot is reused its
generation is one more, so an old identifier does not designate the new owner: `Snapshot::of`
returns `None` for it, and `changes_since` reports the old source released and the new one
registered. A generation never wraps: a slot that used up all its generations is **retired** and
never handed out again, and `register()` returns `Full` once no slot is left.

At most 32 sources (`S`) and at least 1 observer (`R`), checked at compile time.

## Notifications

The registry has a **revision**. It grows by one at every *real* change:

- a source registered or released, **including an empty one**;
- a source's set that differs after `raise`, `clear` or `replace`.

An operation that changes nothing (raising a raised flag, clearing an absent one, replacing a set by
itself) changes no revision and wakes nobody. A change that leaves the aggregate unchanged is still
a change: when the Wi-Fi source drops `Connected` while USB holds it, the aggregate keeps
`Connected` and the observers are told anyway, because `Snapshot::changes_since` shows what the
Wi-Fi source did.

An [`Observer`] follows the registry independently of the others:

- `changed().await` returns the **current state at once** the first time (a late observer needs
  nothing else), then the latest state after each change. `get()` and `try_changed()` are the
  non-waiting forms.
- **Intermediate changes may be merged.** An observer that was slow, or not polled, gets one
  snapshot for several changes. It never gets a state the registry did not have, it never misses the
  last one, and revisions it sees only grow. Nothing is kept for it in between: this is not an event
  queue. Compare two snapshots with `changes_since` to learn what happened.
- Observers do not take notifications from one another. At most `R` exist; `observer()` returns
  `None` beyond.

## Locking

One `embassy_sync::blocking_mutex::Mutex<M, _>` protects the table, the revision and the wakers
(`MultiWakerRegistration`). A change (table, revision, wake-up), a snapshot, and an observer's check
of the revision together with the registration of its waker are each **one critical section**, so a
wake-up cannot be lost between reading a snapshot and waiting for the next change. There is one lock
and nothing takes it while holding it: no deadlock. It is never held across an `.await`.

Choose `M` for the contexts that touch the registry:

| `M` | Use |
| --- | --- |
| `CriticalSectionRawMutex` | tasks of several executors, interrupts, both cores of an ESP32-S3 |
| `NoopRawMutex`, `ThreadModeRawMutex` | one executor, nothing else (no interrupt, no other core) |

A critical section holds a few stores and at most `R` wake-ups for a change, and a copy of about
`12 * S + 16` bytes for a snapshot.

Wakers run under the lock, as in `embassy_sync::watch::Watch`: a waker must only schedule a task,
never call back into the registry. If more than `R` distinct wakers wait (an observer polled from
several tasks in turn), the registry wakes them all and starts over: spurious wake-ups, never a
loss.

## Memory

All static, no allocation. Measured by `size_of` in the host tests on a 64-bit host (wakers are
16 bytes there; a 32-bit target has 8-byte wakers and 4-byte `usize`).

| `S` | `Snapshot<S>` | `Registry<_, S, 4>` |
| --- | --- | --- |
| 4 | 64 B | 120 B (R = 2) |
| 8 | 112 B | 200 B |
| 16 | 208 B | 296 B |
| 32 | 400 B | 488 B |

`Flags` is 8 bytes, `SourceId` 8, a `Source` 16, an `Observer` 24 (64-bit host).

On a 32-bit target the same fields add up to about 160 bytes for `Registry<_, 8, 4>` (revision 8,
flags 64, generations 32, three `u32` words, four 8-byte wakers and their length, the `RefCell`
flag). That figure is computed from the layout, not measured on a board.

## Not here

No policy, no priority between states, no history, no event queue, no timer, no persistence, no
state specific to a product. Observers decide how to show what they read.
