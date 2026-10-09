//! DNS/TCP/TLS over `embassy-net`: the client dialer and the connected-session
//! stream shared by client and server.

use alloc::ffi::CString;
use alloc::string::String;

use crate::mbedtls_rs::{
    Certificate, ClientSessionConfig, Session, SessionConfig, SessionError, TlsReference, X509,
};
use embassy_net::tcp::{ConnectError, TcpSocket};
use embedded_io_async::{ErrorType, Read, Write};
use iobewi_net_io::Close;
use iobewi_net_tls_core::TlsDialer;

use crate::TlsReferenceStatic;

/// Failures in DNS/TCP/TLS establishment. Application policy such as
/// "clock not synchronized" or "CA not provisioned" intentionally stays
/// outside this crate (`iobewi-tls-service`). `Display` texts are the
/// operator-facing log messages and are unchanged from before the split.
#[derive(Debug)]
pub enum ClientConnectError {
    BadCa,
    /// `host` contains an embedded NUL byte, so it cannot be turned into
    /// the C string MbedTLS' hostname/SNI setter requires.
    InvalidHost,
    Dns,
    Tcp(ConnectError),
    Handshake(SessionError),
}

impl core::fmt::Display for ClientConnectError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::BadCa => write!(f, "stored CA failed to parse"),
            Self::InvalidHost => write!(f, "host name contains a nul byte"),
            Self::Dns => write!(f, "DNS resolution failed"),
            Self::Tcp(e) => write!(f, "TCP connect failed: {e:?}"),
            Self::Handshake(e) => write!(f, "TLS handshake failed: {e}"),
        }
    }
}

/// Resolve, connect, and complete a certificate-verifying client TLS
/// handshake over Embassy networking.
///
/// Returns `Session<'static, _>`: nothing this function passes into
/// `ClientSessionConfig` needs to outlive the call (the CA is parsed into
/// owned, `'static` storage by `Certificate::new`, and the hostname is
/// set *after* construction via `set_server_name`, which MbedTLS copies
/// into its own allocation immediately -- see that call below). Only the
/// TCP buffers' lifetime survives into the returned type.
pub async fn connect_client<'buf>(
    tls: TlsReference<'static>,
    stack: embassy_net::Stack<'static>,
    rx_buffer: &'buf mut [u8],
    tx_buffer: &'buf mut [u8],
    host: &str,
    port: u16,
    ca_pem: &str,
) -> Result<Session<'static, TcpSocket<'buf>>, ClientConnectError> {
    let ca_c = CString::new(ca_pem).map_err(|_| ClientConnectError::BadCa)?;
    let ca_chain = Certificate::new(X509::PEM(&ca_c)).map_err(|_| ClientConnectError::BadCa)?;

    let dns = embassy_net::dns::DnsSocket::new(stack);
    let ip = dns
        .query(host, embassy_net::dns::DnsQueryType::A)
        .await
        .ok()
        .and_then(|addrs| addrs.into_iter().next())
        .ok_or(ClientConnectError::Dns)?;

    let mut socket = TcpSocket::new(stack, rx_buffer, tx_buffer);
    socket
        .connect((ip, port))
        .await
        .map_err(ClientConnectError::Tcp)?;

    // No `server_name` here (unlike the config's other fields, that one
    // would force this function's returned `Session<'a, _>` down to the
    // lifetime of a *local* `host_c` -- see `set_server_name` below,
    // called separately once the session already exists).
    let config = SessionConfig::Client(ClientSessionConfig {
        ca_chain: Some(ca_chain),
        ..ClientSessionConfig::new()
    });
    let mut session = Session::new(tls, socket, &config).map_err(ClientConnectError::Handshake)?;
    // Must happen before `connect()`/the handshake (required by
    // `set_server_name`'s own contract); the local `host_c` only needs
    // to live for this one synchronous call, since MbedTLS copies the
    // hostname into its own allocation right away.
    let host_c = CString::new(host).map_err(|_| ClientConnectError::InvalidHost)?;
    session
        .set_server_name(&host_c)
        .map_err(ClientConnectError::Handshake)?;
    session
        .connect()
        .await
        .map_err(ClientConnectError::Handshake)?;
    Ok(session)
}

/// A connected MbedTLS session over an Embassy TCP socket, as a `net/io`
/// connection. Protocol code depends on the async I/O traits only.
///
/// `close` is the graceful shutdown: TLS close-notify, then TCP FIN. A
/// *server* stream that is dropped without having been closed aborts the TCP
/// connection (RST) -- the abort a timed-out or failed request needs.
pub struct TlsStream<'tls, 'buf> {
    session: Session<'tls, TcpSocket<'buf>>,
    abort_on_drop: bool,
    closed: bool,
}

impl<'tls, 'buf> TlsStream<'tls, 'buf> {
    /// A client session: dropping it unclosed just releases the socket.
    pub fn client(session: Session<'tls, TcpSocket<'buf>>) -> Self {
        Self {
            session,
            abort_on_drop: false,
            closed: false,
        }
    }

    /// A server session: dropping it unclosed aborts the TCP connection.
    pub fn server(session: Session<'tls, TcpSocket<'buf>>) -> Self {
        Self {
            session,
            abort_on_drop: true,
            closed: false,
        }
    }
}

impl Drop for TlsStream<'_, '_> {
    fn drop(&mut self) {
        if self.abort_on_drop && !self.closed {
            self.session.stream().abort();
        }
    }
}

impl ErrorType for TlsStream<'_, '_> {
    type Error = SessionError;
}

impl Read for TlsStream<'_, '_> {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, SessionError> {
        self.session.read(buf).await
    }
}

impl Write for TlsStream<'_, '_> {
    async fn write(&mut self, buf: &[u8]) -> Result<usize, SessionError> {
        self.session.write(buf).await
    }

    async fn flush(&mut self) -> Result<(), SessionError> {
        self.session.flush().await
    }
}

impl Close for TlsStream<'_, '_> {
    async fn close(&mut self) -> Result<(), SessionError> {
        self.closed = true;
        let result = self.session.close().await;
        self.session.stream().close();
        result
    }
}

/// The ESP implementation of [`TlsDialer`]: Embassy DNS + TCP and an MbedTLS
/// handshake verified against the CA it is handed.
#[derive(Clone, Copy)]
pub struct EspTlsDialer {
    pub tls: TlsReferenceStatic,
    pub stack: embassy_net::Stack<'static>,
}

impl TlsDialer for EspTlsDialer {
    type Error = ClientConnectError;
    type Connection<'a>
        = TlsStream<'static, 'a>
    where
        Self: 'a;

    async fn dial<'a>(
        &'a self,
        host: &'a str,
        port: u16,
        ca_pem: &str,
        rx: &'a mut [u8],
        tx: &'a mut [u8],
    ) -> Result<Self::Connection<'a>, Self::Error> {
        connect_client(self.tls, self.stack, rx, tx, host, port, ca_pem)
            .await
            .map(TlsStream::client)
    }

    fn local_address(&self) -> Option<String> {
        self.stack
            .config_v4()
            .map(|c| alloc::format!("{}", c.address.address()))
    }
}
