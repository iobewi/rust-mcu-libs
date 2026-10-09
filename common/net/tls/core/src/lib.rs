#![no_std]
#![allow(async_fn_in_trait)]

//! TLS network contracts, free of identity/ConfigSpace policy.
//!
//! The guarantees carried by a secure outbound connector
//! ([`SecureClientTransport`]) and by a secure inbound listener
//! ([`TlsListener`]).

extern crate alloc;

use alloc::string::String;
use core::fmt::Display;
use iobewi_net_io::{Connection, ConnectionListener, Connector};

/// A [`Connector`] that promises every connection it returns is
/// authenticated and encrypted, with a fail-closed trust policy.
///
/// A caller asks for a connection to `host:port` and gets back such a stream
/// without ever knowing how DNS, TCP, the TLS handshake, certificates, the
/// clock, or the underlying network stack are implemented. Those are the
/// implementation's own construction-time concern -- trust material (CA,
/// clock) is injected when the implementation is built, never passed to
/// `connect()` itself, so the same fail-closed policy applies to every call
/// without the caller having to know it exists.
///
/// This is a marker: it adds no methods. Implementors opt in explicitly, so a
/// plaintext connector can never satisfy a bound on `SecureClientTransport`
/// by accident.
///
/// This crate deliberately does not cover:
/// - identity/CA/crypto policy -- see `iobewi-tls-service` and
///   `iobewi-crypto-core`'s `TlsCrypto`;
/// - entropy -- not a property of a secure transport; a consumer that needs
///   it (e.g. WebSocket frame masking) asks for it as its own capability.
pub trait SecureClientTransport: Connector {}

/// A [`ConnectionListener`] that promises every connection it accepts has
/// completed a TLS handshake with the server identity: only an encrypted,
/// server-authenticated stream is ever returned, and a missing identity or a
/// failed handshake yields `Err` -- never a plaintext connection.
///
/// A marker, like [`SecureClientTransport`]: it adds no methods, so a
/// plaintext listener cannot satisfy a `TlsListener` bound by accident. The
/// accepted connection is the generic `net/io` stream (`Read + Write +
/// Close`); any protocol server (HTTP, ...) adapts it as it needs, so this
/// contract mentions no protocol or framework type. The listener owns the
/// retry delay and logging: an `Err` from `accept` just means "no connection
/// this time".
pub trait TlsListener: ConnectionListener {}

/// Resolve, connect and complete a certificate-verifying TLS handshake.
/// Policy ("clock not synchronized", "CA not provisioned") is not the
/// dialer's concern: it receives the CA and verifies against it.
#[allow(async_fn_in_trait)]
pub trait TlsDialer {
    type Error: Display;
    type Connection<'a>: Connection
    where
        Self: 'a;

    async fn dial<'a>(
        &'a self,
        host: &'a str,
        port: u16,
        ca_pem: &str,
        rx: &'a mut [u8],
        tx: &'a mut [u8],
    ) -> Result<Self::Connection<'a>, Self::Error>;

    /// Best-effort local address, see `Connector::local_address`.
    fn local_address(&self) -> Option<String> {
        None
    }
}
