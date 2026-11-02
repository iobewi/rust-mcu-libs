//! SHA-256 on the SHA peripheral through the `esp-hal` driver, an alternative to the ROM's
//! (`rom_verify::RomSha256`). Blocking, no interrupt and no DMA.
//!
//! The driver counts the users of the peripheral: the first one switches the clock on and pulses
//! the reset, the last one switches the clock off. The ROM's RSA-PSS check enables, uses and
//! *disables* the same peripheral behind that count, so a hash must never be alive across it:
//! each `HalSha256` takes the peripheral when it is created and gives it back when it is
//! finished or dropped, like the ROM adapter, and `RomRsaPss3072::verify` finishes its own
//! before calling the ROM.

use esp_hal::peripherals::SHA;
use esp_hal::sha::{Sha, Sha256, ShaDigest};
use iobewi_firmware_bootauth::Sha256Stream;

pub struct HalSha256 {
    digest: ShaDigest<'static, Sha256, Sha<'static>>,
}

impl Sha256Stream for HalSha256 {
    fn new() -> Self {
        // SAFETY: sole boot phase. The peripheral is taken here and released with the hash; the
        // only other user of it is the ROM's RSA-PSS check, which runs between two hashes.
        let sha = Sha::new(unsafe { SHA::steal() });
        Self {
            digest: sha.start_owned::<Sha256>(),
        }
    }

    fn update(&mut self, mut data: &[u8]) {
        while !data.is_empty() {
            let Ok(rest) = nb::block!(self.digest.update(data));
            data = rest;
        }
    }

    fn finish(mut self) -> [u8; 32] {
        let mut out = [0u8; 32];
        let Ok(()) = nb::block!(self.digest.finish(&mut out));
        out
    }
}
