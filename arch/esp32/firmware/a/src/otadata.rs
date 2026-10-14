//! Transactional legacy A/B boot metadata access.
//!
//! Reads two 4KiB sectors; writes erase -> uncommitted body -> readback ->
//! commit word last. Hardware flash access is implemented by the caller.
//! This module never assumes that ESP-IDF's standard bootloader accepts the legacy transactional metadata encoding.

use crate::Partition;
use iobewi_firmware_boot::{Decoded, Raw, SECTOR_COUNT, Write, decode};

pub use iobewi_firmware_boot::transaction::Flash;
use iobewi_firmware_boot::transaction::{self, Region};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error<E> {
    Flash(E),
    BadPartition,
    BadSector,
    NoTrustedFallback,
    ReadbackMismatch,
    CommitMismatch,
}
impl<E> From<transaction::Error<E>> for Error<E> {
    fn from(e: transaction::Error<E>) -> Self {
        match e {
            transaction::Error::Flash(e) => Self::Flash(e),
            transaction::Error::BadPartition => Self::BadPartition,
            transaction::Error::BadSector => Self::BadSector,
            transaction::Error::ReadbackMismatch => Self::ReadbackMismatch,
            transaction::Error::CommitMismatch => Self::CommitMismatch,
        }
    }
}
fn region(part: Partition) -> Region {
    Region {
        offset: part.offset,
        size: part.size,
    }
}
pub fn read_entries<F: Flash>(
    flash: &mut F,
    part: Partition,
) -> Result<[Raw; SECTOR_COUNT], Error<F::Error>> {
    transaction::read_entries(flash, region(part)).map_err(Into::into)
}
/// Protect the sole confirmed fallback before the shared physical transaction.
pub fn execute<F: Flash>(
    flash: &mut F,
    part: Partition,
    write: Write,
) -> Result<(), Error<F::Error>> {
    let sector = usize::from(write.sector);
    if sector >= SECTOR_COUNT {
        return Err(Error::BadPartition);
    }
    let entries = read_entries(flash, part)?;
    let existing_valid = entries.iter().enumerate().any(|(i, raw)| {
        i != sector && matches!(decode(raw), Decoded::Ok(e) if e.state == iobewi_firmware_boot::state::VALID)
    });
    let target_valid = matches!(decode(&entries[sector]), Decoded::Ok(e) if e.state == iobewi_firmware_boot::state::VALID);
    if target_valid && !existing_valid {
        return Err(Error::NoTrustedFallback);
    }
    transaction::execute(flash, region(part), write).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use iobewi_firmware_boot::{BLANK, ENTRY_SIZE, Entry, OFF_COMMIT, state};

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
