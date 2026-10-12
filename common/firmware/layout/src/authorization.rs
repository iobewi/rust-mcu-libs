//! Fail-closed authorization boundary for artifact writes.
//! The caller MUST derive `running` from a platform-verified running image
//! (A) or running workload supervisor (B), independently of update metadata.
//! A validated layout alone is not write authorization.
use crate::Region;
use iobewi_firmware_slots::{Domain, FirmwareSlot, Slot};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorizationError {
    UnknownRunning,
    WrongDomain,
    ActiveTarget,
    InvalidLayout,
}

/// Only a checked inactive partition can be passed to the physical writer.
/// Private fields prevent manufacturing this from an arbitrary flash offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InactiveWriteTarget {
    slot: FirmwareSlot,
    region: Region,
}

impl InactiveWriteTarget {
    pub const fn slot(&self) -> FirmwareSlot {
        self.slot
    }
    pub const fn region(&self) -> Region {
        self.region
    }
}

/// Turn independently observed execution state and a validated two-slot layout
/// into a write target. This does not observe hardware or read OTM2/otadata.
/// Those checks belong to the appropriate target/supervisor adapter.
pub fn authorize(
    domain: Domain,
    images: [Region; 2],
    running: Option<FirmwareSlot>,
    requested: FirmwareSlot,
) -> Result<InactiveWriteTarget, AuthorizationError> {
    let active = running.ok_or(AuthorizationError::UnknownRunning)?;
    if active.domain != domain || requested.domain != domain {
        return Err(AuthorizationError::WrongDomain);
    }
    if active.slot == requested.slot {
        return Err(AuthorizationError::ActiveTarget);
    }
    let a = images[0];
    let b = images[1];
    let a_end = (a.offset as u64) + (a.size as u64);
    let b_end = (b.offset as u64) + (b.size as u64);
    if a.size == 0 || b.size == 0 || (a.offset as u64) < b_end && (b.offset as u64) < a_end {
        return Err(AuthorizationError::InvalidLayout);
    }
    let region = images[match requested.slot {
        Slot::Zero => 0,
        Slot::One => 1,
    }];
    Ok(InactiveWriteTarget {
        slot: requested,
        region,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn slots() -> [Region; 2] {
        [
            Region {
                offset: 0x20000,
                size: 0x10000,
            },
            Region {
                offset: 0x30000,
                size: 0x10000,
            },
        ]
    }
    fn id(domain: Domain, slot: Slot) -> FirmwareSlot {
        FirmwareSlot::new(domain, slot)
    }
    #[test]
    fn unknown_or_active_slot_cannot_get_write_permission() {
        let target = id(Domain::A, Slot::One);
        assert_eq!(
            authorize(Domain::A, slots(), None, target),
            Err(AuthorizationError::UnknownRunning)
        );
        assert_eq!(
            authorize(Domain::A, slots(), Some(target), target),
            Err(AuthorizationError::ActiveTarget)
        );
        assert_eq!(
            authorize(Domain::A, slots(), Some(id(Domain::B, Slot::Zero)), target),
            Err(AuthorizationError::WrongDomain)
        );
    }
    #[test]
    fn only_other_slot_of_same_domain_is_authorized() {
        let permit = authorize(
            Domain::A,
            slots(),
            Some(id(Domain::A, Slot::Zero)),
            id(Domain::A, Slot::One),
        )
        .unwrap();
        assert_eq!(permit.region(), slots()[1]);
        assert_eq!(
            authorize(
                Domain::B,
                slots(),
                Some(id(Domain::B, Slot::Zero)),
                id(Domain::B, Slot::One)
            )
            .unwrap()
            .slot()
            .domain,
            Domain::B
        );
    }
    #[test]
    fn refuses_overlapping_regions() {
        let mut regions = slots();
        regions[1].offset = 0x28000;
        assert_eq!(
            authorize(
                Domain::A,
                regions,
                Some(id(Domain::A, Slot::Zero)),
                id(Domain::A, Slot::One)
            ),
            Err(AuthorizationError::InvalidLayout)
        );
    }
}
