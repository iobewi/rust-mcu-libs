#![no_std]
#![allow(async_fn_in_trait)]
//! EKV NOR partition adapter using the existing single ESP SharedFlash owner.
//! Transaction, generation and capacity policies remain in ConfigSpace/EKV.
#[cfg(any(test, feature = "esp32c3", feature = "esp32s3"))]
const PAGE: usize = iobewi_config_space::storage::PAGE_SIZE;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeometryError {
    Alignment,
    Size,
    Bounds,
}
#[cfg(any(test, feature = "esp32c3", feature = "esp32s3"))]
fn geometry(offset: u32, size: usize, capacity: usize) -> Result<(), GeometryError> {
    if !(offset as usize).is_multiple_of(PAGE) || !size.is_multiple_of(PAGE) {
        return Err(GeometryError::Alignment);
    }
    if !(32..=256).contains(&(size / PAGE)) {
        return Err(GeometryError::Size);
    }
    let end = (offset as usize)
        .checked_add(size)
        .ok_or(GeometryError::Bounds)?;
    if end > capacity || end > u32::MAX as usize {
        return Err(GeometryError::Bounds);
    }
    Ok(())
}
#[cfg(any(test, feature = "esp32c3", feature = "esp32s3"))]
fn address(base: u32, size: usize, page: usize, offset: usize, len: usize) -> Option<u32> {
    if page >= size / PAGE || offset.checked_add(len)? > PAGE {
        return None;
    }
    if !offset.is_multiple_of(4) || !len.is_multiple_of(4) {
        return None;
    }
    let relative = page.checked_mul(PAGE)?.checked_add(offset)?;
    base.checked_add(u32::try_from(relative).ok()?)
}
#[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
mod runtime {
    use super::*;
    use embedded_storage::nor_flash::{ErrorType, NorFlash, ReadNorFlash};
    pub use iobewi_esp_flash::FlashPeripheral;
    use iobewi_esp_flash::{EspFlash, SharedFlash};
    #[derive(Debug)]
    pub enum Error {
        Discovery(iobewi_esp_partitions::PartitionError),
        Layout(iobewi_firmware_layout::DiscoveryError),
        Geometry(GeometryError),
        DriverGeometry,
        Bounds,
        Flash(<EspFlash as ErrorType>::Error),
    }
    /// Exclusive EKV view of one data/0x40 partition. No cloned view or second
    /// FlashStorage owner. All callbacks lock SharedFlash for one operation.
    pub struct EspConfigFlash {
        flash: &'static SharedFlash,
        base: u32,
        size: usize,
    }
    impl EspConfigFlash {
        pub async fn from_label(flash: &'static SharedFlash, label: &str) -> Result<Self, Error> {
            if EspFlash::ERASE_SIZE != PAGE
                || EspFlash::READ_SIZE == 0
                || EspFlash::WRITE_SIZE == 0
                || !4usize.is_multiple_of(EspFlash::READ_SIZE)
                || !4usize.is_multiple_of(EspFlash::WRITE_SIZE)
            {
                return Err(Error::DriverGeometry);
            }
            let mut owner = flash.lock().await;
            let mut table = [0u8; iobewi_esp_partitions::TABLE_BUFFER_SIZE];
            // Standard reference profile: second stage and table occupy 0..0x9000.
            // Validate the complete map and unique label before mutation.
            use iobewi_firmware_layout::{Entry, Region, Requirement};
            let mut regions =
                [Region { offset: 0, size: 0 }; iobewi_esp_partitions::raw::MAX_LEN / 32 + 1];
            regions[0] = Region {
                offset: 0,
                size: 0x9000,
            };
            let mut count = 1;
            iobewi_esp_partitions::for_each_entry(owner.storage(), &mut table, |entry| {
                regions[count] = Region {
                    offset: entry.offset,
                    size: entry.size as usize,
                };
                count += 1;
            })
            .map_err(Error::Discovery)?;
            let capacity = u32::try_from(owner.capacity()).map_err(|_| Error::Bounds)?;
            iobewi_firmware_layout::validate_regions(&regions[..count], capacity, PAGE as u32)
                .map_err(Error::Layout)?;
            let entries = iobewi_esp_partitions::raw::entries(&table)
                .map_err(|_| Error::Bounds)?
                .map(|e| Entry {
                    label: e.label,
                    kind: e.kind,
                    subtype: e.subtype,
                    offset: e.offset,
                    size: e.size,
                });
            let [range] = iobewi_firmware_layout::validate(
                entries,
                &[Requirement {
                    label,
                    kind: 1,
                    subtype: 0x40,
                }],
                capacity,
                PAGE as u32,
            )
            .map_err(Error::Layout)?;
            geometry(range.offset, range.size, owner.capacity()).map_err(Error::Geometry)?;
            Ok(Self {
                flash,
                base: range.offset,
                size: range.size,
            })
        }
        /// Read-only virgin-region check for an explicit first-boot policy.
        /// A nonblank region must be mounted; errors never trigger formatting.
        pub async fn is_blank(&self) -> Result<bool, Error> {
            let mut data = [0u8; 256];
            for offset in (0..self.size).step_by(data.len()) {
                let addr = self
                    .base
                    .checked_add(u32::try_from(offset).map_err(|_| Error::Bounds)?)
                    .ok_or(Error::Bounds)?;
                self.flash
                    .lock()
                    .await
                    .read(addr, &mut data)
                    .map_err(Error::Flash)?;
                if data.iter().any(|b| *b != 255) {
                    return Ok(false);
                }
            }
            Ok(true)
        }
    }
    impl ekv::flash::Flash for EspConfigFlash {
        type Error = Error;
        fn page_count(&self) -> usize {
            self.size / PAGE
        }
        async fn read(
            &mut self,
            page: ekv::flash::PageID,
            offset: usize,
            data: &mut [u8],
        ) -> Result<(), Error> {
            let addr = address(self.base, self.size, page.index(), offset, data.len())
                .ok_or(Error::Bounds)?;
            self.flash
                .lock()
                .await
                .read(addr, data)
                .map_err(Error::Flash)
        }
        async fn write(
            &mut self,
            page: ekv::flash::PageID,
            offset: usize,
            data: &[u8],
        ) -> Result<(), Error> {
            let addr = address(self.base, self.size, page.index(), offset, data.len())
                .ok_or(Error::Bounds)?;
            self.flash
                .lock()
                .await
                .write(addr, data)
                .map_err(Error::Flash)
        }
        async fn erase(&mut self, page: ekv::flash::PageID) -> Result<(), Error> {
            let addr = address(self.base, self.size, page.index(), 0, PAGE).ok_or(Error::Bounds)?;
            let end = addr.checked_add(PAGE as u32).ok_or(Error::Bounds)?;
            self.flash
                .lock()
                .await
                .erase(addr, end)
                .map_err(Error::Flash)
        }
    }
}
#[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
pub use runtime::*;
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_regions_outside_geometry_and_flash() {
        assert_eq!(geometry(0x3c0000, 0x40000, 0x400000), Ok(()));
        assert_eq!(
            geometry(0x3c0001, 0x40000, 0x400000),
            Err(GeometryError::Alignment)
        );
        assert_eq!(
            geometry(0x3d0000, 0x40000, 0x400000),
            Err(GeometryError::Bounds)
        );
        assert_eq!(geometry(0, 0x4000, 0x400000), Err(GeometryError::Size));
        assert_eq!(geometry(0, 0x101000, 0x400000), Err(GeometryError::Size));
    }
    #[test]
    fn every_page_callback_is_bounded_and_aligned() {
        assert_eq!(address(0x3c0000, 0x40000, 63, 0, 4096), Some(0x3ff000));
        assert!(address(0x3c0000, 0x40000, 64, 0, 4).is_none());
        assert!(address(0x3c0000, 0x40000, 63, 4092, 8).is_none());
        assert!(address(0, 0x40000, 0, 1, 4).is_none());
        assert!(address(0, 0x40000, 0, 0, 3).is_none());
        assert!(address(0, 0x40000, 0, usize::MAX, 4).is_none());
    }
}
