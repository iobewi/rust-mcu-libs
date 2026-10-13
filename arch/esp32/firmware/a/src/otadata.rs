//! Transactional legacy A/B boot metadata access.
//!
//! Reads two 4KiB sectors; writes erase -> uncommitted body -> readback ->
//! commit word last. Hardware flash access is implemented by the caller.
//! This module never assumes that ESP-IDF's standard bootloader accepts the legacy transactional metadata encoding.

use crate::Partition;
use iobewi_firmware_boot::{
    BLANK, COMMIT, Decoded, ENTRY_SIZE, OFF_COMMIT, Raw, SECTOR_COUNT, Write, decode,
};

const SECTOR_SIZE: u32 = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error<E> {
    Flash(E),
    BadPartition,
    BadSector,
    NoTrustedFallback,
    ReadbackMismatch,
    CommitMismatch,
}

/// Physical flash operations. Implementations must use one shared flash owner
/// and satisfy 4KiB erase / 4-byte write alignment and durability guarantees.
pub trait Flash {
    type Error;
    fn read(&mut self, absolute: u32, output: &mut [u8]) -> Result<(), Self::Error>;
    fn erase(&mut self, absolute: u32, len: u32) -> Result<(), Self::Error>;
    fn program(&mut self, absolute: u32, bytes: &[u8]) -> Result<(), Self::Error>;
}

fn sector_at<E>(part: Partition, sector: usize) -> Result<u32, Error<E>> {
    if part.size < SECTOR_SIZE * SECTOR_COUNT as u32
        || !part.offset.is_multiple_of(SECTOR_SIZE)
        || sector >= SECTOR_COUNT
    {
        return Err(Error::BadPartition);
    }
    part.offset
        .checked_add(part.size)
        .ok_or(Error::BadPartition)?;
    part.offset
        .checked_add((sector as u32) * SECTOR_SIZE)
        .ok_or(Error::BadPartition)
}

/// Read the first 32 bytes from each metadata sector.
pub fn read_entries<F: Flash>(
    flash: &mut F,
    part: Partition,
) -> Result<[Raw; SECTOR_COUNT], Error<F::Error>> {
    let mut out = [BLANK; SECTOR_COUNT];
    for (i, raw) in out.iter_mut().enumerate() {
        let offset = sector_at::<F::Error>(part, i)?;
        flash.read(offset, raw).map_err(Error::Flash)?;
    }
    Ok(out)
}

/// Write the requested sector, while refusing to erase the sole confirmed
/// fallback. The selection and actual running firmware must also be verified
/// by the calling update orchestration before an activation is attempted.
pub fn execute<F: Flash>(
    flash: &mut F,
    part: Partition,
    write: Write,
) -> Result<(), Error<F::Error>> {
    let sector = usize::from(write.sector);
    let addr = sector_at::<F::Error>(part, sector)?;
    let entries = read_entries(flash, part)?;

    let existing_valid = entries.iter().enumerate().any(|(i, raw)| {
        i != sector && matches!(decode(raw), Decoded::Ok(e) if e.state == iobewi_firmware_boot::state::VALID)
    });
    let target_valid = matches!(decode(&entries[sector]), Decoded::Ok(e) if e.state == iobewi_firmware_boot::state::VALID);
    if target_valid && !existing_valid {
        return Err(Error::NoTrustedFallback);
    }

    let expected = write.entry.body();
    flash.erase(addr, SECTOR_SIZE).map_err(Error::Flash)?;
    let mut readback = [0u8; ENTRY_SIZE];
    flash.read(addr, &mut readback).map_err(Error::Flash)?;
    if readback != BLANK {
        return Err(Error::ReadbackMismatch);
    }
    flash.program(addr, &expected).map_err(Error::Flash)?;
    flash.read(addr, &mut readback).map_err(Error::Flash)?;
    if readback != expected {
        return Err(Error::ReadbackMismatch);
    }
    // The commit word must still be erased at this point.
    if readback[OFF_COMMIT..OFF_COMMIT + 4] != [0xff; 4] {
        return Err(Error::ReadbackMismatch);
    }

    flash
        .program(addr + OFF_COMMIT as u32, &COMMIT.to_le_bytes())
        .map_err(Error::Flash)?;
    flash.read(addr, &mut readback).map_err(Error::Flash)?;
    if readback != write.entry.encode() || decode(&readback) != Decoded::Ok(write.entry) {
        return Err(Error::CommitMismatch);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use iobewi_firmware_boot::{Entry, state};

    struct Mem {
        raw: [u8; 8192],
        cut_after_body: bool,
        erase_fault: bool,
    }
    impl Mem {
        fn new() -> Self {
            Self {
                raw: [0xff; 8192],
                cut_after_body: false,
                erase_fault: false,
            }
        }
    }
    impl Flash for Mem {
        type Error = ();
        fn read(&mut self, absolute: u32, out: &mut [u8]) -> Result<(), ()> {
            let at = absolute as usize;
            out.copy_from_slice(&self.raw[at..at + out.len()]);
            Ok(())
        }
        fn erase(&mut self, absolute: u32, len: u32) -> Result<(), ()> {
            if !self.erase_fault {
                self.raw[absolute as usize..(absolute + len) as usize].fill(0xff);
            }
            Ok(())
        }
        fn program(&mut self, absolute: u32, bytes: &[u8]) -> Result<(), ()> {
            if self.cut_after_body && bytes.len() == 4 {
                return Err(());
            }
            for (old, new) in self.raw[absolute as usize..absolute as usize + bytes.len()]
                .iter_mut()
                .zip(bytes)
            {
                *old &= *new;
            }
            Ok(())
        }
    }
    fn part() -> Partition {
        Partition {
            offset: 0,
            size: 8192,
        }
    }
    #[test]
    fn publish_only_after_body_readback() {
        let mut flash = Mem::new();
        execute(
            &mut flash,
            part(),
            Write {
                sector: 0,
                entry: Entry::new(1, state::VALID),
            },
        )
        .unwrap();
        assert_eq!(
            read_entries(&mut flash, part()).unwrap()[0],
            Entry::new(1, state::VALID).encode()
        );
        flash.cut_after_body = true;
        assert_eq!(
            execute(
                &mut flash,
                part(),
                Write {
                    sector: 1,
                    entry: Entry::new(2, state::NEW)
                }
            ),
            Err(Error::Flash(()))
        );
        assert_eq!(
            read_entries(&mut flash, part()).unwrap()[1][OFF_COMMIT..OFF_COMMIT + 4],
            [0xff; 4]
        );
        assert!(
            matches!(decode(&read_entries(&mut flash, part()).unwrap()[0]), Decoded::Ok(e) if e.state == state::VALID)
        );
    }
    #[test]
    fn sole_valid_entry_is_never_erased() {
        let mut flash = Mem::new();
        execute(
            &mut flash,
            part(),
            Write {
                sector: 0,
                entry: Entry::new(1, state::VALID),
            },
        )
        .unwrap();
        assert_eq!(
            execute(
                &mut flash,
                part(),
                Write {
                    sector: 0,
                    entry: Entry::new(3, state::NEW)
                }
            ),
            Err(Error::NoTrustedFallback)
        );
    }
    #[test]
    fn failed_erase_is_detected_before_body_or_commit() {
        let mut flash = Mem::new();
        flash.raw[4096..4096 + ENTRY_SIZE].fill(0);
        flash.erase_fault = true;
        let before = flash.raw;
        assert_eq!(
            execute(
                &mut flash,
                part(),
                Write {
                    sector: 1,
                    entry: Entry::new(2, state::NEW)
                }
            ),
            Err(Error::ReadbackMismatch)
        );
        assert_eq!(flash.raw, before);
    }

    #[test]
    fn system_activation_confirmation_and_unconfirmed_reboot_preserve_fallback() {
        let mut flash = Mem::new();
        let mut apply_plan = |records, image_ok: &mut dyn FnMut(u8) -> bool| {
            let plan = iobewi_firmware_boot::plan_boot(records, 2, image_ok);
            for write in plan.writes() {
                execute(&mut flash, part(), write).unwrap();
            }
            plan.boot
        };
        assert!(matches!(
            apply_plan([BLANK; 2], &mut |_| true),
            iobewi_firmware_boot::Boot::Slot { slot: 0, .. }
        ));
        let records = read_entries(&mut flash, part()).unwrap();
        execute(
            &mut flash,
            part(),
            iobewi_firmware_boot::activate(records, 2, 1).unwrap(),
        )
        .unwrap();
        let plan = iobewi_firmware_boot::plan_boot(
            read_entries(&mut flash, part()).unwrap(),
            2,
            &mut |_| true,
        );
        for write in plan.writes() {
            execute(&mut flash, part(), write).unwrap();
        }
        assert!(matches!(
            plan.boot,
            iobewi_firmware_boot::Boot::Slot { slot: 1, .. }
        ));
        let records = read_entries(&mut flash, part()).unwrap();
        execute(
            &mut flash,
            part(),
            iobewi_firmware_boot::confirm(records).unwrap(),
        )
        .unwrap();
        assert_eq!(
            iobewi_firmware_boot::update_target(read_entries(&mut flash, part()).unwrap(), 2),
            Some(0)
        );
        let records = read_entries(&mut flash, part()).unwrap();
        execute(
            &mut flash,
            part(),
            iobewi_firmware_boot::activate(records, 2, 0).unwrap(),
        )
        .unwrap();
        let plan = iobewi_firmware_boot::plan_boot(
            read_entries(&mut flash, part()).unwrap(),
            2,
            &mut |_| true,
        );
        for write in plan.writes() {
            execute(&mut flash, part(), write).unwrap();
        }
        assert!(matches!(
            plan.boot,
            iobewi_firmware_boot::Boot::Slot { slot: 0, .. }
        ));
        // A reboot before confirmation aborts slot 0 and restores confirmed slot 1.
        let plan = iobewi_firmware_boot::plan_boot(
            read_entries(&mut flash, part()).unwrap(),
            2,
            &mut |_| true,
        );
        for write in plan.writes() {
            execute(&mut flash, part(), write).unwrap();
        }
        assert!(matches!(
            plan.boot,
            iobewi_firmware_boot::Boot::Slot { slot: 1, .. }
        ));
        assert_eq!(
            iobewi_firmware_boot::update_target(read_entries(&mut flash, part()).unwrap(), 2),
            Some(0)
        );
    }
}
