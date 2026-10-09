#![no_std]

//! The logical slot model of the A/B firmware layout in use today: two OTA
//! application slots, `ota_0` and `ota_1`, and the layout identifier OTA
//! clients see.
//!
//! Deliberately *only* what exists: no generic slot roles (kernel/userspace/
//! recovery), no per-slot metadata, no partition-table knowledge -- locating
//! a slot in flash is a platform adapter's job, selecting a boot target is
//! `iobewi-firmware-boot`'s.

/// Identifier of the partition layout (the compatibility contract exposed to
/// OTA clients as `partition_layout`): two OTA slots, A/B.
pub const PARTITION_LAYOUT: &str = "embewi-ab-v1";

/// Number of OTA application slots in [`PARTITION_LAYOUT`].
pub const SLOT_COUNT: u8 = 2;

/// OTA-capable application slots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppSlot {
    Ota0,
    Ota1,
}

impl AppSlot {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "ota_0" => Some(Self::Ota0),
            "ota_1" => Some(Self::Ota1),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ota0 => "ota_0",
            Self::Ota1 => "ota_1",
        }
    }

    pub const fn other(self) -> Self {
        match self {
            Self::Ota0 => Self::Ota1,
            Self::Ota1 => Self::Ota0,
        }
    }

    /// 0-based slot index, as used by the EWBT sequence arithmetic
    /// (`iobewi_firmware_boot::slot_of`).
    pub const fn index(self) -> u8 {
        match self {
            Self::Ota0 => 0,
            Self::Ota1 => 1,
        }
    }

    pub const fn from_index(index: u8) -> Option<Self> {
        match index {
            0 => Some(Self::Ota0),
            1 => Some(Self::Ota1),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_the_on_flash_labels() {
        assert_eq!(AppSlot::Ota0.as_str(), "ota_0");
        assert_eq!(AppSlot::Ota1.as_str(), "ota_1");
        assert_eq!(AppSlot::from_name("ota_0"), Some(AppSlot::Ota0));
        assert_eq!(AppSlot::from_name("ota_1"), Some(AppSlot::Ota1));
        assert_eq!(AppSlot::from_name("ota_2"), None);
        assert_eq!(AppSlot::from_name("factory"), None);
        assert_eq!(AppSlot::from_name(""), None);
    }

    #[test]
    fn other_is_an_involution_and_indices_round_trip() {
        for slot in [AppSlot::Ota0, AppSlot::Ota1] {
            assert_ne!(slot.other(), slot);
            assert_eq!(slot.other().other(), slot);
            assert_eq!(AppSlot::from_index(slot.index()), Some(slot));
            assert_eq!(AppSlot::from_name(slot.as_str()), Some(slot));
        }
        assert_eq!(AppSlot::from_index(SLOT_COUNT), None);
    }

    #[test]
    fn the_layout_identifier_is_unchanged() {
        assert_eq!(PARTITION_LAYOUT, "embewi-ab-v1");
    }
}
