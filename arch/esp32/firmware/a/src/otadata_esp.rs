//! ESP32 implementation of the transactional boot metadata flash interface.
//! The SharedFlash lock is held across the complete read/erase/program/readback
//! transaction; no other IOBEWI flash consumer can interleave operations.

use crate::{Partition, otadata};
use embedded_storage::nor_flash::{NorFlash, ReadNorFlash};
use iobewi_esp_flash::{EspFlash, SharedFlash};
use iobewi_firmware_boot::{Raw, SECTOR_COUNT, Write};

pub(crate) struct LockedFlash<'a>(pub(crate) &'a mut EspFlash);

impl otadata::Flash for LockedFlash<'_> {
    type Error = <EspFlash as embedded_storage::nor_flash::ErrorType>::Error;

    fn read(&mut self, absolute: u32, output: &mut [u8]) -> Result<(), Self::Error> {
        self.0.read(absolute, output)
    }

    fn erase(&mut self, absolute: u32, len: u32) -> Result<(), Self::Error> {
        self.0.erase(absolute, absolute + len)
    }

    fn program(&mut self, absolute: u32, bytes: &[u8]) -> Result<(), Self::Error> {
        self.0.write(absolute, bytes)
    }
}

/// Atomically exclusive at the mutex level, with commit-word-last ordering.
/// Flash power-cut safety is governed by otadata::execute, not the mutex.
pub async fn execute(
    flash: &SharedFlash,
    partition: Partition,
    write: Write,
) -> Result<(), otadata::Error<<EspFlash as embedded_storage::nor_flash::ErrorType>::Error>> {
    let mut guard = flash.lock().await;
    otadata::execute(&mut LockedFlash(&mut guard), partition, write)
}

/// Snapshot both boot metadata entries under a single shared flash lock.
pub async fn read_entries(
    flash: &SharedFlash,
    partition: Partition,
) -> Result<
    [Raw; SECTOR_COUNT],
    otadata::Error<<EspFlash as embedded_storage::nor_flash::ErrorType>::Error>,
> {
    let mut guard = flash.lock().await;
    otadata::read_entries(&mut LockedFlash(&mut guard), partition)
}
