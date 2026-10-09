#![no_std]

//! Crypto contract for TLS material. It validates and generates X.509
//! identities; it never touches sockets, persistence or a network stack.
//! `iobewi-crypto-mbedtls` implements it; `iobewi-tls-service` consumes it.

extern crate alloc;

use alloc::string::String;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairError {
    /// The certificate or the private key does not parse.
    Invalid,
    /// Both parse but the key is not the certificate's key.
    Mismatch,
}

pub struct Identity {
    pub cert_pem: String,
    pub key_pem: String,
}

/// Platform crypto boundary. The TLS service requires validation before it
/// stores an identity, but never refers to MbedTLS or an ESP peripheral.
pub trait TlsCrypto {
    type ServerConfig;
    fn validate_pair(&self, cert: &str, key: &str) -> Result<(), PairError>;
    fn server_config(&self, cert: &str, key: &str) -> Option<Self::ServerConfig>;
    fn generate_identity(&self, common_name: &str) -> Option<Identity>;
    fn validate_ca(&self, ca: &str) -> bool;
}
