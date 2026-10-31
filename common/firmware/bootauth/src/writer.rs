//! Writing a proof: erase, body, read back, commit word last.
//!
//! The sector is erased first, so an interrupted write can only leave an erased sector, a
//! header without commit word, or the complete record. Run it before the slot's `otadata`
//! entry is written, from code that already verified the envelope.

use iobewi_firmware_boot::transaction::Flash;
use iobewi_firmware_layout::domain_a::Partition;

use crate::iap1::{self, Write};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error<E> {
    Flash(E),
    /// The sector was not erased, or a programmed part reads back differently.
    ReadbackMismatch,
    /// The commit word reads back differently.
    CommitMismatch,
}

/// Write `proof` into the proof sector of `slot`. The caller owns the flash exclusively.
pub fn execute<F: Flash>(
    flash: &mut F,
    slot: Partition,
    proof: &Write<'_>,
) -> Result<(), Error<F::Error>> {
    let base = slot.proof_offset();
    flash
        .erase(base, iap1::SECTOR as u32)
        .map_err(Error::Flash)?;
    let mut probe = [0u8; iap1::HEADER_LEN];
    flash.read(base, &mut probe).map_err(Error::Flash)?;
    if probe != [0xFF; iap1::HEADER_LEN] {
        return Err(Error::ReadbackMismatch);
    }

    flash.program(base, &proof.head).map_err(Error::Flash)?;
    let payload_at = base + iap1::HEADER_LEN as u32;
    flash
        .program(payload_at, proof.payload)
        .map_err(Error::Flash)?;
    let mut head = [0u8; iap1::OFF_COMMIT];
    flash.read(base, &mut head).map_err(Error::Flash)?;
    if head != proof.head {
        return Err(Error::ReadbackMismatch);
    }
    let mut chunk = [0u8; 64];
    let mut done = 0;
    while done < proof.payload.len() {
        let n = (proof.payload.len() - done).min(chunk.len());
        flash
            .read(payload_at + done as u32, &mut chunk[..n])
            .map_err(Error::Flash)?;
        if chunk[..n] != proof.payload[done..done + n] {
            return Err(Error::ReadbackMismatch);
        }
        done += n;
    }
    let mut commit = [0u8; 4];
    flash
        .read(base + iap1::OFF_COMMIT as u32, &mut commit)
        .map_err(Error::Flash)?;
    if commit != [0xFF; 4] {
        return Err(Error::ReadbackMismatch);
    }

    flash
        .program(base + iap1::OFF_COMMIT as u32, &iap1::COMMIT.to_le_bytes())
        .map_err(Error::Flash)?;
    flash
        .read(base + iap1::OFF_COMMIT as u32, &mut commit)
        .map_err(Error::Flash)?;
    if commit != iap1::COMMIT.to_le_bytes() {
        return Err(Error::CommitMismatch);
    }
    Ok(())
}
