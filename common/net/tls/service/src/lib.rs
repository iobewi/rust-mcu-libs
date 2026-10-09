#![no_std]

//! Persistent server identity and outbound trust for TLS transports.
//! Cryptography comes from a `iobewi_crypto_core::TlsCrypto` implementation
//! and sockets from a [`client::TlsDialer`]; this crate names no platform type.

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod client;

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Debug;
use iobewi_config_space::{Budget, ConfigBackend, ConfigSpace};
use iobewi_crypto_core::{PairError, TlsCrypto};
use log::warn;

const MAGIC: &[u8; 4] = b"TLS1";
const HEADER_LEN: usize = 10;
const MAX_CA_LEN: usize = 2048;
const MAX_CERT_LEN: usize = 2048;
const MAX_KEY_LEN: usize = 2048;
pub const CONFIG_BUDGET: Budget = Budget::new(HEADER_LEN + MAX_CA_LEN + MAX_CERT_LEN + MAX_KEY_LEN);

#[derive(Clone, Default)]
struct TlsConfig {
    ca_pem: String,
    cert_pem: String,
    key_pem: String,
}

impl TlsConfig {
    fn encode(&self) -> Option<Vec<u8>> {
        if self.ca_pem.len() > MAX_CA_LEN
            || self.cert_pem.len() > MAX_CERT_LEN
            || self.key_pem.len() > MAX_KEY_LEN
        {
            return None;
        }
        let ca_len = u16::try_from(self.ca_pem.len()).ok()?;
        let cert_len = u16::try_from(self.cert_pem.len()).ok()?;
        let key_len = u16::try_from(self.key_pem.len()).ok()?;
        let total = HEADER_LEN
            .checked_add(self.ca_pem.len())?
            .checked_add(self.cert_pem.len())?
            .checked_add(self.key_pem.len())?;
        if total > CONFIG_BUDGET.max_bytes() {
            return None;
        }
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&ca_len.to_le_bytes());
        out.extend_from_slice(&cert_len.to_le_bytes());
        out.extend_from_slice(&key_len.to_le_bytes());
        out.extend_from_slice(self.ca_pem.as_bytes());
        out.extend_from_slice(self.cert_pem.as_bytes());
        out.extend_from_slice(self.key_pem.as_bytes());
        Some(out)
    }

    fn decode(raw: &[u8]) -> Option<Self> {
        if raw.len() < HEADER_LEN || &raw[..4] != MAGIC {
            return None;
        }
        let ca_len = u16::from_le_bytes([raw[4], raw[5]]) as usize;
        let cert_len = u16::from_le_bytes([raw[6], raw[7]]) as usize;
        let key_len = u16::from_le_bytes([raw[8], raw[9]]) as usize;
        if ca_len > MAX_CA_LEN || cert_len > MAX_CERT_LEN || key_len > MAX_KEY_LEN {
            return None;
        }
        let ca_end = HEADER_LEN.checked_add(ca_len)?;
        let cert_end = ca_end.checked_add(cert_len)?;
        let key_end = cert_end.checked_add(key_len)?;
        if key_end != raw.len() {
            return None;
        }
        Some(Self {
            ca_pem: String::from(core::str::from_utf8(&raw[HEADER_LEN..ca_end]).ok()?),
            cert_pem: String::from(core::str::from_utf8(&raw[ca_end..cert_end]).ok()?),
            key_pem: String::from(core::str::from_utf8(&raw[cert_end..key_end]).ok()?),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoadError {
    Storage,
    Corrupt,
}

async fn load_existing<B: ConfigBackend>(
    space: &ConfigSpace<B>,
) -> Result<Option<TlsConfig>, LoadError>
where
    B::Error: Debug,
{
    match space.load().await {
        Ok(Some(snapshot)) => TlsConfig::decode(&snapshot.data).map(Some).ok_or_else(|| {
            warn!(
                "tls: stored config generation={} has an unsupported/corrupt schema",
                snapshot.generation
            );
            LoadError::Corrupt
        }),
        Ok(None) => Ok(None),
        Err(e) => {
            warn!("tls: config-space load failed: {e:?}");
            Err(LoadError::Storage)
        }
    }
}

async fn load_config<B: ConfigBackend>(space: &ConfigSpace<B>) -> Option<TlsConfig>
where
    B::Error: Debug,
{
    match load_existing(space).await {
        Ok(Some(config)) => Some(config),
        Ok(None) => Some(TlsConfig::default()),
        Err(_) => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityBootstrapError {
    Storage,
    Corrupt,
    Generation,
    Invalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveCertError {
    Invalid,
    Mismatch,
    Storage,
}

/// Owns the durable TLS identity and trust policy. The application passes
/// its isolated ConfigSpace and the platform's crypto implementation.
pub struct TlsService<C> {
    crypto: C,
}

impl<C: TlsCrypto> TlsService<C> {
    pub const fn new(crypto: C) -> Self {
        Self { crypto }
    }

    fn valid_identity(&self, config: &TlsConfig) -> bool {
        !config.cert_pem.is_empty()
            && !config.key_pem.is_empty()
            && self
                .crypto
                .validate_pair(&config.cert_pem, &config.key_pem)
                .is_ok()
            && self
                .crypto
                .server_config(&config.cert_pem, &config.key_pem)
                .is_some()
    }

    /// Only an absent identity can be generated; corrupt stored data fails
    /// closed. Re-read after commit before exposing the network service.
    pub async fn ensure_server_identity<B: ConfigBackend>(
        &self,
        space: &ConfigSpace<B>,
        common_name: &str,
    ) -> Result<(), IdentityBootstrapError>
    where
        B::Error: Debug,
    {
        match load_existing(space).await {
            Ok(Some(config)) => {
                if config.cert_pem.is_empty() || config.key_pem.is_empty() {
                    return Err(IdentityBootstrapError::Corrupt);
                }
                return self
                    .valid_identity(&config)
                    .then_some(())
                    .ok_or(IdentityBootstrapError::Invalid);
            }
            Ok(None) => {}
            Err(LoadError::Storage) => return Err(IdentityBootstrapError::Storage),
            Err(LoadError::Corrupt) => return Err(IdentityBootstrapError::Corrupt),
        }
        let generated = self
            .crypto
            .generate_identity(common_name)
            .ok_or(IdentityBootstrapError::Generation)?;
        if generated.cert_pem.len() > MAX_CERT_LEN || generated.key_pem.len() > MAX_KEY_LEN {
            return Err(IdentityBootstrapError::Generation);
        }
        if self
            .crypto
            .validate_pair(&generated.cert_pem, &generated.key_pem)
            .is_err()
            || self
                .crypto
                .server_config(&generated.cert_pem, &generated.key_pem)
                .is_none()
        {
            return Err(IdentityBootstrapError::Invalid);
        }
        let config = TlsConfig {
            ca_pem: String::new(),
            cert_pem: generated.cert_pem,
            key_pem: generated.key_pem,
        };
        let encoded = config.encode().ok_or(IdentityBootstrapError::Storage)?;
        space
            .commit(&encoded)
            .await
            .map_err(|_| IdentityBootstrapError::Storage)?;
        match load_existing(space).await {
            Ok(Some(stored)) if self.valid_identity(&stored) => Ok(()),
            Ok(_) | Err(LoadError::Corrupt) => Err(IdentityBootstrapError::Corrupt),
            Err(LoadError::Storage) => Err(IdentityBootstrapError::Storage),
        }
    }

    pub async fn server_identity_valid<B: ConfigBackend>(&self, space: &ConfigSpace<B>) -> bool
    where
        B::Error: Debug,
    {
        matches!(load_existing(space).await, Ok(Some(config)) if self.valid_identity(&config))
    }

    pub async fn save_cert<B: ConfigBackend>(
        &self,
        space: &ConfigSpace<B>,
        cert_pem: &str,
        key_pem: &str,
    ) -> Result<(), SaveCertError>
    where
        B::Error: Debug,
    {
        self.crypto
            .validate_pair(cert_pem, key_pem)
            .map_err(|error| match error {
                PairError::Invalid => SaveCertError::Invalid,
                PairError::Mismatch => SaveCertError::Mismatch,
            })?;
        if self.crypto.server_config(cert_pem, key_pem).is_none() {
            return Err(SaveCertError::Invalid);
        }
        let mut config = load_config(space).await.ok_or(SaveCertError::Storage)?;
        config.cert_pem = String::from(cert_pem);
        config.key_pem = String::from(key_pem);
        let encoded = config.encode().ok_or(SaveCertError::Storage)?;
        space
            .commit(&encoded)
            .await
            .map_err(|_| SaveCertError::Storage)?;
        Ok(())
    }

    pub async fn server_config<B: ConfigBackend>(
        &self,
        space: &ConfigSpace<B>,
    ) -> Option<C::ServerConfig>
    where
        B::Error: Debug,
    {
        let config = load_config(space).await?;
        if config.cert_pem.is_empty() || config.key_pem.is_empty() {
            return None;
        }
        self.crypto.server_config(&config.cert_pem, &config.key_pem)
    }

    pub async fn save_ca<B: ConfigBackend>(
        &self,
        space: &ConfigSpace<B>,
        ca_pem: &str,
    ) -> Result<(), SaveCertError>
    where
        B::Error: Debug,
    {
        if !self.crypto.validate_ca(ca_pem) {
            return Err(SaveCertError::Invalid);
        }
        let mut config = load_config(space).await.ok_or(SaveCertError::Storage)?;
        config.ca_pem = String::from(ca_pem);
        let encoded = config.encode().ok_or(SaveCertError::Storage)?;
        space
            .commit(&encoded)
            .await
            .map_err(|_| SaveCertError::Storage)?;
        Ok(())
    }

    pub async fn trusted_ca<B: ConfigBackend>(&self, space: &ConfigSpace<B>) -> Option<String>
    where
        B::Error: Debug,
    {
        trusted_ca(space).await
    }
}

/// The durable CA, if one has been provisioned. Needs no crypto: validation
/// happened when it was saved.
pub async fn trusted_ca<B: ConfigBackend>(space: &ConfigSpace<B>) -> Option<String>
where
    B::Error: Debug,
{
    let config = load_config(space).await?;
    (!config.ca_pem.is_empty()).then_some(config.ca_pem)
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod legacy_codec_tests {
    use super::*;

    #[test]
    fn legacy_tls1_identity_and_ca_round_trip() {
        let bytes = b"TLS1\x02\x00\x04\x00\x03\x00CAcertkey";
        let config = TlsConfig::decode(bytes).unwrap();
        assert_eq!(config.ca_pem, "CA");
        assert_eq!(config.cert_pem, "cert");
        assert_eq!(config.key_pem, "key");
        assert_eq!(config.encode().unwrap(), bytes);
        assert!(TlsConfig::decode(b"TLS1\x02\x00\x04\x00\x03\x00CAcertke").is_none());
    }
}
