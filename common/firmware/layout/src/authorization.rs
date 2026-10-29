//! Fail-closed software authorization boundary for artifact writes.
//! Runtime slot identity MUST be independently observed by the platform.
//! Domain A: running application identity, NOT merely selected OTA metadata.
//! Domain B: supervisor identity, NOT ESP otadata.
//! An authorization token is not a hardware-backed attestation.
use crate::{Region, domain_a, domain_b};
use iobewi_firmware_slots::{Domain, FirmwareSlot, Slot};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorizationError {
    ActiveTarget,
    InvalidLayout,
}

/// Bounded write authorization. Deliberately !Copy and !Clone.
/// Writers borrow this value exclusively; it is not a hardware attestation.
/// Use a session guard to exclude activation for the entire transaction.
#[derive(Debug, PartialEq, Eq)]
pub struct InactiveWriteTarget {
    slot: FirmwareSlot,
    region: Region,
    proof: Option<Region>,
}

impl InactiveWriteTarget {
    pub const fn slot(&self) -> FirmwareSlot {
        self.slot
    }
    pub const fn region(&self) -> Region {
        self.region
    }

    /// The slot's boot-proof sector (domain A): outside [`region`](Self::region), erased
    /// before the image is rewritten and written once the image is complete.
    pub const fn proof_sector(&self) -> Option<Region> {
        self.proof
    }
}

/// A physically checked pair plus its protected metadata region.
/// No boot protocol or workload-specific minimum is imposed here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotLayout {
    domain: Domain,
    images: [Region; 2],
    proofs: [Option<Region>; 2],
}

impl SlotLayout {
    pub fn new(
        domain: Domain,
        images: [Region; 2],
        metadata: Region,
        flash_size: u32,
        erase_size: u32,
    ) -> Result<Self, AuthorizationError> {
        crate::validate_regions(&[images[0], images[1], metadata], flash_size, erase_size)
            .map_err(|_| AuthorizationError::InvalidLayout)?;
        Ok(Self {
            domain,
            images,
            proofs: [None; 2],
        })
    }

    pub fn for_a(layout: &domain_a::Layout, flash_size: u32) -> Result<Self, AuthorizationError> {
        layout
            .validate(flash_size)
            .map_err(|_| AuthorizationError::InvalidLayout)?;
        let mut slots = Self::new(
            Domain::A,
            // The writable region of a slot excludes its proof sector.
            layout.images.map(|p| Region {
                offset: p.offset,
                size: p.usable_size() as usize,
            }),
            Region {
                offset: layout.otadata.offset,
                size: layout.otadata.size as usize,
            },
            flash_size,
            0x1000,
        )?;
        slots.proofs = layout.images.map(|p| {
            Some(Region {
                offset: p.proof_offset(),
                size: domain_a::PROOF_SECTOR_SIZE as usize,
            })
        });
        Ok(slots)
    }

    pub fn for_b(
        layout: &domain_b::WorkloadLayout,
        flash_size: u32,
    ) -> Result<Self, AuthorizationError> {
        let [meta, a, b] = layout.regions();
        Self::new(
            Domain::B,
            [a, b].map(|r| Region {
                offset: r.offset,
                size: r.size as usize,
            }),
            Region {
                offset: meta.offset,
                size: meta.size as usize,
            },
            flash_size,
            layout.erase_size(),
        )
    }

    pub const fn domain(&self) -> Domain {
        self.domain
    }

    pub(crate) fn authorize(
        &self,
        active: Option<Slot>,
        requested: Slot,
    ) -> Result<InactiveWriteTarget, AuthorizationError> {
        if active == Some(requested) {
            return Err(AuthorizationError::ActiveTarget);
        }
        Ok(InactiveWriteTarget {
            slot: FirmwareSlot::new(self.domain, requested),
            region: self.images[usize::from(requested.index())],
            proof: self.proofs[usize::from(requested.index())],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_capacity_geometry_and_metadata_before_authorization() {
        let images = [
            Region {
                offset: 0x20000,
                size: 0x40000,
            },
            Region {
                offset: 0x60000,
                size: 0x40000,
            },
        ];
        let meta = Region {
            offset: 0x9000,
            size: 0x2000,
        };
        assert!(SlotLayout::new(Domain::new(2), images, meta, 0x1000000, 0x1000).is_ok());
        assert_eq!(
            SlotLayout::new(Domain::B, images, meta, 0x90000, 0x1000),
            Err(AuthorizationError::InvalidLayout)
        );
        assert_eq!(
            SlotLayout::new(Domain::B, images, images[0], 0x1000000, 0x1000),
            Err(AuthorizationError::InvalidLayout)
        );
        assert_eq!(
            SlotLayout::new(Domain::B, images, meta, 0x1000000, 0),
            Err(AuthorizationError::InvalidLayout)
        );
    }

    #[test]
    fn a_domain_a_write_region_never_reaches_the_proof_sector() {
        let layout = domain_a::Layout {
            images: [
                domain_a::Partition {
                    offset: 0x20000,
                    size: 0x1b0000,
                },
                domain_a::Partition {
                    offset: 0x1d0000,
                    size: 0x1b0000,
                },
            ],
            otadata: domain_a::Partition {
                offset: 0xd000,
                size: 0x2000,
            },
        };
        let slots = SlotLayout::for_a(&layout, 0x1000000).unwrap();
        for (slot, partition) in [Slot::Zero, Slot::One].into_iter().zip(layout.images) {
            let other = if slot == Slot::Zero {
                Slot::One
            } else {
                Slot::Zero
            };
            let target = slots.authorize(Some(other), slot).unwrap();
            let region = target.region();
            assert_eq!(region.offset, partition.offset);
            assert_eq!(
                region.size as u32,
                partition.size - domain_a::PROOF_SECTOR_SIZE
            );
            assert!(region.offset + region.size as u32 <= partition.proof_offset());
            // The proof sector is the slot's last sector, and only a domain A token has one.
            let proof = target.proof_sector().unwrap();
            assert_eq!(proof.offset, partition.proof_offset());
            assert_eq!(proof.size as u32, domain_a::PROOF_SECTOR_SIZE);
            assert_eq!(
                proof.offset + proof.size as u32,
                partition.offset + partition.size
            );
        }
    }
}
