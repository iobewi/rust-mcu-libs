use core::cell::RefCell;
use core::future::poll_fn;
use core::task::Poll;

use embassy_sync::blocking_mutex::{Mutex, raw::RawMutex};
use embassy_sync::waitqueue::MultiWakerRegistration;

use crate::{Flags, State};

/// `register` found no free slot: every slot is held by a [`Source`], or retired for good (see
/// [`SourceId`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Full;

/// Identifies one source, as long as the source exists.
///
/// It is a slot index and the generation of that slot at registration. A slot freed by the
/// [`Drop`] of its [`Source`] gets the next generation, so an old identifier never designates the
/// new owner of the slot: [`Snapshot::of`] returns `None` for it and [`Snapshot::changes_since`]
/// tells the two apart.
///
/// The generation is a `u32` and never wraps. A slot whose generation reaches `u32::MAX` is
/// **retired** when its owner goes away: it is never handed out again. The registry then has one
/// slot less (4 billion registrations of a slot later); when every slot is retired, `register`
/// returns [`Full`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SourceId {
    slot: u8,
    generation: u32,
}

impl SourceId {
    /// The slot, from 0 up to the registry capacity.
    pub const fn slot(self) -> usize {
        self.slot as usize
    }

    /// How many owners the slot had before this one.
    pub const fn generation(self) -> u32 {
        self.generation
    }
}

/// What happened to a source between two snapshots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    /// The source is in the later snapshot and not in the earlier one. `raised` is what it
    /// already holds, possibly nothing.
    Registered,
    /// The source is in both snapshots and its flags differ.
    Updated,
    /// The source is in the earlier snapshot and not in the later one. `cleared` is what it held.
    Released,
}

/// One difference between two snapshots, for one source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Change {
    /// The source concerned, with its generation: a reused slot gives a [`ChangeKind::Released`]
    /// of the old identifier and a [`ChangeKind::Registered`] of the new one.
    pub source: SourceId,
    pub kind: ChangeKind,
    /// Flags the source holds now and held not before.
    pub raised: Flags,
    /// Flags the source held before and holds no more.
    pub cleared: Flags,
}

/// The registry at one revision: the flags of every source, read under the lock, so they are
/// consistent with each other.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot<const S: usize> {
    revision: u64,
    flags: [Flags; S],
    generations: [u32; S],
    occupied: u32,
}

impl<const S: usize> Snapshot<S> {
    /// The registry before anything happened: revision 0, no source. A registry that has not
    /// changed yet gives a snapshot equal to this one.
    pub const EMPTY: Self = Self {
        revision: 0,
        flags: [Flags::EMPTY; S],
        generations: [0; S],
        occupied: 0,
    };

    /// The revision of the registry when the snapshot was taken. It grows by one at every real
    /// change.
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// The union of the flags of every source.
    pub fn aggregate(&self) -> Flags {
        self.sources()
            .fold(Flags::EMPTY, |all, (_, flags)| all.union(flags))
    }

    /// The flags of the source `id`, or `None` if it is not there: released, or its slot reused by
    /// another source.
    pub fn of(&self, id: SourceId) -> Option<Flags> {
        match self.entry(id.slot()) {
            Some((found, flags)) if found == id => Some(flags),
            _ => None,
        }
    }

    /// Every source of the snapshot, by slot.
    pub fn sources(&self) -> impl Iterator<Item = (SourceId, Flags)> + '_ {
        (0..S).filter_map(|slot| self.entry(slot))
    }

    /// What differs from `previous` (an earlier snapshot of the same registry), source by source.
    /// A source is the same only if slot and generation are: a reused slot is reported as the old
    /// source released then the new one registered. Sources that did not change are not listed.
    ///
    /// It compares two snapshots and keeps no history.
    pub fn changes_since<'a>(&'a self, previous: &'a Self) -> impl Iterator<Item = Change> + 'a {
        Changes {
            current: self,
            previous,
            next: 0,
            pending: None,
        }
    }

    fn entry(&self, slot: usize) -> Option<(SourceId, Flags)> {
        (slot < S && self.occupied & (1 << slot) != 0).then(|| {
            let id = SourceId {
                slot: slot as u8,
                generation: self.generations[slot],
            };
            (id, self.flags[slot])
        })
    }
}

struct Changes<'a, const S: usize> {
    current: &'a Snapshot<S>,
    previous: &'a Snapshot<S>,
    next: usize,
    pending: Option<Change>,
}

impl<const S: usize> Iterator for Changes<'_, S> {
    type Item = Change;

    fn next(&mut self) -> Option<Change> {
        loop {
            if let Some(change) = self.pending.take() {
                return Some(change);
            }
            if self.next >= S {
                return None;
            }
            let slot = self.next;
            self.next += 1;
            match (self.previous.entry(slot), self.current.entry(slot)) {
                (None, None) => {}
                (Some((before, was)), Some((now, is))) if before == now => {
                    if was != is {
                        return Some(Change {
                            source: now,
                            kind: ChangeKind::Updated,
                            raised: is.difference(was),
                            cleared: was.difference(is),
                        });
                    }
                }
                (before, now) => {
                    let released = before.map(|(source, was)| Change {
                        source,
                        kind: ChangeKind::Released,
                        raised: Flags::EMPTY,
                        cleared: was,
                    });
                    let registered = now.map(|(source, is)| Change {
                        source,
                        kind: ChangeKind::Registered,
                        raised: is,
                        cleared: Flags::EMPTY,
                    });
                    if released.is_some() {
                        self.pending = registered;
                        return released;
                    }
                    if registered.is_some() {
                        return registered;
                    }
                }
            }
        }
    }
}

/// Everything the lock protects.
struct Inner<const S: usize, const R: usize> {
    revision: u64,
    flags: [Flags; S],
    /// Generation the slot has now: the one of its owner if occupied, the next one's if not.
    generations: [u32; S],
    occupied: u32,
    /// Slots that reached the last generation and left service.
    retired: u32,
    observers: usize,
    wakers: MultiWakerRegistration<R>,
}

impl<const S: usize, const R: usize> Inner<S, R> {
    const SLOTS: u32 = if S == 32 { u32::MAX } else { (1 << S) - 1 };

    const fn new() -> Self {
        Self {
            revision: 0,
            flags: [Flags::EMPTY; S],
            generations: [0; S],
            occupied: 0,
            retired: 0,
            observers: 0,
            wakers: MultiWakerRegistration::new(),
        }
    }

    /// A real change happened: new revision, and every waiting observer is woken. The wakers run
    /// under the lock (as in `embassy_sync::watch::Watch`): they must only schedule a task.
    fn touch(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.wakers.wake();
    }

    fn set_flags(&mut self, slot: usize, flags: Flags) -> bool {
        if self.flags[slot] == flags {
            return false;
        }
        self.flags[slot] = flags;
        self.touch();
        true
    }

    fn register(&mut self) -> Result<SourceId, Full> {
        let free = !(self.occupied | self.retired) & Self::SLOTS;
        if free == 0 {
            return Err(Full);
        }
        let slot = free.trailing_zeros() as usize;
        self.occupied |= 1 << slot;
        self.touch();
        Ok(SourceId {
            slot: slot as u8,
            generation: self.generations[slot],
        })
    }

    fn release(&mut self, slot: usize) {
        self.flags[slot] = Flags::EMPTY;
        self.occupied &= !(1 << slot);
        match self.generations[slot].checked_add(1) {
            Some(next) => self.generations[slot] = next,
            None => self.retired |= 1 << slot,
        }
        self.touch();
    }

    fn snapshot(&self) -> Snapshot<S> {
        Snapshot {
            revision: self.revision,
            flags: self.flags,
            generations: self.generations,
            occupied: self.occupied,
        }
    }
}

/// A registry of state flags with room for `S` sources and `R` observers.
///
/// Sources [`register`](Registry::register) and raise or clear their own flags. The registry keeps
/// each source's flags apart, so a source clearing a flag does not touch the same flag held by
/// another. Observers learn that something changed and read a consistent [`Snapshot`], from which
/// they take the aggregate or the flags of one source.
///
/// It decides nothing: it keeps what the sources say and tells the observers.
///
/// # Notification contract
///
/// Every *real* change of the registry (a source registered or released, a source's flags
/// different) makes the **revision** grow by one and wakes the waiting observers. An operation
/// that changes nothing does neither. A change whose aggregate is unchanged is still a change: an
/// observer is told when a source drops a flag another source also holds.
///
/// An observer receives the current snapshot first, then the latest one after each change.
/// **Intermediate changes may be merged**: an observer that was slow, or was not polled, sees one
/// snapshot for several changes, never a state the registry did not have, and never misses the
/// last state. [`Snapshot::changes_since`] gives what differs between two snapshots.
///
/// # Locking
///
/// One lock, `Mutex<M, _>`, protects the table, the revision and the wakers. A change, a snapshot
/// and the registration of a waiting observer are each one critical section: that is why no wake-up
/// is lost between reading a snapshot and waiting for the next change. The critical sections are
/// short: a change is a few stores plus the wake-ups (at most `R`), a snapshot copies about
/// `12 * S + 16` bytes. The lock is never held across an `.await`, and no code of this crate takes
/// it twice.
///
/// `M` must suit every context that uses the registry:
///
/// - `CriticalSectionRawMutex`: tasks of several executors and interrupts, including both cores of
///   an ESP32-S3.
/// - `NoopRawMutex`, `ThreadModeRawMutex`: one executor, no interrupt using the registry.
///
/// A [`Waker`](core::task::Waker) woken by the registry must not call the registry back
/// synchronously (it would find the lock taken). Executor wakers only schedule a task.
///
/// # Capacity
///
/// `S` is at most 32 and `R` at least 1, both checked at compile time. Wakers: up to `R`
/// observers can wait; an observer polled from several tasks over time can leave a stale waker
/// behind, and past `R` distinct wakers the registry wakes all of them and starts over. Those
/// extra wake-ups are spurious (the observer finds no change and waits again), never a loss.
pub struct Registry<M: RawMutex, const S: usize, const R: usize> {
    inner: Mutex<M, RefCell<Inner<S, R>>>,
}

impl<M: RawMutex, const S: usize, const R: usize> Registry<M, S, R> {
    const VALID: () = {
        assert!(S >= 1 && S <= 32, "a registry holds 1 to 32 sources");
        assert!(R >= 1, "a registry needs room for at least one observer");
    };

    /// An empty registry, at revision 0. It can be a `static`.
    pub const fn new() -> Self {
        let () = Self::VALID;
        Self {
            inner: Mutex::new(RefCell::new(Inner::new())),
        }
    }

    fn with<T>(&self, f: impl FnOnce(&mut Inner<S, R>) -> T) -> T {
        self.inner.lock(|cell| f(&mut cell.borrow_mut()))
    }

    /// Registers a source. The registry changes (a new revision) even though the source holds no
    /// flag yet.
    ///
    /// # Errors
    ///
    /// [`Full`] when no slot is free.
    pub fn register(&self) -> Result<Source<'_, M, S, R>, Full> {
        let id = self.with(Inner::register)?;
        Ok(Source { registry: self, id })
    }

    /// The registry as it is now.
    pub fn snapshot(&self) -> Snapshot<S> {
        self.with(|inner| inner.snapshot())
    }

    /// A new observer, or `None` if `R` observers exist. It sees the current state first.
    pub fn observer(&self) -> Option<Observer<'_, M, S, R>> {
        self.with(|inner| {
            (inner.observers < R).then(|| {
                inner.observers += 1;
                Observer {
                    registry: self,
                    last: None,
                }
            })
        })
    }
}

impl<M: RawMutex, const S: usize, const R: usize> Default for Registry<M, S, R> {
    fn default() -> Self {
        Self::new()
    }
}

/// The handle of one source: it raises and clears that source's flags, and nobody else's.
///
/// It cannot be cloned. When it is dropped the source's flags are removed, its slot is freed and
/// the observers are told (a new revision, even if the source held nothing).
pub struct Source<'a, M: RawMutex, const S: usize, const R: usize> {
    registry: &'a Registry<M, S, R>,
    id: SourceId,
}

impl<M: RawMutex, const S: usize, const R: usize> Source<'_, M, S, R> {
    /// The identifier of this source, for [`Snapshot::of`] and [`Change::source`].
    pub const fn id(&self) -> SourceId {
        self.id
    }

    /// Raises `state` for this source. Returns `true` if it was not raised already; nothing
    /// happens (no revision, no wake-up) otherwise.
    pub fn raise(&self, state: State) -> bool {
        let slot = self.id.slot();
        self.registry
            .with(|inner| inner.set_flags(slot, inner.flags[slot].with(state)))
    }

    /// Clears `state` for this source. Returns `true` if it was raised; nothing happens otherwise.
    /// A flag another source holds stays in the aggregate.
    pub fn clear(&self, state: State) -> bool {
        let slot = self.id.slot();
        self.registry
            .with(|inner| inner.set_flags(slot, inner.flags[slot].without(state)))
    }

    /// Makes `flags` the whole set of this source, in one step: observers see the old set or the
    /// new one, never a mixture, and the revision grows once. Returns `true` if the set changed.
    pub fn replace(&self, flags: Flags) -> bool {
        let slot = self.id.slot();
        self.registry.with(|inner| inner.set_flags(slot, flags))
    }

    /// The flags of this source.
    pub fn flags(&self) -> Flags {
        let slot = self.id.slot();
        self.registry.with(|inner| inner.flags[slot])
    }

    /// Whether this source holds `state`.
    pub fn is_set(&self, state: State) -> bool {
        self.flags().contains(state)
    }
}

impl<M: RawMutex, const S: usize, const R: usize> Drop for Source<'_, M, S, R> {
    fn drop(&mut self) {
        let slot = self.id.slot();
        self.registry.with(|inner| inner.release(slot));
    }
}

/// Follows a registry. Each observer has its own position, so observers never take notifications
/// from one another.
///
/// Dropping the observer gives its place back.
pub struct Observer<'a, M: RawMutex, const S: usize, const R: usize> {
    registry: &'a Registry<M, S, R>,
    /// Revision of the last snapshot handed out.
    last: Option<u64>,
}

impl<M: RawMutex, const S: usize, const R: usize> Observer<'_, M, S, R> {
    /// The registry as it is now, whether or not it changed, and counts it as seen.
    pub fn get(&mut self) -> Snapshot<S> {
        let last = &mut self.last;
        self.registry.with(|inner| {
            *last = Some(inner.revision);
            inner.snapshot()
        })
    }

    /// The registry as it is now if it differs from the last snapshot this observer got. The first
    /// call always returns the current state.
    pub fn try_changed(&mut self) -> Option<Snapshot<S>> {
        let last = &mut self.last;
        self.registry.with(|inner| {
            (*last != Some(inner.revision)).then(|| {
                *last = Some(inner.revision);
                inner.snapshot()
            })
        })
    }

    /// Waits until the registry differs from the last snapshot this observer got, and returns it.
    /// The first call returns the current state at once.
    ///
    /// Checking the revision and registering the wake-up are one critical section, and so is every
    /// change with its wake-up: a change cannot slip between the two. Dropping the future loses
    /// nothing, the next call still sees the change.
    pub async fn changed(&mut self) -> Snapshot<S> {
        let Self { registry, last } = self;
        poll_fn(|cx| {
            registry.with(|inner| {
                if *last != Some(inner.revision) {
                    *last = Some(inner.revision);
                    Poll::Ready(inner.snapshot())
                } else {
                    inner.wakers.register(cx.waker());
                    Poll::Pending
                }
            })
        })
        .await
    }
}

impl<M: RawMutex, const S: usize, const R: usize> Drop for Observer<'_, M, S, R> {
    fn drop(&mut self) {
        self.registry.with(|inner| inner.observers -= 1);
    }
}

#[cfg(test)]
impl<M: RawMutex, const S: usize, const R: usize> Registry<M, S, R> {
    /// Test hook: pretend `slot` has already been used `generation` times.
    pub(crate) fn set_generation(&self, slot: usize, generation: u32) {
        self.with(|inner| inner.generations[slot] = generation);
    }
}
