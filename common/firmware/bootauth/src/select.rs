//! `plan_boot` with a boot profile, keeping flash errors apart from refused images.
//!
//! `plan_boot` asks one question per candidate, "is this image bootable?", and records a
//! "no" in `otadata` as `Invalid`. A flash read error must not be recorded as a statement
//! about an image, so the answer is computed here with three outcomes and the plan is
//! thrown away whenever one of them was an I/O error. Nothing in `iobewi-firmware-boot` changes.

use iobewi_firmware_boot::{Plan, Raw, SECTOR_COUNT, plan_boot};
use iobewi_firmware_layout::domain_a::Partition;

use crate::policy::BootPolicy;
use crate::traits::SlotRead;
use crate::verdict::{Refusal, Verdict};

/// The bootloader's own structure check of a slot (image header, segments, ...).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Structure {
    Bootable,
    /// The bytes are not a bootable image (carries the bootloader's own reason code).
    Refused(u32),
    Io,
}

/// Why a slot was not admitted, for the log. Reported once per slot checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Structure(u32),
    Proof(Refusal),
    Io,
}

/// A flash read failed while checking a slot: the plan is not to be applied. Retry after
/// re-initialising the flash, and halt without writing anything if it persists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Interrupted;

pub fn plan<P: BootPolicy, R: SlotRead>(
    entries: [Raw; SECTOR_COUNT],
    slot_count: u8,
    policy: &P,
    reader: &mut R,
    slots: [Partition; 2],
    structure: &mut dyn FnMut(u8) -> Structure,
    report: &mut dyn FnMut(u8, Outcome),
) -> Result<Plan, Interrupted> {
    let mut interrupted = false;
    let mut image_ok = |slot: u8| {
        match structure(slot) {
            Structure::Io => {
                interrupted = true;
                report(slot, Outcome::Io);
                return false;
            }
            Structure::Refused(code) => {
                report(slot, Outcome::Structure(code));
                return false;
            }
            Structure::Bootable => {}
        }
        match policy.admit(reader, slots[usize::from(slot)]) {
            Verdict::Authentic(_) => true,
            Verdict::Refused(refusal) => {
                report(slot, Outcome::Proof(refusal));
                false
            }
            Verdict::Io => {
                interrupted = true;
                report(slot, Outcome::Io);
                false
            }
        }
    };
    let plan = plan_boot(entries, slot_count, &mut image_ok);
    if interrupted {
        Err(Interrupted)
    } else {
        Ok(plan)
    }
}
