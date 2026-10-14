//! OTM2 copies on two independent NOR erase units, under an already held flash lock.
//! The caller borrows the sole physical owner; no flash or mutex is constructed here.
use embedded_storage::nor_flash::{NorFlash, ReadNorFlash};
use iobewi_firmware_layout::domain_b::WorkloadLayout;
use iobewi_firmware_update::compat::otm2::RECORD_LEN;
use iobewi_firmware_update::compat::otm2_store::MetadataBackend;

#[derive(Debug, PartialEq, Eq)]
pub enum Error<E> {
    Geometry,
    Copy,
    Readback,
    Flash(E),
}

pub struct NorMetadata<'a, F> {
    flash: &'a mut F,
    offset: u32,
}

impl<'a, F: NorFlash> NorMetadata<'a, F> {
    pub fn new(flash: &'a mut F, layout: &WorkloadLayout) -> Result<Self, Error<F::Error>> {
        let meta = layout.meta();
        if F::ERASE_SIZE == 0
            || F::WRITE_SIZE == 0
            || F::READ_SIZE == 0
            || layout.erase_size() as usize != F::ERASE_SIZE
            || RECORD_LEN > F::ERASE_SIZE
            || !RECORD_LEN.is_multiple_of(F::WRITE_SIZE)
            || !RECORD_LEN.is_multiple_of(F::READ_SIZE)
            || !(meta.offset as usize).is_multiple_of(F::ERASE_SIZE)
            || meta.size as usize / F::ERASE_SIZE < 2
            || meta.end() > flash.capacity() as u64
            || layout
                .regions()
                .iter()
                .any(|r| r.end() > flash.capacity() as u64)
        {
            return Err(Error::Geometry);
        }
        Ok(Self {
            flash,
            offset: meta.offset,
        })
    }

    fn address(&self, copy: usize) -> Result<u32, Error<F::Error>> {
        if copy >= 2 {
            return Err(Error::Copy);
        }
        self.offset
            .checked_add((copy * F::ERASE_SIZE) as u32)
            .ok_or(Error::Geometry)
    }
}

impl<F: NorFlash> MetadataBackend for NorMetadata<'_, F> {
    type Error = Error<F::Error>;
    fn read(&mut self, copy: usize) -> Result<[u8; RECORD_LEN], Self::Error> {
        let address = self.address(copy)?;
        let mut bytes = [0; RECORD_LEN];
        self.flash.read(address, &mut bytes).map_err(Error::Flash)?;
        Ok(bytes)
    }
    fn write(&mut self, copy: usize, bytes: &[u8; RECORD_LEN]) -> Result<(), Self::Error> {
        let address = self.address(copy)?;
        self.flash
            .erase(address, address + F::ERASE_SIZE as u32)
            .map_err(Error::Flash)?;
        if self.read(copy)? != [0xff; RECORD_LEN] {
            return Err(Error::Readback);
        }
        self.flash.write(address, bytes).map_err(Error::Flash)?;
        if self.read(copy)? != *bytes {
            return Err(Error::Readback);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use embedded_storage::nor_flash::{ErrorType, NorFlashError, NorFlashErrorKind};
    use iobewi_firmware_layout::domain_b::{Region, assemble};
    use iobewi_firmware_update::compat::otm2::{Record, State};
    use iobewi_firmware_update::compat::otm2_store::{Loaded, MetadataStore};
    use std::vec;
    use std::vec::Vec;
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct Cut;
    impl NorFlashError for Cut {
        fn kind(&self) -> NorFlashErrorKind {
            NorFlashErrorKind::Other
        }
    }
    struct Flash {
        bytes: Vec<u8>,
        cut: Option<usize>,
        faulty_erase: bool,
        faulty_write: bool,
    }
    impl ErrorType for Flash {
        type Error = Cut;
    }
    impl ReadNorFlash for Flash {
        const READ_SIZE: usize = 1;
        fn capacity(&self) -> usize {
            self.bytes.len()
        }
        fn read(&mut self, at: u32, out: &mut [u8]) -> Result<(), Cut> {
            out.copy_from_slice(&self.bytes[at as usize..at as usize + out.len()]);
            Ok(())
        }
    }
    impl NorFlash for Flash {
        const WRITE_SIZE: usize = 4;
        const ERASE_SIZE: usize = 4096;
        fn erase(&mut self, from: u32, to: u32) -> Result<(), Cut> {
            if self.faulty_erase {
                return Ok(());
            }
            for i in from as usize..to as usize {
                self.byte()?;
                self.bytes[i] = 0xff;
            }
            Ok(())
        }
        fn write(&mut self, at: u32, bytes: &[u8]) -> Result<(), Cut> {
            if self.faulty_write {
                return Ok(());
            }
            for (i, b) in bytes.iter().enumerate() {
                self.byte()?;
                self.bytes[at as usize + i] &= b;
            }
            Ok(())
        }
    }
    impl Flash {
        fn byte(&mut self) -> Result<(), Cut> {
            if let Some(n) = self.cut.as_mut() {
                if *n == 0 {
                    return Err(Cut);
                }
                *n -= 1;
            }
            Ok(())
        }
        fn new() -> Self {
            Self {
                bytes: vec![0xff; 0xa0000],
                cut: None,
                faulty_erase: false,
                faulty_write: false,
            }
        }
    }
    fn layout() -> WorkloadLayout {
        assemble(
            Some(Region::new(0x9000, 0x2000)),
            Some(Region::new(0x20000, 0x40000)),
            Some(Region::new(0x60000, 0x40000)),
            4096,
        )
        .unwrap()
    }
    #[test]
    fn every_erase_and_program_prefix_preserves_the_other_copy_and_regions() {
        let mut base = Flash::new();
        // Exercise a real rotation: both copies already contain valid records,
        // and the older sector is reused rather than being virgin.
        for sequence in 1..=3 {
            MetadataStore::new(NorMetadata::new(&mut base, &layout()).unwrap())
                .commit(&Record::empty(sequence))
                .unwrap();
        }
        let first = Record::empty(3);
        let snapshot = base.bytes.clone();
        // Real backend, including every interrupted erase byte, not only record prefixes.
        for cut in 0..4096 + RECORD_LEN {
            let mut flash = Flash::new();
            flash.bytes.clone_from(&snapshot);
            flash.cut = Some(cut);
            let mut next = Record::empty(4);
            next.state = State::Empty;
            assert!(
                MetadataStore::new(NorMetadata::new(&mut flash, &layout()).unwrap())
                    .commit(&next)
                    .is_err()
            );
            flash.cut = None;
            let loaded = MetadataStore::new(NorMetadata::new(&mut flash, &layout()).unwrap())
                .load()
                .unwrap();
            assert!(matches!(loaded, Loaded::Record { record, .. } if record == first));
            assert_eq!(&flash.bytes[..0xa000], &snapshot[..0xa000]);
            assert_eq!(&flash.bytes[0xb000..], &snapshot[0xb000..]);
        }
    }
    #[test]
    fn false_success_is_rejected_and_bad_copy_never_writes() {
        let mut flash = Flash::new();
        flash.bytes[0x9000..0x9000 + RECORD_LEN].fill(0);
        flash.faulty_erase = true;
        let mut backend = NorMetadata::new(&mut flash, &layout()).unwrap();
        assert_eq!(
            backend.write(0, &Record::empty(1).encode()),
            Err(Error::Readback)
        );
        assert_eq!(
            backend.write(2, &Record::empty(1).encode()),
            Err(Error::Copy)
        );
        flash.faulty_erase = false;
        flash.faulty_write = true;
        assert_eq!(
            NorMetadata::new(&mut flash, &layout())
                .unwrap()
                .write(0, &Record::empty(1).encode()),
            Err(Error::Readback)
        );
    }
    #[test]
    fn refuses_out_of_flash_layout_before_mutation() {
        let mut flash = Flash::new();
        flash.bytes.truncate(0x90000);
        assert!(matches!(
            NorMetadata::new(&mut flash, &layout()),
            Err(Error::Geometry)
        ));
    }
}
