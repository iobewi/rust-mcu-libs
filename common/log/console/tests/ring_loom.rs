//! Exhaustive interleavings (with the C11 memory model, through `loom`) of the ring's freeze
//! protocol. Run with:
//!
//! ```text
//! RUSTFLAGS="--cfg loom" cargo test -p iobewi-console --release --test ring_loom
//! ```
//!
//! Every scenario freezes the ring from an "emergency" thread while other threads deposit and
//! pump, then checks that (1) loom saw no data race on the buffer, (2) after a confirmed freeze
//! nothing changed, and (3) what the device took plus what the emergency path recovered is the
//! stream of whole messages, with no loss, no duplicate and no torn message.
//!
//! `--cfg loom_no_recheck` and `--cfg loom_no_fence` break the protocol on purpose; the tests
//! must then fail (they are run that way once to prove the model is sensitive).
#![cfg(loom)]

use iobewi_console::ring::{Claim, Deposit, FreezeError, Platform, Ring};
use loom::{
    cell::Cell,
    sync::{Arc, Mutex},
    thread,
};

loom::thread_local! { static CORE: Cell<u32> = Cell::new(0); }

struct LoomPlatform(Mutex<()>);

impl Platform for LoomPlatform {
    fn with<R>(&self, f: impl FnOnce() -> R) -> R {
        let _guard = self.0.lock().unwrap();
        f()
    }
    fn core(&self) -> u32 {
        CORE.with(|c| c.get())
    }
}

type R = Ring<16, LoomPlatform>;

fn ring() -> Arc<R> {
    Arc::new(Ring::new(LoomPlatform(Mutex::new(()))))
}

const M1: &[u8] = b"ab\n";
const M2: &[u8] = b"cde\n";

fn model(f: impl Fn() + Sync + Send + 'static) {
    let mut b = loom::model::Builder::new();
    b.preemption_bound = Some(4);
    b.check(f);
}

/// The emergency path, as the panic handler runs it, on core 0. Returns what it recovered.
fn emergency(ring: &R) -> Option<Vec<u8>> {
    assert_eq!(ring.claim(), Claim::Winner);
    let mut polls = 0;
    match ring.freeze(|| {
        polls += 1;
        thread::yield_now();
        polls > 3
    }) {
        Ok(frozen) => {
            let mut got = Vec::new();
            frozen.for_each(4, |c| got.extend_from_slice(c));
            Some(got)
        }
        Err(FreezeError::Timeout) => None,
        Err(other) => panic!("unexpected {other:?}"),
    }
}

fn is_whole_stream(bytes: &[u8]) -> bool {
    bytes.is_empty() || bytes == M1 || bytes == [M1, M2].concat()
}

/// Producer deposits two messages while the emergency path freezes.
#[test]
fn freeze_against_a_producer() {
    model(|| {
        let ring = ring();
        let p = {
            let ring = ring.clone();
            thread::spawn(move || {
                CORE.with(|c| c.set(1));
                let _ = ring.deposit(M1);
                let _ = ring.deposit(M2);
            })
        };
        let recovered = emergency(&ring);
        let frozen_pending = ring.pending();
        p.join().unwrap();
        if let Some(got) = recovered {
            assert!(is_whole_stream(&got), "torn or partial: {got:?}");
            // nothing changed after the freeze
            assert_eq!(ring.pending(), got.len());
            assert_eq!(frozen_pending, got.len());
        }
    });
}

/// Producer and a pump (the device takes 2 bytes at a time) against the emergency path.
#[test]
fn freeze_against_a_producer_and_a_pump() {
    model(|| {
        let ring = ring();
        let sink = Arc::new(Mutex::new(Vec::<u8>::new()));
        let p = {
            let ring = ring.clone();
            thread::spawn(move || {
                CORE.with(|c| c.set(1));
                let _ = ring.deposit(M1);
                let _ = ring.deposit(M2);
            })
        };
        let d = {
            let (ring, sink) = (ring.clone(), sink.clone());
            thread::spawn(move || {
                CORE.with(|c| c.set(1));
                ring.pump(|b| {
                    let n = b.len().min(2);
                    sink.lock().unwrap().extend_from_slice(&b[..n]);
                    n
                });
            })
        };
        let recovered = emergency(&ring);
        let (pending, taken_at_freeze) = (ring.pending(), sink.lock().unwrap().len());
        p.join().unwrap();
        d.join().unwrap();
        if let Some(got) = recovered {
            let taken = sink.lock().unwrap().clone();
            let all = [taken.clone(), got.clone()].concat();
            assert!(
                is_whole_stream(&all),
                "lost, duplicated or torn: {taken:?} + {got:?}"
            );
            // the device took nothing more after the freeze, and the ring did not change
            assert_eq!(taken.len(), taken_at_freeze);
            assert_eq!(pending, got.len());
            assert_eq!(ring.pending(), got.len());
        }
    });
}

/// A deposit that lands after the freeze is refused and leaves no trace.
#[test]
fn a_deposit_after_the_freeze_is_refused() {
    model(|| {
        let ring = ring();
        assert_eq!(ring.deposit(M1), Deposit::Accepted);
        let p = {
            let ring = ring.clone();
            thread::spawn(move || {
                CORE.with(|c| c.set(1));
                ring.deposit(M2)
            })
        };
        let recovered = emergency(&ring);
        let late = p.join().unwrap();
        if let Some(got) = recovered {
            match late {
                Deposit::Accepted => assert_eq!(got, [M1, M2].concat()),
                Deposit::Frozen => assert_eq!(got, M1),
                Deposit::Dropped => panic!("nothing is full here"),
            }
            assert_eq!(ring.pending(), got.len());
        }
    });
}

/// The loser of two simultaneous emergencies never freezes nor reads.
#[test]
fn only_one_emergency_wins() {
    model(|| {
        let ring = ring();
        let other = {
            let ring = ring.clone();
            thread::spawn(move || {
                CORE.with(|c| c.set(1));
                ring.claim()
            })
        };
        let mine = ring.claim();
        let theirs = other.join().unwrap();
        assert_ne!(mine, theirs);
    });
}
