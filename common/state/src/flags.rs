use core::fmt;

use crate::State;

/// A set of [`State`]s: one bit per state, in a `u64`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Flags(u64);

impl Flags {
    /// No state.
    pub const EMPTY: Flags = Flags(0);

    const fn bit(state: State) -> u64 {
        1 << (state as u8)
    }

    /// The set holding exactly `states`.
    pub const fn of(states: &[State]) -> Flags {
        let mut bits = 0;
        let mut i = 0;
        while i < states.len() {
            bits |= Flags::bit(states[i]);
            i += 1;
        }
        Flags(bits)
    }

    /// Whether `state` is in the set.
    pub const fn contains(self, state: State) -> bool {
        self.0 & Flags::bit(state) != 0
    }

    /// The set plus `state`.
    #[must_use]
    pub const fn with(self, state: State) -> Flags {
        Flags(self.0 | Flags::bit(state))
    }

    /// The set minus `state`.
    #[must_use]
    pub const fn without(self, state: State) -> Flags {
        Flags(self.0 & !Flags::bit(state))
    }

    /// The states in `self` or in `other`.
    #[must_use]
    pub const fn union(self, other: Flags) -> Flags {
        Flags(self.0 | other.0)
    }

    /// The states in `self` and not in `other`.
    #[must_use]
    pub const fn difference(self, other: Flags) -> Flags {
        Flags(self.0 & !other.0)
    }

    /// Whether no state is in the set.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The number of states in the set.
    pub const fn len(self) -> u32 {
        self.0.count_ones()
    }

    /// The bits, bit `s as u8` being state `s`. Stable, since discriminants never change.
    pub const fn bits(self) -> u64 {
        self.0
    }

    /// The states in the set, in discriminant order.
    pub fn iter(self) -> impl Iterator<Item = State> {
        State::ALL.into_iter().filter(move |s| self.contains(*s))
    }
}

impl From<State> for Flags {
    fn from(state: State) -> Flags {
        Flags(Flags::bit(state))
    }
}

impl fmt::Debug for Flags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.iter()).finish()
    }
}
