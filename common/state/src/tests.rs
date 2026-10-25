use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Wake, Waker};
use std::thread;
use std::time::{Duration, Instant};

use embassy_sync::blocking_mutex::raw::{CriticalSectionRawMutex, NoopRawMutex};

use super::*;

type Reg<const S: usize = 8, const R: usize = 4> = Registry<NoopRawMutex, S, R>;

/// A waker that counts how many times it is woken.
#[derive(Default)]
struct Counter(AtomicUsize);

impl Wake for Counter {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

impl Counter {
    fn waker() -> (Arc<Counter>, Waker) {
        let counter = Arc::new(Counter::default());
        (counter.clone(), Waker::from(counter))
    }

    fn count(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}

/// Polls `fut` once with `waker`.
fn poll_once<F: Future>(fut: &mut std::pin::Pin<&mut F>, waker: &Waker) -> Poll<F::Output> {
    fut.as_mut().poll(&mut Context::from_waker(waker))
}

struct Unpark(thread::Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// Runs `fut` on the current thread, parked between polls, and fails after ten seconds: a lost
/// wake-up shows as a test failure, not as a hang.
fn block_on<F: Future>(fut: F) -> F::Output {
    let mut fut = pin!(fut);
    let waker = Waker::from(Arc::new(Unpark(thread::current())));
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Poll::Ready(value) = fut.as_mut().poll(&mut Context::from_waker(&waker)) {
            return value;
        }
        let left = deadline
            .checked_duration_since(Instant::now())
            .expect("woken too late: a wake-up was lost");
        thread::park_timeout(left);
    }
}

// --- State and Flags -------------------------------------------------------------------------

#[test]
fn the_catalogue_is_stable() {
    assert_eq!(State::COUNT, 35);
    for (i, state) in State::ALL.into_iter().enumerate() {
        assert_eq!(state as usize, i, "{state:?}");
        assert_eq!(State::from_index(i as u8), Some(state));
    }
    assert_eq!(State::from_index(35), None);
    // Values fixed by the contract: changing one is a breaking change.
    assert_eq!(State::Booting as u8, 0);
    assert_eq!(State::Connected as u8, 15);
    assert_eq!(State::Disconnected as u8, 17);
    assert_eq!(State::Error as u8, 29);
    assert_eq!(State::Testing as u8, 34);
}

#[test]
fn flags_are_a_set() {
    let f = Flags::of(&[State::Connected, State::Ready]);
    assert!(f.contains(State::Connected) && f.contains(State::Ready));
    assert!(!f.contains(State::Error));
    assert_eq!(f.len(), 2);
    assert!(!f.is_empty() && Flags::EMPTY.is_empty());

    assert_eq!(f.with(State::Ready), f);
    assert_eq!(f.without(State::Error), f);
    assert_eq!(f.without(State::Ready), Flags::from(State::Connected));
    assert_eq!(f.union(Flags::from(State::Busy)).len(), 3);
    assert_eq!(
        f.difference(Flags::from(State::Ready)),
        State::Connected.into()
    );
    assert_eq!(
        f.iter().collect::<Vec<_>>(),
        [State::Ready, State::Connected]
    );
    assert_eq!(f.bits(), 1 << 2 | 1 << 15);

    let all = State::ALL.into_iter().fold(Flags::EMPTY, Flags::with);
    assert_eq!(all.len(), 35);
    assert_eq!(format!("{f:?}"), "{Ready, Connected}");
}

// --- Sources ---------------------------------------------------------------------------------

#[test]
fn raise_and_clear() {
    let reg = Reg::<4, 2>::new();
    let src = reg.register().unwrap();
    assert!(!src.is_set(State::Ready));
    assert!(src.raise(State::Ready));
    assert!(src.is_set(State::Ready));
    assert_eq!(src.flags(), Flags::from(State::Ready));
    assert_eq!(reg.snapshot().aggregate(), Flags::from(State::Ready));
    assert!(src.clear(State::Ready));
    assert!(!src.is_set(State::Ready));
    assert!(reg.snapshot().aggregate().is_empty());
}

#[test]
fn a_repeated_operation_changes_nothing() {
    let reg = Reg::<4, 2>::new();
    let src = reg.register().unwrap();
    let mut obs = reg.observer().unwrap();
    assert!(src.raise(State::Busy));
    let rev = reg.snapshot().revision();
    obs.get();

    let (counter, waker) = Counter::waker();
    let mut fut = pin!(obs.changed());
    assert!(poll_once(&mut fut, &waker).is_pending());

    assert!(!src.raise(State::Busy), "already raised");
    assert!(!src.clear(State::Error), "never raised");
    assert!(!src.replace(Flags::from(State::Busy)), "same set");
    assert_eq!(reg.snapshot().revision(), rev);
    assert_eq!(counter.count(), 0, "no change, no wake-up");
    assert!(poll_once(&mut fut, &waker).is_pending());

    assert!(src.clear(State::Busy));
    assert_eq!(reg.snapshot().revision(), rev + 1);
    assert_eq!(counter.count(), 1);
}

#[test]
fn several_flags_at_once() {
    let reg = Reg::<4, 2>::new();
    let src = reg.register().unwrap();
    for s in [State::Ready, State::Running, State::Healthy, State::Busy] {
        src.raise(s);
    }
    assert_eq!(src.flags().len(), 4);
    src.clear(State::Busy);
    assert_eq!(
        src.flags(),
        Flags::of(&[State::Ready, State::Running, State::Healthy])
    );
}

#[test]
fn the_aggregate_is_the_union_and_each_source_keeps_its_own() {
    let reg = Reg::<4, 2>::new();
    let a = reg.register().unwrap();
    let b = reg.register().unwrap();
    let c = reg.register().unwrap();
    a.replace(Flags::of(&[State::Connected, State::Ready]));
    b.replace(Flags::of(&[State::Connected, State::Busy]));
    c.replace(Flags::of(&[State::Warning]));

    let snap = reg.snapshot();
    assert_eq!(
        snap.aggregate(),
        Flags::of(&[State::Connected, State::Ready, State::Busy, State::Warning])
    );
    assert_eq!(snap.of(a.id()), Some(a.flags()));
    assert_eq!(snap.of(b.id()), Some(b.flags()));
    assert_eq!(snap.of(c.id()), Some(Flags::from(State::Warning)));
    assert_eq!(snap.sources().count(), 3);

    a.clear(State::Connected);
    let snap = reg.snapshot();
    assert!(
        snap.aggregate().contains(State::Connected),
        "b still holds it"
    );
    assert!(!snap.of(a.id()).unwrap().contains(State::Connected));
}

#[test]
fn a_negative_state_is_not_the_absence_of_the_positive_one() {
    let reg = Reg::<4, 2>::new();
    let wifi = reg.register().unwrap();
    let usb = reg.register().unwrap();
    assert!(!reg.snapshot().aggregate().contains(State::Disconnected));
    wifi.raise(State::Connected);
    usb.raise(State::Disconnected);
    // Contradictions between sources are not arbitrated.
    assert_eq!(
        reg.snapshot().aggregate(),
        Flags::of(&[State::Connected, State::Disconnected])
    );
}

#[test]
fn replace_is_one_atomic_step() {
    let reg = Reg::<4, 2>::new();
    let src = reg.register().unwrap();
    let mut obs = reg.observer().unwrap();
    src.raise(State::Connecting);
    obs.get();
    let rev = reg.snapshot().revision();

    assert!(src.replace(Flags::from(State::Connected)));
    let snap = obs.try_changed().unwrap();
    assert_eq!(snap.revision(), rev + 1, "one revision for the whole step");
    assert_eq!(snap.of(src.id()), Some(Flags::from(State::Connected)));
    assert!(
        obs.try_changed().is_none(),
        "no intermediate state was published"
    );
}

// --- Registration and release ----------------------------------------------------------------

#[test]
fn registering_an_empty_source_is_a_change() {
    let reg = Reg::<4, 2>::new();
    let mut obs = reg.observer().unwrap();
    obs.get();
    let (counter, waker) = Counter::waker();
    let mut fut = pin!(obs.changed());
    assert!(poll_once(&mut fut, &waker).is_pending());

    let src = reg.register().unwrap();
    assert_eq!(counter.count(), 1);
    let Poll::Ready(snap) = poll_once(&mut fut, &waker) else {
        panic!("not woken for nothing")
    };
    assert_eq!(snap.revision(), 1);
    assert_eq!(snap.of(src.id()), Some(Flags::EMPTY));
}

#[test]
fn releasing_an_empty_source_is_a_change() {
    let reg = Reg::<4, 2>::new();
    let src = reg.register().unwrap();
    let id = src.id();
    let mut obs = reg.observer().unwrap();
    obs.get();
    let (counter, waker) = Counter::waker();
    let mut fut = pin!(obs.changed());
    assert!(poll_once(&mut fut, &waker).is_pending());

    drop(src);
    assert_eq!(counter.count(), 1);
    let Poll::Ready(snap) = poll_once(&mut fut, &waker) else {
        panic!("not woken for nothing")
    };
    assert_eq!(snap.revision(), 2);
    assert_eq!(snap.of(id), None);
    assert_eq!(snap.sources().count(), 0);
}

#[test]
fn dropping_a_source_removes_its_flags_but_not_the_ones_others_hold() {
    let reg = Reg::<4, 2>::new();
    let wifi = reg.register().unwrap();
    let usb = reg.register().unwrap();
    wifi.replace(Flags::of(&[State::Connected, State::Error]));
    usb.raise(State::Connected);
    let wifi_id = wifi.id();

    let before = reg.snapshot();
    drop(wifi);
    let after = reg.snapshot();
    assert_eq!(after.revision(), before.revision() + 1);
    assert_eq!(after.aggregate(), Flags::from(State::Connected));
    assert_eq!(after.of(wifi_id), None);
}

#[test]
fn a_reused_slot_has_a_new_generation() {
    let reg = Reg::<2, 2>::new();
    let first = reg.register().unwrap();
    let old = first.id();
    first.raise(State::Error);
    drop(first);

    let second = reg.register().unwrap();
    let new = second.id();
    assert_eq!(new.slot(), old.slot(), "the freed slot is reused");
    assert_eq!(new.generation(), old.generation() + 1);
    assert_ne!(new, old);

    second.raise(State::Ready);
    let snap = reg.snapshot();
    assert_eq!(
        snap.of(old),
        None,
        "the old identifier does not reach the new owner"
    );
    assert_eq!(snap.of(new), Some(Flags::from(State::Ready)));
}

#[test]
fn snapshots_compared_across_a_reused_slot() {
    let reg = Reg::<2, 2>::new();
    let first = reg.register().unwrap();
    let old = first.id();
    first.replace(Flags::of(&[State::Connected, State::Error]));
    let before = reg.snapshot();

    drop(first);
    let second = reg.register().unwrap();
    let new = second.id();
    second.raise(State::Connected);
    let after = reg.snapshot();

    // Same slot, same flag, but not the same source: it is reported as a release and a
    // registration, never as an update.
    let changes: Vec<Change> = after.changes_since(&before).collect();
    assert_eq!(
        changes,
        [
            Change {
                source: old,
                kind: ChangeKind::Released,
                raised: Flags::EMPTY,
                cleared: Flags::of(&[State::Connected, State::Error]),
            },
            Change {
                source: new,
                kind: ChangeKind::Registered,
                raised: Flags::from(State::Connected),
                cleared: Flags::EMPTY,
            },
        ]
    );
}

#[test]
fn changes_since_lists_what_differs() {
    let reg = Reg::<4, 2>::new();
    let wifi = reg.register().unwrap();
    let usb = reg.register().unwrap();
    wifi.raise(State::Connected);
    usb.raise(State::Connected);
    let before = reg.snapshot();
    assert_eq!(before.changes_since(&before).count(), 0);

    // The aggregate does not change; the source does.
    wifi.clear(State::Connected);
    let after = reg.snapshot();
    assert_eq!(after.aggregate(), before.aggregate());
    assert_eq!(
        after.changes_since(&before).collect::<Vec<_>>(),
        [Change {
            source: wifi.id(),
            kind: ChangeKind::Updated,
            raised: Flags::EMPTY,
            cleared: Flags::from(State::Connected),
        }]
    );

    // Several sources, a registration, and an update with both raised and cleared flags.
    let extra = reg.register().unwrap();
    usb.replace(Flags::of(&[State::Connected, State::Busy]));
    wifi.raise(State::Error);
    wifi.raise(State::Connecting);
    wifi.clear(State::Error);
    let later = reg.snapshot();
    let changes: Vec<Change> = later.changes_since(&after).collect();
    assert_eq!(changes.len(), 3);
    assert!(changes.contains(&Change {
        source: extra.id(),
        kind: ChangeKind::Registered,
        raised: Flags::EMPTY,
        cleared: Flags::EMPTY,
    }));
    assert!(changes.contains(&Change {
        source: usb.id(),
        kind: ChangeKind::Updated,
        raised: Flags::from(State::Busy),
        cleared: Flags::EMPTY,
    }));
    assert!(changes.contains(&Change {
        source: wifi.id(),
        kind: ChangeKind::Updated,
        raised: Flags::from(State::Connecting),
        cleared: Flags::EMPTY,
    }));
}

#[test]
fn an_exhausted_generation_retires_the_slot() {
    let reg = Reg::<2, 2>::new();
    reg.set_generation(0, u32::MAX - 1);

    let a = reg.register().unwrap();
    let id_a = a.id();
    assert_eq!((id_a.slot(), id_a.generation()), (0, u32::MAX - 1));
    drop(a);

    // The last generation of slot 0 is used once.
    let b = reg.register().unwrap();
    let id_b = b.id();
    assert_eq!((id_b.slot(), id_b.generation()), (0, u32::MAX));
    b.raise(State::Ready);
    let snap = reg.snapshot();
    assert_eq!(snap.of(id_a), None);
    assert_eq!(snap.of(id_b), Some(Flags::from(State::Ready)));
    drop(b);

    // Then the slot is out of service: it does not come back with an old identity.
    let c = reg.register().unwrap();
    assert_eq!(c.id().slot(), 1);
    let d = reg.register();
    assert_eq!(d.err(), Some(Full), "slot 0 is retired, slot 1 is taken");
    let snap = reg.snapshot();
    assert_eq!(snap.of(id_a), None);
    assert_eq!(snap.of(id_b), None);
    drop(c);
    let e = reg.register().unwrap();
    assert_eq!(e.id().slot(), 1, "the other slot keeps working");
}

#[test]
fn every_slot_retired_means_full() {
    let reg = Reg::<1, 1>::new();
    reg.set_generation(0, u32::MAX);
    drop(reg.register().unwrap());
    assert_eq!(reg.register().err(), Some(Full));
    // Still a change-free refusal.
    let rev = reg.snapshot().revision();
    assert_eq!(reg.register().err(), Some(Full));
    assert_eq!(reg.snapshot().revision(), rev);
}

#[test]
fn capacity_of_sources_and_observers() {
    let reg = Reg::<3, 2>::new();
    let sources: Vec<_> = (0..3).map(|_| reg.register().unwrap()).collect();
    let rev = reg.snapshot().revision();
    assert_eq!(reg.register().err(), Some(Full));
    assert_eq!(reg.snapshot().revision(), rev, "a refusal is not a change");
    drop(sources);
    assert!(reg.register().is_ok(), "slots come back");

    let o1 = reg.observer().unwrap();
    let _o2 = reg.observer().unwrap();
    assert!(reg.observer().is_none());
    drop(o1);
    assert!(reg.observer().is_some(), "an observer's place comes back");

    // The largest registry.
    let big = Registry::<NoopRawMutex, 32, 1>::new();
    let all: Vec<_> = (0..32).map(|_| big.register().unwrap()).collect();
    assert_eq!(big.register().err(), Some(Full));
    assert_eq!(all[31].id().slot(), 31);
    assert_eq!(big.snapshot().sources().count(), 32);
}

// --- Observers -------------------------------------------------------------------------------

#[test]
fn a_late_observer_gets_the_current_state_at_once() {
    let reg = Reg::<4, 2>::new();
    let src = reg.register().unwrap();
    src.raise(State::Connected);

    let mut late = reg.observer().unwrap();
    let (counter, waker) = Counter::waker();
    {
        let mut fut = pin!(late.changed());
        let Poll::Ready(snap) = poll_once(&mut fut, &waker) else {
            panic!("the first consultation must not wait")
        };
        assert_eq!(snap.aggregate(), Flags::from(State::Connected));
        assert_eq!(counter.count(), 0);
    }

    // And then it waits for a change.
    let mut fut = pin!(late.changed());
    assert!(poll_once(&mut fut, &waker).is_pending());
}

#[test]
fn an_observer_of_a_fresh_registry_gets_the_empty_state_at_once() {
    let reg = Reg::<4, 2>::new();
    let mut obs = reg.observer().unwrap();
    let snap = block_on(obs.changed());
    assert_eq!(snap, Snapshot::EMPTY);
}

#[test]
fn two_independent_observers() {
    let reg = Reg::<4, 2>::new();
    let src = reg.register().unwrap();
    let mut led = reg.observer().unwrap();
    let mut log = reg.observer().unwrap();
    led.get();
    log.get();

    let (led_wakes, led_waker) = Counter::waker();
    let (log_wakes, log_waker) = Counter::waker();
    {
        let mut led_fut = pin!(led.changed());
        let mut log_fut = pin!(log.changed());
        assert!(poll_once(&mut led_fut, &led_waker).is_pending());
        assert!(poll_once(&mut log_fut, &log_waker).is_pending());

        src.raise(State::Ready);
        assert_eq!((led_wakes.count(), log_wakes.count()), (1, 1));
        // Consuming the notification in one does not take it from the other.
        assert!(poll_once(&mut led_fut, &led_waker).is_ready());
        let Poll::Ready(snap) = poll_once(&mut log_fut, &log_waker) else {
            panic!("the second observer lost its notification")
        };
        assert_eq!(snap.aggregate(), Flags::from(State::Ready));
    }

    assert!(led.try_changed().is_none());
    assert!(log.try_changed().is_none());
    src.clear(State::Ready);
    assert!(led.try_changed().is_some());
    assert!(log.try_changed().is_some());
}

#[test]
fn a_slow_observer_gets_the_last_state() {
    let reg = Reg::<4, 2>::new();
    let src = reg.register().unwrap();
    let mut slow = reg.observer().unwrap();
    let first = slow.get();

    src.raise(State::Connecting);
    src.replace(Flags::from(State::Connected));
    src.raise(State::Busy);
    src.clear(State::Busy);
    src.raise(State::Warning);

    let last = block_on(slow.changed());
    assert_eq!(last.revision(), first.revision() + 5);
    assert_eq!(
        last.of(src.id()),
        Some(Flags::of(&[State::Connected, State::Warning]))
    );
    assert!(
        slow.try_changed().is_none(),
        "one snapshot for five changes"
    );

    // What happened in between is derived by comparing, not replayed.
    let changes: Vec<_> = last.changes_since(&first).collect();
    assert_eq!(changes.len(), 1);
    assert_eq!(
        changes[0].raised,
        Flags::of(&[State::Connected, State::Warning])
    );
}

#[test]
fn the_wi_fi_drops_a_flag_the_usb_still_holds() {
    let reg = Reg::<4, 2>::new();
    let wifi = reg.register().unwrap();
    let usb = reg.register().unwrap();
    let mut led = reg.observer().unwrap();
    let mut diag = reg.observer().unwrap();
    wifi.raise(State::Connected);
    usb.raise(State::Connected);
    let led_before = led.get();
    let diag_before = diag.get();
    assert_eq!(led_before.aggregate(), Flags::from(State::Connected));

    wifi.clear(State::Connected);

    // The aggregate is the same, and both observers are told anyway.
    let led_after = block_on(led.changed());
    let diag_after = block_on(diag.changed());
    assert_eq!(led_after.aggregate(), led_before.aggregate());
    let changes: Vec<_> = diag_after.changes_since(&diag_before).collect();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].source, wifi.id());
    assert_eq!(changes[0].cleared, Flags::from(State::Connected));

    usb.clear(State::Connected);
    assert!(block_on(led.changed()).aggregate().is_empty());
}

#[test]
fn no_wake_up_is_lost_between_reading_and_waiting() {
    let reg = Reg::<4, 2>::new();
    let src = reg.register().unwrap();
    let mut obs = reg.observer().unwrap();

    // 1. The change comes after the observer read the snapshot and before it waits.
    obs.get();
    src.raise(State::Ready);
    let (counter, waker) = Counter::waker();
    {
        let mut fut = pin!(obs.changed());
        assert!(
            poll_once(&mut fut, &waker).is_ready(),
            "seen at the first poll"
        );
    }
    assert_eq!(counter.count(), 0);

    // 2. The change comes after the observer registered its wake-up.
    {
        let mut fut = pin!(obs.changed());
        assert!(poll_once(&mut fut, &waker).is_pending());
        src.raise(State::Busy);
        assert_eq!(counter.count(), 1, "woken");
        assert!(poll_once(&mut fut, &waker).is_ready());
    }

    // 3. A future dropped while waiting loses nothing.
    {
        let mut fut = pin!(obs.changed());
        assert!(poll_once(&mut fut, &waker).is_pending());
    }
    src.raise(State::Running);
    assert!(obs.try_changed().is_some());

    // 4. A change consumed by `get` is not announced again.
    src.raise(State::Healthy);
    obs.get();
    assert!(obs.try_changed().is_none());
}

#[test]
fn more_waiting_tasks_than_wakers_means_spurious_wake_ups_never_a_loss() {
    // One waker slot, two observers (two tasks) waiting.
    let reg = Registry::<NoopRawMutex, 2, 1>::new();
    let src = reg.register().unwrap();
    let mut a = reg.observer().unwrap();
    assert!(reg.observer().is_none(), "R = 1 observer");
    a.get();
    let (a_wakes, a_waker) = Counter::waker();
    let (b_wakes, b_waker) = Counter::waker();
    let mut fut = pin!(a.changed());
    // The same observer polled from two different tasks over time.
    assert!(poll_once(&mut fut, &a_waker).is_pending());
    assert!(poll_once(&mut fut, &b_waker).is_pending());
    assert_eq!(a_wakes.count(), 1, "pushed out: woken to register again");
    src.raise(State::Ready);
    assert_eq!(b_wakes.count(), 1);
    assert!(poll_once(&mut fut, &b_waker).is_ready());
}

// --- Concurrency -----------------------------------------------------------------------------

#[test]
fn several_threads_producing_and_observing() {
    const PRODUCERS: usize = 4;
    const ROUNDS: u64 = 400;
    const BOTH: Flags = Flags::of(&[State::Connected, State::Ready]);
    // Registrations, two replaces per round, releases.
    const FINAL: u64 = PRODUCERS as u64 * (2 + 2 * ROUNDS);

    let reg = Registry::<CriticalSectionRawMutex, 8, 3>::new();

    thread::scope(|scope| {
        let observers: Vec<_> = (0..3)
            .map(|_| {
                let mut obs = reg.observer().unwrap();
                scope.spawn(move || {
                    let mut last = 0;
                    let mut seen = 0;
                    loop {
                        let snap = block_on(obs.changed());
                        assert!(snap.revision() > last || seen == 0, "revisions only grow");
                        last = snap.revision();
                        seen += 1;
                        // `replace` is atomic: a source holds both flags or none.
                        for (_, flags) in snap.sources() {
                            assert!(flags.is_empty() || flags == BOTH, "{flags:?}");
                        }
                        if snap.revision() == FINAL {
                            return seen;
                        }
                    }
                })
            })
            .collect();

        for _ in 0..PRODUCERS {
            scope.spawn(|| {
                let src = reg.register().unwrap();
                for _ in 0..ROUNDS {
                    assert!(src.replace(BOTH));
                    assert!(src.replace(Flags::EMPTY));
                }
            });
        }

        for obs in observers {
            // Every observer ends on the final state, however many snapshots it merged.
            assert!(obs.join().unwrap() >= 1);
        }
    });

    let snap = reg.snapshot();
    assert_eq!(snap.revision(), FINAL);
    assert_eq!(snap.sources().count(), 0);
    assert!(snap.aggregate().is_empty());
}

#[test]
fn concurrent_sources_never_lose_each_others_flags() {
    const ROUNDS: usize = 500;
    let reg = Registry::<CriticalSectionRawMutex, 4, 1>::new();
    let shared = reg.register().unwrap();
    let wifi = reg.register().unwrap();
    let usb = reg.register().unwrap();
    shared.raise(State::Ready);

    thread::scope(|scope| {
        for (src, own) in [(&wifi, State::Connected), (&usb, State::Connected)] {
            scope.spawn(move || {
                for _ in 0..ROUNDS {
                    src.raise(own);
                    src.clear(own);
                }
            });
        }
        scope.spawn(|| {
            for _ in 0..ROUNDS {
                let snap = reg.snapshot();
                assert!(snap.aggregate().contains(State::Ready));
            }
        });
    });
    assert_eq!(reg.snapshot().aggregate(), Flags::from(State::Ready));
}

// --- Memory ----------------------------------------------------------------------------------

#[test]
fn memory_cost() {
    use core::mem::size_of;

    println!(
        "host ({}-bit): Registry<Noop, S, R> / Snapshot<S> bytes",
        usize::BITS
    );
    for (s, snap) in [
        (4, size_of::<Snapshot<4>>()),
        (8, size_of::<Snapshot<8>>()),
        (16, size_of::<Snapshot<16>>()),
        (32, size_of::<Snapshot<32>>()),
    ] {
        println!("S={s}: Snapshot {snap}");
    }
    println!(
        "Registry<_, 4, 2> {}, <_, 8, 4> {}, <_, 16, 4> {}, <_, 32, 4> {}",
        size_of::<Reg<4, 2>>(),
        size_of::<Reg<8, 4>>(),
        size_of::<Reg<16, 4>>(),
        size_of::<Reg<32, 4>>(),
    );
    println!(
        "Flags {}, SourceId {}, Source {}, Observer {}",
        size_of::<Flags>(),
        size_of::<SourceId>(),
        size_of::<Source<'_, NoopRawMutex, 8, 4>>(),
        size_of::<Observer<'_, NoopRawMutex, 8, 4>>(),
    );

    assert_eq!(size_of::<Flags>(), 8);
    assert_eq!(size_of::<SourceId>(), 8);
    // Snapshot: revision 8 + flags 8 S + generations 4 S + occupied 4, rounded to 8.
    for (actual, s) in [
        (size_of::<Snapshot<4>>(), 4),
        (size_of::<Snapshot<8>>(), 8),
        (size_of::<Snapshot<32>>(), 32),
    ] {
        assert_eq!(actual, (8 + 12 * s + 4usize).next_multiple_of(8));
    }
    // The registry is the table plus the wakers: no hidden buffer.
    let table = |s: usize| 8 + 12 * s + 4 + 4 + 8;
    assert!(size_of::<Reg<8, 4>>() <= table(8) + 16 + 4 * 2 * size_of::<usize>() + 8);
}

#[test]
fn a_registry_can_be_static() {
    static STATE: Registry<CriticalSectionRawMutex, 4, 2> = Registry::new();
    let src = STATE.register().unwrap();
    src.raise(State::Ready);
    assert_eq!(STATE.snapshot().aggregate(), Flags::from(State::Ready));
}
