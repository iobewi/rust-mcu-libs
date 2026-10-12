//! Fail-closed software authorization boundary for artifact writes.
//! Runtime slot identity MUST be independently observed by the platform.
//! Domain A: running application identity, NOT merely selected OTA metadata.
//! Domain B: supervisor identity, NOT ESP otadata.
//! An authorization token is not a hardware-backed attestation.
use crate::{domain_a, domain_b, Region};
use iobewi_firmware_slots::{Domain, FirmwareSlot, Slot};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorizationError {
    UnknownRunning,
    WrongDomain,
    ActiveTarget,
    InvalidLayout,
}

/// The only target accepted by the ESP artifact writer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InactiveWriteTarget {
    slot: FirmwareSlot,
    region: Region,
}

impl InactiveWriteTarget {
    pub const fn slot(&self) -> FirmwareSlot { self.slot }
    pub const fn region(&self) -> Region { self.region }
}

/// Authorize against an A-domain layout returned by the platform's partition
/// discovery. The caller is responsible for establishing that provenance.
pub fn authorize_a(
    layout: &domain_a::Layout,
    running: Option<FirmwareSlot>,
    requested: FirmwareSlot,
) -> Result<InactiveWriteTarget, AuthorizationError> {
    let images = layout.images.map(|p| Region { offset: p.offset, size: p.size as usize });
    let metadata = Region { offset: layout.otadata.offset, size: layout.otadata.size as usize };
    if !valid_pair(images) || !valid_region(metadata) ||
        images.iter().any(|p| overlaps(*p, metadata)) ||
        layout.otadata.size < domain_a::OTADATA_MIN_SIZE ||
        images.iter().any(|p| p.offset % 0x10000 != 0) {
        return Err(AuthorizationError::InvalidLayout);
    }
    authorize_checked(Domain::A, images, running, requested)
}

/// B uses the already assembled workload layout. B is NOT a second ESP OTA
/// bootloader; running identity must come from the workload supervisor.
pub fn authorize_b(
    layout: &domain_b::WorkloadLayout,
    running: Option<FirmwareSlot>,
    requested: FirmwareSlot,
) -> Result<InactiveWriteTarget, AuthorizationError> {
    let a = layout.slot(domain_b::Side::A);
    let b = layout.slot(domain_b::Side::B);
    let images = [a, b].map(|p| Region { offset: p.offset, size: p.size as usize });
    if !valid_pair(images) { return Err(AuthorizationError::InvalidLayout); }
    authorize_checked(Domain::B, images, running, requested)
}

fn valid_region(region: Region) -> bool {
    region.size != 0 &&
        (region.offset as u64).checked_add(region.size as u64).is_some_and(|end| end <= u32::MAX as u64)
}
fn overlaps(a: Region, b: Region) -> bool {
    (a.offset as u64) < b.offset as u64 + b.size as u64 &&
    (b.offset as u64) < a.offset as u64 + a.size as u64
}
fn valid_pair(images: [Region; 2]) -> bool {
    images.iter().copied().all(valid_region) && !overlaps(images[0], images[1])
}
fn authorize_checked(
    domain: Domain, images: [Region; 2],
    running: Option<FirmwareSlot>, requested: FirmwareSlot,
) -> Result<InactiveWriteTarget, AuthorizationError> {
    let active = running.ok_or(AuthorizationError::UnknownRunning)?;
    if active.domain != domain || requested.domain != domain {
        return Err(AuthorizationError::WrongDomain);
    }
    if active.slot == requested.slot { return Err(AuthorizationError::ActiveTarget); }
    if !valid_pair(images) { return Err(AuthorizationError::InvalidLayout); }
    let region = images[match requested.slot { Slot::Zero => 0, Slot::One => 1 }];
    Ok(InactiveWriteTarget { slot: requested, region })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn id(domain: Domain, slot: Slot) -> FirmwareSlot { FirmwareSlot::new(domain, slot) }
    fn a() -> domain_a::Layout {
        domain_a::Layout {
            images: [
                domain_a::Partition { offset: 0x20000, size: 0x20000 },
                domain_a::Partition { offset: 0x40000, size: 0x20000 },
            ],
            otadata: domain_a::Partition { offset: 0xD000, size: 0x2000 },
        }
    }
    fn b() -> domain_b::WorkloadLayout {
        domain_b::assemble(
            Some(domain_b::Region::new(0x9000, 0x2000)),
            Some(domain_b::Region::new(0x20000, 0x40000)),
            Some(domain_b::Region::new(0x60000, 0x40000)), 0x1000,
        ).unwrap()
    }
    #[test]
    fn fail_closed_when_running_unknown_active_or_wrong_domain() {
        let target = id(Domain::A, Slot::One);
        assert_eq!(authorize_a(&a(), None, target), Err(AuthorizationError::UnknownRunning));
        assert_eq!(authorize_a(&a(), Some(target), target), Err(AuthorizationError::ActiveTarget));
        assert_eq!(authorize_a(&a(), Some(id(Domain::B, Slot::Zero)), target),
                   Err(AuthorizationError::WrongDomain));
    }
    #[test]
    fn checked_a_and_b_select_only_the_other_slot() {
        let a_permit = authorize_a(&a(), Some(id(Domain::A, Slot::Zero)),
                                   id(Domain::A, Slot::One)).unwrap();
        assert_eq!(a_permit.region().offset, 0x40000);
        let b_permit = authorize_b(&b(), Some(id(Domain::B, Slot::Zero)),
                                   id(Domain::B, Slot::One)).unwrap();
        assert_eq!(b_permit.region().offset, 0x60000);
    }
    #[test]
    fn rejects_bad_a_metadata_overlap_and_alignment() {
        let mut layout = a();
        layout.otadata.offset = 0x20000;
        assert_eq!(authorize_a(&layout, Some(id(Domain::A, Slot::Zero)),
                               id(Domain::A, Slot::One)), Err(AuthorizationError::InvalidLayout));
        let mut layout = a();
        layout.images[1].offset = 0x41000;
        assert_eq!(authorize_a(&layout, Some(id(Domain::A, Slot::Zero)),
                               id(Domain::A, Slot::One)), Err(AuthorizationError::InvalidLayout));
    }
}
