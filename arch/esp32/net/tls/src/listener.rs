//! ESP TLS listener: a `net/tls` [`TlsListener`] over MbedTLS and the ESP TCP
//! transport. It yields generic `net/io` connections; the HTTP server adapts
//! them (this crate names no HTTP framework). HTTPS is this listener plus the
//! HTTP server, composed by the application -- there is no "https" driver.

use crate::mbedtls_rs::{Session, SessionConfig, SessionError};
use crate::{TlsReferenceStatic, embassy::TlsStream};
use embassy_net::Stack;
use embassy_time::{Duration, Timer, with_timeout};
use iobewi_esp_tcp::EspTcpListener;
use iobewi_net_io::ConnectionListener;
use iobewi_net_tls_core::TlsListener;
use log::{debug, warn};

const ADMIN_PORT_HTTPS: u16 = 443;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// ESP adapter implementing the `net/tls` `TlsListener` over MbedTLS and
/// the ESP TCP stack. `identity` reads the current server certificate and
/// key through an application-provided capability; a missing identity never
/// falls back to unencrypted HTTP.
///
/// `rx`/`tx` are caller-owned so this type never borrows a buffer that could
/// go out of scope before the listener itself does -- construct it (and its
/// backing buffers) at the composition boundary that also never returns,
/// e.g. an embassy task, and hand it to a generic `serve` loop from there.
pub struct EspTlsListener<'a, LoadIdentity> {
    tcp: EspTcpListener<'a>,
    tls: TlsReferenceStatic,
    identity: LoadIdentity,
    config: Option<SessionConfig<'static>>,
}

impl<'a, LoadIdentity> EspTlsListener<'a, LoadIdentity>
where
    LoadIdentity: AsyncFn() -> Option<SessionConfig<'static>>,
{
    pub fn new(
        stack: Stack<'static>,
        tls: TlsReferenceStatic,
        identity: LoadIdentity,
        rx: &'a mut [u8],
        tx: &'a mut [u8],
    ) -> Self {
        Self {
            tcp: EspTcpListener::new(stack, ADMIN_PORT_HTTPS, rx, tx),
            tls,
            identity,
            config: None,
        }
    }
}

impl<LoadIdentity> ConnectionListener for EspTlsListener<'_, LoadIdentity>
where
    LoadIdentity: AsyncFn() -> Option<SessionConfig<'static>>,
{
    type Connection<'a>
        = TlsStream<'a, 'a>
    where
        Self: 'a;

    async fn accept(&mut self) -> Result<Self::Connection<'_>, ()> {
        let Some(config) = (self.identity)().await else {
            warn!("HTTPS: no usable server identity; administrative surface remains closed");
            Timer::after(Duration::from_secs(5)).await;
            return Err(());
        };
        self.config = Some(config);

        let mut socket = self.tcp.accept_connection().await?;
        socket.set_keep_alive(Some(Duration::from_secs(30)));
        socket.set_timeout(Some(Duration::from_secs(45)));

        let Some(server_config) = self.config.as_ref() else {
            return Err(());
        };
        let mut session = match Session::new(self.tls, socket, server_config) {
            Ok(session) => session,
            Err(e) => {
                warn!("HTTPS: session setup failed: {e}");
                return Err(());
            }
        };
        match with_timeout(HANDSHAKE_TIMEOUT, session.connect()).await {
            Ok(Ok(())) => Ok(TlsStream::server(session)),
            Ok(Err(e)) if is_peer_hangup(&e) => {
                debug!("HTTPS: handshake aborted by peer: {e}");
                Err(())
            }
            Ok(Err(e)) => {
                warn!("HTTPS: handshake failed: {e}");
                Err(())
            }
            Err(_) => {
                warn!("HTTPS: handshake timed out after {HANDSHAKE_TIMEOUT:?}");
                Err(())
            }
        }
    }
}

/// Only a completed handshake yields a connection; there is no plaintext path.
impl<LoadIdentity> TlsListener for EspTlsListener<'_, LoadIdentity> where
    LoadIdentity: AsyncFn() -> Option<SessionConfig<'static>>
{
}

fn is_peer_hangup(e: &SessionError) -> bool {
    const MBEDTLS_ERR_SSL_FATAL_ALERT_MESSAGE: i32 = -0x7780;
    matches!(e, SessionError::MbedTls(m) if m.code() == MBEDTLS_ERR_SSL_FATAL_ALERT_MESSAGE)
}
