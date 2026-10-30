//! Transmit ring of an application console: whole messages are deposited atomically, a single
//! pump moves them in order to the physical device, and an emergency path can freeze the ring
//! and read what was never transferred.
//!
//! # Roles
//!
//! * **Actors** are the ordinary users of the ring: [`Ring::deposit`] (producers) and
//!   [`Ring::pump`] (the interrupt that drains it, a producer that helps, a flush). Every actor
//!   runs inside [`Platform::with`], a short exclusion (on a microcontroller: the critical
//!   section). At most one actor is inside the ring at any time, whatever its core.
//! * **The emergency path** (a panic handler) never takes that exclusion: it must work even if
//!   another core died inside it. It uses [`Ring::claim`] and [`Ring::freeze`] instead.
//!
//! # Freeze protocol (the part that needs care)
//!
//! Actor, inside the exclusion:
//!
//! ```text
//! busy.store(core + 1, Relaxed);  fence(SeqCst);
//! if emerg.load(Relaxed) != 0 { busy.store(0, Release); return Frozen }   // touches nothing
//! ... copy bytes then head.store(Release)  |  write to the device then tail.store(Release) ...
//! witness.store(witness_of(head, tail), Release);  busy.store(0, Release)
//! ```
//!
//! Emergency:
//!
//! ```text
//! claim():   emerg 0 -> 1                       (the winner)
//! freeze():  fence(SeqCst); loop { b = busy.load(Acquire); b == 0 -> go; b == me -> Own;
//!                                  expired() -> Timeout }
//!            head.load(Acquire), tail.load(Acquire), checks
//! ```
//!
//! This is Dekker's handshake with `SeqCst` fences: the emergency path writes `emerg` then reads
//! `busy`, an actor writes `busy` then reads `emerg`. In the total order of the two fences either
//! the fence of the emergency path comes first, and the actor sees `emerg != 0` and leaves the
//! ring untouched, or the fence of the actor comes first, and the emergency path sees `busy != 0`
//! (or the later `0` of that same actor, whose `Release` store publishes every write it made) and
//! waits for it. The first `0` read after the fence is therefore enough: an actor that starts
//! later reads `emerg != 0`. After a successful freeze nobody writes the buffer or an index, so
//! plain reads of `[tail, head)` are free of data races.
//!
//! # Invariants
//!
//! 1. `head` advances once per message, after the complete copy, on a message boundary.
//! 2. `tail` advances only after the device accepted the bytes, by the count it accepted: it is
//!    exactly the number of bytes definitively transferred.
//! 3. `head`, `tail`, `witness`, `busy` and the loss counters are written by the active actor only.
//! 4. After a confirmed freeze no byte and no index changes.
//! 5. `head - tail <= N` (modulo 2^32) and `witness == witness_of(head, tail)`; otherwise the state
//!    is doubtful and nothing is replayed.

use core::{fmt::Write as _, marker::PhantomData};

#[cfg(not(loom))]
use core::{
    cell::UnsafeCell,
    sync::atomic::{AtomicU32, Ordering::*, fence},
};
#[cfg(loom)]
use loom::{
    cell::UnsafeCell,
    sync::atomic::{AtomicU32, Ordering::*, fence},
};

use crate::LineBuf;

/// Redundant image of the two indices. Zero for an empty ring at position 0, so that a fresh
/// ring is all zeroes and lives in `.bss`, not in the initialised data copied from flash.
fn witness_of(head: u32, tail: u32) -> u32 {
    head ^ tail.rotate_left(13)
}
const MARKER_MAX: usize = 64;

/// What a platform provides: a short exclusion and the identity of the running core.
pub trait Platform {
    /// Runs `f` alone: no other actor, on any core, runs its own `with` meanwhile. It may be
    /// re-entered by the same core.
    fn with<R>(&self, f: impl FnOnce() -> R) -> R;
    /// Identity of the running core, from 0.
    fn core(&self) -> u32;
}

/// Result of [`Ring::deposit`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Deposit {
    /// The whole message is in the ring.
    Accepted,
    /// No room, or the message is longer than the ring: nothing was written, the loss is counted.
    Dropped,
    /// An emergency froze the ring: nothing was written, nothing is counted.
    Frozen,
}

/// Result of [`Ring::pump`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pump {
    /// An emergency froze the ring: nothing was moved.
    Frozen,
    /// `moved` bytes went to the device; `remaining` are still in the ring.
    Done { moved: usize, remaining: usize },
}

/// Why [`Ring::freeze`] did not confirm the freeze.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FreezeError {
    /// The emergency happened inside the ring on this very core: the state is unknown.
    Own,
    /// An actor on another core did not leave the ring in time.
    Timeout,
    /// Indices or witness are inconsistent.
    Doubtful,
}

/// Who handles the emergency.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Claim {
    Winner,
    Loser,
}

struct Bytes<const N: usize>(UnsafeCell<[u8; N]>);

impl<const N: usize> Bytes<N> {
    #[cfg(not(loom))]
    const fn new() -> Self {
        Self(UnsafeCell::new([0; N]))
    }
    #[cfg(loom)]
    fn new() -> Self {
        Self(UnsafeCell::new([0; N]))
    }
    #[cfg(not(loom))]
    fn with<R>(&self, f: impl FnOnce(*const [u8; N]) -> R) -> R {
        f(self.0.get())
    }
    #[cfg(loom)]
    fn with<R>(&self, f: impl FnOnce(*const [u8; N]) -> R) -> R {
        self.0.with(f)
    }
    #[cfg(not(loom))]
    fn with_mut<R>(&self, f: impl FnOnce(*mut [u8; N]) -> R) -> R {
        f(self.0.get())
    }
    #[cfg(loom)]
    fn with_mut<R>(&self, f: impl FnOnce(*mut [u8; N]) -> R) -> R {
        self.0.with_mut(f)
    }
}

/// The ring. `N` is a power of two.
pub struct Ring<const N: usize, P: Platform> {
    buf: Bytes<N>,
    head: AtomicU32,
    tail: AtomicU32,
    witness: AtomicU32,
    busy: AtomicU32,
    emerg: AtomicU32,
    second: AtomicU32,
    second_done: AtomicU32,
    fatal_done: AtomicU32,
    lost_total: AtomicU32,
    lost_unreported: AtomicU32,
    platform: P,
    _p: PhantomData<P>,
}

// SAFETY: the buffer is written only by the active actor (alone inside `Platform::with`) and
// read by the emergency path only after a confirmed freeze (see the module documentation); every
// other field is atomic.
unsafe impl<const N: usize, P: Platform + Sync> Sync for Ring<N, P> {}

impl<const N: usize, P: Platform> Ring<N, P> {
    const CHECK: () = assert!(N.is_power_of_two() && N >= 16 && N <= (1 << 30));
    const MASK: u32 = (N - 1) as u32;

    #[cfg(not(loom))]
    pub const fn new(platform: P) -> Self {
        let () = Self::CHECK;
        Self {
            buf: Bytes::new(),
            head: AtomicU32::new(0),
            tail: AtomicU32::new(0),
            witness: AtomicU32::new(0),
            busy: AtomicU32::new(0),
            emerg: AtomicU32::new(0),
            second: AtomicU32::new(0),
            second_done: AtomicU32::new(0),
            fatal_done: AtomicU32::new(0),
            lost_total: AtomicU32::new(0),
            lost_unreported: AtomicU32::new(0),
            platform,
            _p: PhantomData,
        }
    }

    #[cfg(loom)]
    pub fn new(platform: P) -> Self {
        let () = Self::CHECK;
        Self {
            buf: Bytes::new(),
            head: AtomicU32::new(0),
            tail: AtomicU32::new(0),
            witness: AtomicU32::new(0),
            busy: AtomicU32::new(0),
            emerg: AtomicU32::new(0),
            second: AtomicU32::new(0),
            second_done: AtomicU32::new(0),
            fatal_done: AtomicU32::new(0),
            lost_total: AtomicU32::new(0),
            lost_unreported: AtomicU32::new(0),
            platform,
            _p: PhantomData,
        }
    }

    /// Enters the ring as an actor. Inside `Platform::with`. `false`: an emergency froze it.
    fn enter(&self) -> bool {
        self.busy.store(self.platform.core() + 1, Relaxed);
        // The two `loom_*` switches exist to break the protocol on purpose: the loom model must
        // then fail, which shows that it really checks what it claims to.
        #[cfg(not(loom_no_fence))]
        fence(SeqCst);
        #[cfg(not(loom_no_recheck))]
        if self.emerg.load(Relaxed) != 0 {
            self.busy.store(0, Release);
            return false;
        }
        true
    }

    fn leave(&self) {
        let head = self.head.load(Relaxed);
        let tail = self.tail.load(Relaxed);
        self.witness.store(witness_of(head, tail), Release);
        self.busy.store(0, Release);
    }

    /// Whether an emergency is under way. Cheap, for the fast path of interrupt handlers.
    pub fn emergency(&self) -> bool {
        self.emerg.load(Acquire) != 0
    }

    /// Deposits one message, whole or not at all. Never waits for the device.
    ///
    /// If earlier messages were dropped, a `WARN - console: N message(s) dropped` line goes in
    /// first, in the same step, when both fit; if only the message fits it goes alone and the
    /// marker waits for the next deposit.
    pub fn deposit(&self, msg: &[u8]) -> Deposit {
        if msg.is_empty() {
            return Deposit::Accepted;
        }
        if self.emerg.load(Relaxed) != 0 {
            return Deposit::Frozen;
        }
        self.platform.with(|| {
            if !self.enter() {
                return Deposit::Frozen;
            }
            let result = self.deposit_in(msg);
            self.leave();
            result
        })
    }

    fn deposit_in(&self, msg: &[u8]) -> Deposit {
        let head = self.head.load(Relaxed);
        let tail = self.tail.load(Relaxed);
        let free = N - head.wrapping_sub(tail) as usize;
        let unreported = self.lost_unreported.load(Relaxed);
        let mut marker = LineBuf::<MARKER_MAX>::new();
        if unreported > 0 {
            let _ = write!(marker, "WARN - console: {unreported} message(s) dropped");
            marker.finish();
        }
        let with_marker = marker.as_bytes().len() + msg.len();
        let (marker, total) = if unreported > 0 && with_marker <= free {
            (marker.as_bytes(), with_marker)
        } else if msg.len() <= free {
            (&[][..], msg.len())
        } else {
            self.lost_total
                .store(self.lost_total.load(Relaxed).saturating_add(1), Relaxed);
            self.lost_unreported
                .store(unreported.saturating_add(1), Relaxed);
            return Deposit::Dropped;
        };
        self.buf.with_mut(|base| {
            let base = base.cast::<u8>();
            let mut at = head;
            for part in [marker, msg] {
                let off = (at & Self::MASK) as usize;
                let first = part.len().min(N - off);
                // SAFETY: `[head, head + total)` is free (checked above) and nobody else writes
                // or reads the buffer here (module documentation); both copies stay inside it.
                unsafe {
                    core::ptr::copy_nonoverlapping(part.as_ptr(), base.add(off), first);
                    core::ptr::copy_nonoverlapping(
                        part.as_ptr().add(first),
                        base,
                        part.len() - first,
                    );
                }
                at = at.wrapping_add(part.len() as u32);
            }
        });
        if !marker.is_empty() {
            self.lost_unreported.store(0, Relaxed);
        }
        self.head.store(head.wrapping_add(total as u32), Release);
        Deposit::Accepted
    }

    /// Offers the oldest bytes to `write`, which returns how many the device took and must not
    /// wait. Repeats until the ring is empty or `write` takes fewer than offered.
    pub fn pump(&self, mut write: impl FnMut(&[u8]) -> usize) -> Pump {
        if self.emerg.load(Relaxed) != 0 {
            return Pump::Frozen;
        }
        self.platform.with(|| {
            if !self.enter() {
                return Pump::Frozen;
            }
            let head = self.head.load(Relaxed);
            let mut tail = self.tail.load(Relaxed);
            let mut moved = 0;
            while tail != head {
                let off = (tail & Self::MASK) as usize;
                let avail = head.wrapping_sub(tail) as usize;
                let run = avail.min(N - off);
                // SAFETY: bytes `[tail, head)` were published by an earlier actor and nobody
                // writes them; the slice does not leave the buffer.
                let took = self.buf.with(|base| {
                    write(unsafe { core::slice::from_raw_parts(base.cast::<u8>().add(off), run) })
                });
                let took = took.min(run);
                if took == 0 {
                    break;
                }
                // The device has the bytes: only now do they leave the ring.
                tail = tail.wrapping_add(took as u32);
                self.tail.store(tail, Release);
                moved += took;
                if took < run {
                    break;
                }
            }
            self.leave();
            Pump::Done {
                moved,
                remaining: head.wrapping_sub(tail) as usize,
            }
        })
    }

    /// Bytes deposited and not yet transferred (advisory: another actor may change it).
    pub fn pending(&self) -> usize {
        self.head
            .load(Acquire)
            .wrapping_sub(self.tail.load(Acquire)) as usize
    }

    /// Messages dropped since the start.
    pub fn lost_total(&self) -> u32 {
        self.lost_total.load(Relaxed)
    }

    // ---- emergency path ----

    /// Declares the emergency. The winner freezes and replays; the loser must not touch the ring.
    pub fn claim(&self) -> Claim {
        #[cfg(any(loom, target_has_atomic = "ptr"))]
        let won = self.emerg.compare_exchange(0, 1, AcqRel, Acquire).is_ok();
        // No compare-and-swap (ESP32-C3): a single core, and a panic inside this very sequence
        // would be a second panic of the emergency path itself.
        #[cfg(not(any(loom, target_has_atomic = "ptr")))]
        let won = {
            let free = self.emerg.load(Acquire) == 0;
            if free {
                self.emerg.store(1, Release);
            }
            free
        };
        if won { Claim::Winner } else { Claim::Loser }
    }

    /// Winner only, after [`Ring::claim`]: confirms that nothing changes the ring any more and
    /// reads its state. `expired` is polled while waiting for an actor of another core.
    pub fn freeze(
        &self,
        mut expired: impl FnMut() -> bool,
    ) -> Result<Frozen<'_, N, P>, FreezeError> {
        #[cfg(not(loom_no_fence))]
        fence(SeqCst);
        let me = self.platform.core() + 1;
        loop {
            let busy = self.busy.load(Acquire);
            if busy == 0 {
                break;
            }
            if busy == me {
                return Err(FreezeError::Own);
            }
            if expired() {
                return Err(FreezeError::Timeout);
            }
            core::hint::spin_loop();
        }
        let head = self.head.load(Acquire);
        let tail = self.tail.load(Acquire);
        if head.wrapping_sub(tail) as usize > N
            || self.witness.load(Acquire) != witness_of(head, tail)
        {
            return Err(FreezeError::Doubtful);
        }
        Ok(Frozen {
            ring: self,
            tail,
            len: head.wrapping_sub(tail) as usize,
        })
    }

    /// Loser announces itself; the winner then waits briefly for its line.
    pub fn announce_second(&self) {
        self.second.store(1, Release);
    }
    pub fn second_announced(&self) -> bool {
        self.second.load(Acquire) != 0
    }
    /// Winner: its fatal line is out.
    pub fn mark_fatal_done(&self) {
        self.fatal_done.store(1, Release);
    }
    pub fn fatal_done(&self) -> bool {
        self.fatal_done.load(Acquire) != 0
    }
    /// Loser: its own line is out, the winner may go on with the replay.
    pub fn mark_second_done(&self) {
        self.second_done.store(1, Release);
    }
    pub fn second_done(&self) -> bool {
        self.second_done.load(Acquire) != 0
    }
}

/// The ring after a confirmed freeze: its pending bytes, which nobody can change any more.
pub struct Frozen<'a, const N: usize, P: Platform> {
    ring: &'a Ring<N, P>,
    tail: u32,
    len: usize,
}

impl<const N: usize, P: Platform> Frozen<'_, N, P> {
    /// Bytes never transferred to the device.
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Hands the pending bytes, unchanged and in order, to `f`, at most `chunk` at a time.
    pub fn for_each(&self, chunk: usize, mut f: impl FnMut(&[u8])) {
        let chunk = chunk.max(1);
        let mut done = 0;
        while done < self.len {
            let at = self.tail.wrapping_add(done as u32);
            let off = (at & Ring::<N, P>::MASK) as usize;
            let run = (self.len - done).min(N - off).min(chunk);
            // SAFETY: the freeze was confirmed, so no actor writes the buffer or an index; the
            // slice stays inside the pending region.
            self.ring.buf.with(|base| {
                f(unsafe { core::slice::from_raw_parts(base.cast::<u8>().add(off), run) })
            });
            done += run;
        }
    }
}

#[cfg(all(test, not(loom)))]
impl<const N: usize, P: Platform> Ring<N, P> {
    /// Test hooks: start close to the index wrap, fake a panic inside the ring, corrupt it.
    fn at_position(self, position: u32) -> Self {
        self.head.store(position, Relaxed);
        self.tail.store(position, Relaxed);
        self.witness.store(witness_of(position, position), Relaxed);
        self
    }
    fn force_busy(&self, core: u32) {
        self.busy.store(core + 1, Relaxed);
    }
    fn corrupt_witness(&self) {
        self.witness.store(0xDEAD, Relaxed);
    }
    fn set_indices(&self, head: u32, tail: u32) {
        self.head.store(head, Relaxed);
        self.tail.store(tail, Relaxed);
        self.witness.store(witness_of(head, tail), Relaxed);
    }
}

#[cfg(all(test, not(loom)))]
mod tests {
    extern crate std;
    use super::*;
    use std::{cell::Cell, sync::Mutex, vec::Vec};

    std::thread_local! { static CORE: Cell<u32> = const { Cell::new(0) }; }

    struct StdPlatform(Mutex<()>);
    impl StdPlatform {
        const fn new() -> Self {
            Self(Mutex::new(()))
        }
    }
    impl Platform for StdPlatform {
        fn with<R>(&self, f: impl FnOnce() -> R) -> R {
            // The real critical section is re-entrant; these tests never re-enter.
            let _g = self.0.lock().unwrap();
            f()
        }
        fn core(&self) -> u32 {
            CORE.with(Cell::get)
        }
    }

    fn ring<const N: usize>() -> Ring<N, StdPlatform> {
        Ring::new(StdPlatform::new())
    }

    fn drain<const N: usize>(r: &Ring<N, StdPlatform>, per_call: usize) -> Vec<u8> {
        let mut out = Vec::new();
        while r.pending() > 0 {
            let p = r.pump(|b| {
                let n = b.len().min(per_call);
                out.extend_from_slice(&b[..n]);
                n
            });
            assert_ne!(p, Pump::Frozen);
        }
        out
    }

    #[test]
    fn messages_come_out_whole_and_in_order() {
        let r = ring::<64>();
        assert_eq!(r.deposit(b"first\n"), Deposit::Accepted);
        assert_eq!(r.deposit(b"second\n"), Deposit::Accepted);
        assert_eq!(drain(&r, 1000), b"first\nsecond\n");
        assert_eq!(r.pending(), 0);
    }

    #[test]
    fn bytes_are_not_transformed() {
        let r = ring::<64>();
        let raw = [
            0x00u8, 0x01, 0x7f, 0x80, 0xff, b'\r', b'\n', 0x1b, b'[', b'3', b'1', b'm',
        ];
        assert_eq!(r.deposit(&raw), Deposit::Accepted);
        assert_eq!(drain(&r, 5), raw);
    }

    #[test]
    fn wraps_around_the_end_of_the_buffer() {
        let r = ring::<16>();
        let mut expect = Vec::new();
        let mut out = Vec::new();
        for i in 0..50u8 {
            let m = [b'a' + (i % 26), b'-', b'0' + (i % 10), b'\n'];
            assert_eq!(r.deposit(&m), Deposit::Accepted);
            expect.extend_from_slice(&m);
            out.extend(drain(&r, 3));
        }
        assert_eq!(out, expect);
    }

    #[test]
    fn indices_may_wrap_past_u32_max() {
        let r = ring::<32>().at_position(u32::MAX - 5);
        let mut expect = Vec::new();
        let mut out = Vec::new();
        for i in 0..40u8 {
            let m = [b'x', b'0' + (i % 10), b'y', b'z', b'\n'];
            assert_eq!(r.deposit(&m), Deposit::Accepted);
            expect.extend_from_slice(&m);
            out.extend(drain(&r, 4));
        }
        assert_eq!(out, expect);
    }

    #[test]
    fn full_ring_drops_the_whole_message_and_counts() {
        let r = ring::<16>();
        assert_eq!(r.deposit(b"0123456789\n"), Deposit::Accepted); // 11 of 16
        assert_eq!(r.deposit(b"abcdefgh\n"), Deposit::Dropped); // 9 > 5 free
        assert_eq!(r.lost_total(), 1);
        assert_eq!(r.pending(), 11); // nothing partial
        assert_eq!(drain(&r, 100), b"0123456789\n");
    }

    #[test]
    fn a_message_longer_than_the_ring_is_dropped_whole() {
        let r = ring::<16>();
        assert_eq!(r.deposit(&[b'x'; 17]), Deposit::Dropped);
        assert_eq!(r.pending(), 0);
        assert_eq!(r.lost_total(), 1);
    }

    #[test]
    fn the_loss_marker_comes_before_the_next_accepted_message() {
        let r = ring::<128>();
        assert_eq!(r.deposit(&[b'a'; 100]), Deposit::Accepted);
        assert_eq!(r.deposit(&[b'b'; 40]), Deposit::Dropped);
        assert_eq!(r.deposit(&[b'c'; 40]), Deposit::Dropped);
        assert_eq!(drain(&r, 1000), [b'a'; 100]);
        assert_eq!(r.deposit(b"next\n"), Deposit::Accepted);
        assert_eq!(
            drain(&r, 1000),
            b"WARN - console: 2 message(s) dropped\nnext\n"
        );
        // reported once
        assert_eq!(r.deposit(b"more\n"), Deposit::Accepted);
        assert_eq!(drain(&r, 1000), b"more\n");
        assert_eq!(r.lost_total(), 2);
    }

    #[test]
    fn the_marker_never_pushes_out_a_message_that_fits() {
        let r = ring::<64>();
        assert_eq!(r.deposit(&[b'a'; 60]), Deposit::Accepted);
        assert_eq!(r.deposit(&[b'b'; 10]), Deposit::Dropped);
        assert_eq!(drain(&r, 1000), [b'a'; 60]);
        // 60 bytes: fits alone, not with the 37-byte marker
        assert_eq!(r.deposit(&[b'c'; 60]), Deposit::Accepted);
        assert_eq!(drain(&r, 1000), [b'c'; 60]);
        assert_eq!(r.deposit(b"z\n"), Deposit::Accepted);
        assert_eq!(
            drain(&r, 1000),
            b"WARN - console: 1 message(s) dropped\nz\n"
        );
    }

    #[test]
    fn a_device_that_takes_nothing_leaves_the_ring_untouched() {
        let r = ring::<32>();
        r.deposit(b"hello\n");
        assert_eq!(
            r.pump(|_| 0),
            Pump::Done {
                moved: 0,
                remaining: 6
            }
        );
        assert_eq!(drain(&r, 100), b"hello\n");
    }

    #[test]
    fn a_slow_device_conserves_every_byte() {
        let r = ring::<32>();
        let mut sent = Vec::new();
        let mut out = Vec::new();
        for i in 0..200u32 {
            let m = std::format!("line {i}\n").into_bytes();
            if r.deposit(&m) == Deposit::Accepted {
                sent.extend_from_slice(&m);
            }
            // the device takes 2 bytes per call
            r.pump(|b| {
                let n = b.len().min(2);
                out.extend_from_slice(&b[..n]);
                n
            });
        }
        out.extend(drain(&r, 2));
        // accepted messages, minus the markers, arrive complete and in order
        let text = std::string::String::from_utf8(out).unwrap();
        let kept: Vec<&str> = text.lines().filter(|l| !l.starts_with("WARN")).collect();
        let want: Vec<std::string::String> = std::string::String::from_utf8(sent)
            .unwrap()
            .lines()
            .map(std::string::String::from)
            .collect();
        assert_eq!(kept, want);
    }

    #[test]
    fn producers_on_two_threads_never_interleave_messages() {
        use std::sync::atomic::{AtomicBool, Ordering};
        static R: Ring<256, StdPlatform> = Ring::new(StdPlatform::new());
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let out = std::sync::Arc::new(Mutex::new(Vec::<u8>::new()));
        let mut handles = Vec::new();
        for core in 0..2u32 {
            handles.push(std::thread::spawn(move || {
                CORE.with(|c| c.set(core));
                let mut accepted = 0u32;
                for i in 0..2000u32 {
                    let m =
                        std::format!("P{core} {i:05} {}\n", "x".repeat(((i * 7) % 40) as usize));
                    if R.deposit(m.as_bytes()) == Deposit::Accepted {
                        accepted += 1;
                    }
                }
                accepted
            }));
        }
        let pump = {
            let stop = stop.clone();
            let out = out.clone();
            std::thread::spawn(move || {
                CORE.with(|c| c.set(0));
                while !stop.load(Ordering::Relaxed) || R.pending() > 0 {
                    R.pump(|b| {
                        let n = b.len().min(7);
                        out.lock().unwrap().extend_from_slice(&b[..n]);
                        n
                    });
                }
            })
        };
        let accepted: u32 = handles.into_iter().map(|h| h.join().unwrap()).sum();
        stop.store(true, Ordering::Relaxed);
        pump.join().unwrap();
        let text = std::string::String::from_utf8(out.lock().unwrap().clone()).unwrap();
        let mut lines = 0;
        for l in text.lines() {
            if l.starts_with("WARN") {
                continue;
            }
            let mut it = l.split(' ');
            let who = it.next().unwrap();
            assert!(who == "P0" || who == "P1", "fused or cut line: {l:?}");
            let n: u32 = it.next().unwrap().parse().expect(l);
            let fill = it.next().unwrap_or("");
            assert_eq!(fill.len() as u32, (n * 7) % 40, "{l:?}");
            lines += 1;
        }
        assert_eq!(lines, accepted);
        assert_eq!(accepted + R.lost_total(), 4000);
    }

    #[test]
    fn freeze_returns_the_untransferred_bytes_unchanged() {
        let r = ring::<64>();
        r.deposit(b"one\n");
        r.deposit(b"two\n");
        // the device took part of "one\n"
        r.pump(|b| b.len().min(2));
        assert_eq!(r.claim(), Claim::Winner);
        let f = r.freeze(|| true).ok().unwrap();
        let mut got = Vec::new();
        f.for_each(3, |c| got.extend_from_slice(c));
        assert_eq!(got, b"e\ntwo\n"); // continues the interrupted line
        assert_eq!(f.len(), 6);
    }

    #[test]
    fn freeze_handles_a_wrapped_region() {
        let r = ring::<16>().at_position(u32::MAX - 3);
        assert_eq!(r.deposit(b"abcdefgh\n"), Deposit::Accepted);
        r.claim();
        let f = r.freeze(|| true).ok().unwrap();
        let mut got = Vec::new();
        f.for_each(4, |c| got.extend_from_slice(c));
        assert_eq!(got, b"abcdefgh\n");
    }

    #[test]
    fn a_frozen_ring_refuses_every_actor_and_does_not_change() {
        let r = ring::<64>();
        r.deposit(b"kept\n");
        assert_eq!(r.claim(), Claim::Winner);
        let f = r.freeze(|| true).ok().unwrap();
        assert_eq!(r.deposit(b"late\n"), Deposit::Frozen);
        assert_eq!(r.pump(|b| b.len()), Pump::Frozen);
        let mut got = Vec::new();
        f.for_each(64, |c| got.extend_from_slice(c));
        assert_eq!(got, b"kept\n");
        assert_eq!(r.lost_total(), 0);
    }

    #[test]
    fn an_actor_that_started_before_the_claim_but_has_not_checked_yet_backs_off() {
        // The second check of an actor happens after its `busy` store: a claim that lands in
        // between makes it leave the ring untouched and clear `busy`.
        let r = ring::<64>();
        r.deposit(b"a\n");
        r.emerg.store(1, Relaxed);
        r.platform.with(|| assert!(!r.enter()));
        assert_eq!(r.busy.load(Relaxed), 0);
        assert_eq!(r.pending(), 2);
    }

    #[test]
    fn a_panic_inside_the_ring_on_the_same_core_is_never_replayed() {
        let r = ring::<64>();
        r.deposit(b"x\n");
        r.force_busy(0);
        r.claim();
        assert_eq!(r.freeze(|| true).err(), Some(FreezeError::Own));
    }

    #[test]
    fn an_actor_stuck_on_another_core_times_out() {
        let r = ring::<64>();
        r.force_busy(1);
        r.claim();
        let mut polls = 0;
        let e = r.freeze(|| {
            polls += 1;
            polls > 100
        });
        assert_eq!(e.err(), Some(FreezeError::Timeout));
        assert!(polls > 100);
    }

    #[test]
    fn doubtful_state_is_never_replayed() {
        let r = ring::<64>();
        r.deposit(b"x\n");
        r.corrupt_witness();
        r.claim();
        assert_eq!(r.freeze(|| true).err(), Some(FreezeError::Doubtful));

        let r = ring::<64>();
        r.set_indices(1000, 10); // more than N pending
        r.claim();
        assert_eq!(r.freeze(|| true).err(), Some(FreezeError::Doubtful));
    }

    #[test]
    fn the_second_claimant_loses_and_can_announce_itself() {
        let r = ring::<64>();
        assert_eq!(r.claim(), Claim::Winner);
        assert_eq!(r.claim(), Claim::Loser);
        assert!(!r.second_announced());
        r.announce_second();
        assert!(r.second_announced());
        assert!(!r.fatal_done());
        r.mark_fatal_done();
        assert!(r.fatal_done());
    }

    #[test]
    fn the_replay_plus_what_the_device_took_is_the_whole_stream() {
        let r = ring::<64>();
        let mut taken = Vec::new();
        let mut stream = Vec::new();
        for i in 0..6u8 {
            let m = [b'm', b'0' + i, b'\n'];
            assert_eq!(r.deposit(&m), Deposit::Accepted);
            stream.extend_from_slice(&m);
            if i % 2 == 1 {
                r.pump(|b| {
                    let n = b.len().min(2);
                    taken.extend_from_slice(&b[..n]);
                    n
                });
            }
        }
        r.claim();
        let f = r.freeze(|| true).ok().unwrap();
        f.for_each(5, |c| taken.extend_from_slice(c));
        assert_eq!(taken, stream);
    }
}
