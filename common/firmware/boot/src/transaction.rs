//! Verified erase/body/commit executor shared by ROM boot and the application.
//! The caller selects transitions and owns exclusive flash access for the call.
use crate::{BLANK, COMMIT, Decoded, ENTRY_SIZE, OFF_COMMIT, Raw, SECTOR_COUNT, Write, decode};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub offset: u32,
    pub size: u32,
}
const SECTOR_SIZE: u32 = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error<E> {
    Flash(E),
    BadPartition,
    BadSector,
    ReadbackMismatch,
    CommitMismatch,
}

/// Physical flash operations. Implementations must provide exclusive physical flash access
/// and satisfy 4KiB erase / 4-byte write alignment and durability guarantees.
pub trait Flash {
    type Error;
    fn read(&mut self, absolute: u32, output: &mut [u8]) -> Result<(), Self::Error>;
    fn erase(&mut self, absolute: u32, len: u32) -> Result<(), Self::Error>;
    fn program(&mut self, absolute: u32, bytes: &[u8]) -> Result<(), Self::Error>;
}

fn sector_at<E>(part: Region, sector: usize) -> Result<u32, Error<E>> {
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
    part: Region,
) -> Result<[Raw; SECTOR_COUNT], Error<F::Error>> {
    let mut out = [BLANK; SECTOR_COUNT];
    for (i, raw) in out.iter_mut().enumerate() {
        let offset = sector_at::<F::Error>(part, i)?;
        flash.read(offset, raw).map_err(Error::Flash)?;
    }
    Ok(out)
}

/// Execute an already selected transition. Does not impose application fallback
/// policy: boot may invalidate the sole Valid entry if its image is unbootable.
pub fn execute<F: Flash>(flash: &mut F, part: Region, write: Write) -> Result<(), Error<F::Error>> {
    let addr = sector_at::<F::Error>(part, usize::from(write.sector))?;
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
    use crate::{Boot, Entry, plan_boot, state};

    struct FlashModel {
        data: [u8; 8192],
        // A power cut after this many bytes have reached flash.
        remaining: usize,
    }
    impl Flash for FlashModel {
        type Error = ();
        fn read(&mut self, at: u32, out: &mut [u8]) -> Result<(), ()> {
            out.copy_from_slice(&self.data[at as usize..at as usize + out.len()]);
            Ok(())
        }
        fn erase(&mut self, at: u32, len: u32) -> Result<(), ()> {
            self.data[at as usize..(at + len) as usize].fill(0xff);
            Ok(())
        }
        fn program(&mut self, at: u32, bytes: &[u8]) -> Result<(), ()> {
            for (i, byte) in bytes.iter().enumerate() {
                if self.remaining == 0 {
                    return Err(());
                }
                self.data[at as usize + i] &= byte;
                self.remaining -= 1;
            }
            Ok(())
        }
    }
    const PART: Region = Region {
        offset: 0,
        size: 8192,
    };

    #[test]
    fn every_body_and_commit_prefix_preserves_the_other_confirmed_image() {
        let valid = Entry::new(1, state::VALID);
        let new = Entry::new(2, state::NEW);
        for durable_bytes in 0..=ENTRY_SIZE + 4 {
            let mut flash = FlashModel {
                data: [0xff; 8192],
                remaining: durable_bytes,
            };
            flash.data[..ENTRY_SIZE].copy_from_slice(&valid.encode());
            let result = execute(
                &mut flash,
                PART,
                Write {
                    sector: 1,
                    entry: new,
                },
            );
            let records = read_entries(&mut flash, PART).unwrap();
            assert_eq!(decode(&records[0]), Decoded::Ok(valid));
            if durable_bytes < ENTRY_SIZE + 4 {
                assert!(result.is_err());
                assert!(!matches!(decode(&records[1]), Decoded::Ok(_)));
                assert!(matches!(
                    plan_boot(records, 2, &mut |_| true).boot,
                    Boot::Slot { slot: 0, .. }
                ));
            } else {
                assert!(result.is_ok());
                assert_eq!(decode(&records[1]), Decoded::Ok(new));
            }
        }
    }
    #[test]
    fn boot_can_invalidate_a_sole_valid_entry_when_its_image_is_unbootable() {
        let mut flash = FlashModel {
            data: [0xff; 8192],
            remaining: usize::MAX,
        };
        flash.data[..ENTRY_SIZE].copy_from_slice(&Entry::new(3, state::VALID).encode());
        flash.data[4096..4096 + ENTRY_SIZE].copy_from_slice(&Entry::new(2, state::NEW).encode());
        let plan = plan_boot(read_entries(&mut flash, PART).unwrap(), 2, &mut |slot| {
            slot == 1
        });
        assert!(matches!(plan.boot, Boot::Slot { slot: 1, .. }));
        let writes: std::vec::Vec<_> = plan.writes().collect();
        assert!(!writes.is_empty());
        for write in writes {
            execute(&mut flash, PART, write).unwrap();
        }
        assert!(
            !read_entries(&mut flash, PART)
                .unwrap()
                .iter()
                .any(|e| matches!(decode(e),Decoded::Ok(e) if e.state==state::VALID))
        );
    }
}
