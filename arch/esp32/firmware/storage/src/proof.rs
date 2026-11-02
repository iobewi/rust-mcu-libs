//! Flash access to one slot's boot-proof sector, and nothing else.
//!
//! The proof sector is outside the region the OTA write permit covers for the image (the
//! image never reaches it). The permit also names the sector
//! ([`InactiveWriteTarget::proof_sector`](iobewi_firmware_layout::authorization::InactiveWriteTarget::proof_sector)),
//! and this adapter lets the proof writer erase, program and read **only that sector**, so the
//! proof code cannot reach the confirmed slot even by mistake.

#[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
pub use flash::ProofFlash;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProofFlashError {
    /// The access leaves the proof sector.
    OutsideSector,
    Flash,
}

#[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
mod flash {
    use super::ProofFlashError;
    use embedded_storage::nor_flash::{NorFlash, ReadNorFlash};
    use iobewi_esp_flash::EspFlash;
    use iobewi_firmware_boot::transaction::Flash;
    use iobewi_firmware_layout::Region;

    pub struct ProofFlash<'a> {
        flash: &'a mut EspFlash,
        sector: Region,
    }

    impl<'a> ProofFlash<'a> {
        /// `None` unless `sector` is exactly one erase unit on an erase boundary.
        pub fn new(flash: &'a mut EspFlash, sector: Region) -> Option<Self> {
            let unit = <EspFlash as NorFlash>::ERASE_SIZE as u32;
            (sector.size == unit as usize && sector.offset % unit == 0)
                .then_some(Self { flash, sector })
        }
    }

    impl Flash for ProofFlash<'_> {
        type Error = ProofFlashError;

        fn read(&mut self, absolute: u32, output: &mut [u8]) -> Result<(), Self::Error> {
            if !self.sector.contains_range(absolute, output.len()) {
                return Err(ProofFlashError::OutsideSector);
            }
            ReadNorFlash::read(self.flash, absolute, output).map_err(|_| ProofFlashError::Flash)
        }

        fn erase(&mut self, absolute: u32, len: u32) -> Result<(), Self::Error> {
            if absolute != self.sector.offset || len as usize != self.sector.size {
                return Err(ProofFlashError::OutsideSector);
            }
            NorFlash::erase(self.flash, absolute, absolute + len)
                .map_err(|_| ProofFlashError::Flash)
        }

        fn program(&mut self, absolute: u32, bytes: &[u8]) -> Result<(), Self::Error> {
            if !self.sector.contains_range(absolute, bytes.len()) {
                return Err(ProofFlashError::OutsideSector);
            }
            NorFlash::write(self.flash, absolute, bytes).map_err(|_| ProofFlashError::Flash)
        }
    }
}
