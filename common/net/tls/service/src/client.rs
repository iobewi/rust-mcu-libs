//! Fail-closed secure outbound connector.
//!
//! The platform supplies a [`TlsDialer`] (`iobewi-net-tls-core`: DNS + TCP + a certificate-verifying
//! handshake against a given CA). This module adds the policy that makes the
//! result a `SecureClientTransport`: never dial before the wall clock is set
//! (certificate dates cannot be checked) and never dial without a durable CA.
//! Both refusals happen *before* any network activity.

use alloc::string::String;
use core::fmt::{Debug, Display};
use iobewi_config_space::{ConfigBackend, ConfigSpace};
use iobewi_net_io::Connector;
use iobewi_net_tls_core::{SecureClientTransport, TlsDialer};

/// Why a secure outbound connection was not established.
#[derive(Debug)]
pub enum ClientTlsError<E> {
    /// The wall clock has not been synchronized yet.
    ClockUnsynced,
    /// No CA has been provisioned.
    NoCa,
    /// The dialer failed (DNS, TCP, bad CA, handshake/authentication).
    Connect(E),
}

impl<E: Display> Display for ClientTlsError<E> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::ClockUnsynced => write!(f, "clock not synchronized yet (SNTP)"),
            Self::NoCa => write!(f, "no CA configured (POST /v1alpha1/tls/ca)"),
            Self::Connect(e) => write!(f, "{e}"),
        }
    }
}

/// Secure outbound connector. Trust material (the durable CA's config space,
/// whether the clock has converged) is bound at construction -- `connect()`
/// takes only `host`/`port`/buffers, so every call gets the same fail-closed
/// policy without the caller having to know it exists.
#[derive(Clone, Copy)]
pub struct SecureConnector<D, B: ConfigBackend + 'static> {
    pub dialer: D,
    pub tls_config: &'static ConfigSpace<B>,
    pub clock_is_set: fn() -> bool,
}

impl<D: TlsDialer, B> Connector for SecureConnector<D, B>
where
    B: ConfigBackend + 'static,
    B::Error: Debug,
{
    type Error = ClientTlsError<D::Error>;
    type Connection<'a>
        = D::Connection<'a>
    where
        Self: 'a;

    async fn connect<'a>(
        &'a self,
        host: &'a str,
        port: u16,
        rx: &'a mut [u8],
        tx: &'a mut [u8],
    ) -> Result<Self::Connection<'a>, Self::Error> {
        if !(self.clock_is_set)() {
            return Err(ClientTlsError::ClockUnsynced);
        }
        let ca = crate::trusted_ca(self.tls_config)
            .await
            .ok_or(ClientTlsError::NoCa)?;
        self.dialer
            .dial(host, port, &ca, rx, tx)
            .await
            .map_err(ClientTlsError::Connect)
    }

    fn local_address(&self) -> Option<String> {
        self.dialer.local_address()
    }
}

/// Every connection returned is authenticated and encrypted: the dialer
/// verifies against the durable CA, and the clock/CA preconditions fail
/// closed before any dial.
impl<D: TlsDialer, B> SecureClientTransport for SecureConnector<D, B>
where
    B: ConfigBackend + 'static,
    B::Error: Debug,
{
}
