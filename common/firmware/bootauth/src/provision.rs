//! Keeping a slot's boot proof in step with its image across an OTA.
//!
//! Three operations, placed on the transitions the OTA already has; no new state is kept
//! because the proof sector itself is the state:
//!
//! 1. [`invalidate`] -- before the first byte of the inactive slot's image is rewritten. From
//!    then on the slot has no valid proof, so a power cut at any point of the rewrite leaves a
//!    slot a verified bootloader refuses.
//! 2. [`publish`] -- once the image is complete and its digest checked, and *before* the OTA
//!    metadata publishes it: write the proof (commit word last), then read it back and have
//!    it judged by the very check the boot stage applies. A proof that does not pass is never
//!    left looking valid.
//! 3. [`gate`] -- at activation, before the boot entry is written: the same boot check, so a
//!    slot whose proof is absent, stale, partial or foreign cannot be made bootable.
//!
//! Only the inactive slot is ever touched; the confirmed slot's image and proof are not
//! reachable from here (the caller's write permit does not cover them).

use iobewi_firmware_auth::Algorithm;
use iobewi_firmware_boot::transaction::Flash;
use iobewi_firmware_layout::domain_a::Partition;

use crate::iap1;
use crate::policy::BootPolicy;
use crate::traits::SlotRead;
use crate::verdict::{Refusal, Verdict};
use crate::writer;

/// Erase the slot's proof sector: the slot no longer has a valid proof.
pub fn invalidate<F: Flash>(flash: &mut F, slot: Partition) -> Result<(), F::Error> {
    flash.erase(slot.proof_offset(), iap1::SECTOR as u32)
}

/// Why a proof could not be published.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishError<E> {
    /// The envelope or the key does not have the length the algorithm fixes.
    Length,
    Write(writer::Error<E>),
    /// The proof was written and read back, but the boot check does not accept it.
    NotAuthentic(Refusal),
    /// The boot check could not read the flash.
    Io,
    /// The proof is authentic but signs another size than the image just received.
    WrongSize,
}

/// Write the proof for `envelope` / `key`, then run the boot check on the slot. `image_size`
/// is the size of the image that was received: the proof must sign exactly that.
pub fn publish<F: Flash + SlotRead, P: BootPolicy>(
    flash: &mut F,
    slot: Partition,
    algorithm: Algorithm,
    envelope: &[u8],
    key: &[u8],
    policy: &P,
    image_size: u32,
) -> Result<(), PublishError<<F as Flash>::Error>> {
    let mut scratch = [0u8; iap1::MAX_PAYLOAD];
    let write =
        iap1::Write::new(algorithm, envelope, key, &mut scratch).ok_or(PublishError::Length)?;
    writer::execute(flash, slot, &write).map_err(PublishError::Write)?;
    match gate(flash, slot, policy, image_size) {
        Ok(()) => Ok(()),
        Err(GateError::Refused(refusal)) => Err(PublishError::NotAuthentic(refusal)),
        Err(GateError::Io) => Err(PublishError::Io),
        Err(GateError::WrongSize) => Err(PublishError::WrongSize),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateError {
    Refused(Refusal),
    /// The flash could not be read: nothing is known about the slot, it is not activated.
    Io,
    WrongSize,
}

/// The boot stage's own check of the slot, applied before the slot may be activated.
/// `image_size` is the size the OTA metadata publishes for the image.
pub fn gate<R: SlotRead, P: BootPolicy>(
    reader: &mut R,
    slot: Partition,
    policy: &P,
    image_size: u32,
) -> Result<(), GateError> {
    match policy.admit(reader, slot) {
        Verdict::Refused(refusal) => Err(GateError::Refused(refusal)),
        Verdict::Io => Err(GateError::Io),
        Verdict::Authentic(authentic) => match authentic.signed_size() {
            Some(size) if size != image_size => Err(GateError::WrongSize),
            _ => Ok(()),
        },
    }
}
