#![no_std]

//! Logical firmware domains and their independent A/B slot pairs.
//! No flash, ESP partition labels, boot policy or update policy.

/// Application-assigned firmware domain identity. A and B retain their IDs.
/// This type has no wire codec: legacy OTM1/OTM2/boot records remain unchanged.
/// Applications may reserve additional IDs (e.g. 2 for C) without another engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Domain(u8);

impl Domain {
    pub const A: Self = Self(0);
    pub const B: Self = Self(1);

    pub const fn new(id: u8) -> Self {
        Self(id)
    }

    pub const fn id(self) -> u8 {
        self.0
    }
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
        Self {
            domain: self.domain,
            slot: self.slot.other(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_domains_have_independent_pairs() {
        for domain in [Domain::A, Domain::B, Domain::new(2)] {
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
