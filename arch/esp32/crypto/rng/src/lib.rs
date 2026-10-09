#![no_std]

use iobewi_entropy::EntropySource;

/// ESP hardware RNG implementation of IOBEWI's portable entropy capability.
#[derive(Clone, Copy, Default)]
pub struct EspEntropySource;

impl EntropySource for EspEntropySource {
    fn fill_random(&self, output: &mut [u8]) {
        esp_hal::rng::Rng::new().read(output);
    }
}

/// The same hardware RNG through `rand_core`'s cryptographic-RNG traits, as
/// required by `mbedtls-rs` (`Tls::new`). One hardware source, two faces:
/// [`EspEntropySource`] for IOBEWI capabilities, this for MbedTLS.
#[cfg(feature = "rand-core")]
pub struct EspCryptoRng(esp_hal::rng::Rng);

#[cfg(feature = "rand-core")]
impl EspCryptoRng {
    pub fn new() -> Self {
        Self(esp_hal::rng::Rng::new())
    }
}

#[cfg(feature = "rand-core")]
impl Default for EspCryptoRng {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "rand-core")]
impl rand_core::TryRng for EspCryptoRng {
    type Error = core::convert::Infallible;

    fn try_next_u32(&mut self) -> Result<u32, Self::Error> {
        Ok(self.0.random())
    }

    fn try_next_u64(&mut self) -> Result<u64, Self::Error> {
        let mut bytes = [0u8; 8];
        self.0.read(&mut bytes);
        Ok(u64::from_le_bytes(bytes))
    }

    fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), Self::Error> {
        self.0.read(dst);
        Ok(())
    }
}

#[cfg(feature = "rand-core")]
impl rand_core::TryCryptoRng for EspCryptoRng {}
