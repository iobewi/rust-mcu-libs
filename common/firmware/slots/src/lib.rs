#![no_std]

//! Logical firmware domains and their independent A/B slot pairs.
//! No flash, ESP partition labels, boot policy or update policy.

/// Optional firmware domain. A-only devices do not need domain B.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Domain {
    A,
    B,
}

/// Logical position within one firmware domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    Zero,
    One,
}

impl Slot {
    pub const fn other(self) -> Self {
        match self {
            Self::Zero => Self::One,
            Self::One => Self::Zero,
        }
    }

    pub const fn index(self) -> u8 {
        match self {
            Self::Zero => 0,
            Self::One => 1,
        }
    }

    pub const fn from_index(index: u8) -> Option<Self> {
        match index {
            0 => Some(Self::Zero),
            1 => Some(Self::One),
            _ => None,
        }
    }
}

/// Identifies one logical firmware image slot: A0, A1, B0 or B1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FirmwareSlot {
    pub domain: Domain,
    pub slot: Slot,
}

impl FirmwareSlot {
    pub const fn new(domain: Domain, slot: Slot) -> Self {
        Self { domain, slot }
    }

    pub const fn other(self) -> Self {
        Self { domain: self.domain, slot: self.slot.other() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_domains_have_independent_pairs() {
        for domain in [Domain::A, Domain::B] {
            for slot in [Slot::Zero, Slot::One] {
                let item = FirmwareSlot::new(domain, slot);
                assert_eq!(item.other().other(), item);
                assert_eq!(item.other().domain, domain);
                assert_ne!(item.other().slot, slot);
                assert_eq!(Slot::from_index(slot.index()), Some(slot));
            }
        }
        assert_eq!(Slot::from_index(2), None);
    }

    #[test]
    fn a_only_requires_no_b_slot() {
        let active = FirmwareSlot::new(Domain::A, Slot::Zero);
        assert_eq!(active.other(), FirmwareSlot::new(Domain::A, Slot::One));
    }
}
