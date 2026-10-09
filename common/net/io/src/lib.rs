#![cfg_attr(not(test), no_std)]
#![allow(async_fn_in_trait)]

//! Low-level connection contracts.
//!
//! This crate describes connection and I/O capabilities, not an application
//! protocol: it knows nothing about HTTP, WebSocket, TLS, Wi-Fi, ConfigSpace,
//! `picoserve` or any platform. Byte streams are `embedded-io-async`
//! [`Read`] + [`Write`]; this crate adds only what that standard does not
//! express:
//!
//! - [`Close`]: a clean shutdown of a connection;
//! - [`Connection`]: the bundle `Read + Write + Close`;
//! - [`ConnectionListener`]: wait for and accept an inbound connection;
//! - [`Connector`]: open an outbound connection to `host:port`.
//!
//! What a connection *guarantees* (for example "authenticated and encrypted")
//! is deliberately not expressed here: a connector that promises it says so
//! with a refinement trait in the owning subsystem (see
//! `iobewi-net-tls-core`).

extern crate alloc;

pub use embedded_io_async::{ErrorType, Read, Write};

/// A connection that can be shut down cleanly (e.g. a TLS close-notify, or a
/// TCP FIN) instead of an abrupt reset. Kept separate from `Read`/`Write`
/// because not every consumer needs it -- a client that just drops the
/// connection on reconnect has no use for this.
///
/// Its error is the connection's I/O error (`ErrorType::Error`): every
/// implementation so far already used the same type for both.
pub trait Close: ErrorType {
    async fn close(&mut self) -> Result<(), Self::Error>;
}

/// A bidirectional byte stream that can be shut down cleanly.
pub trait Connection: Read + Write + Close {}

impl<T: Read + Write + Close> Connection for T {}

/// Accepts inbound connections.
///
/// `accept` returns an error without detail (`()`): the listener owns its own
/// retry delay and any platform log, the caller only needs to know that no
/// connection was produced.
pub trait ConnectionListener {
    type Connection<'a>: Connection
    where
        Self: 'a;

    async fn accept(&mut self) -> Result<Self::Connection<'_>, ()>;
}

/// Opens outbound connections. An implementation owns name resolution and
/// connection establishment (and whatever policy its type promises); the
/// caller only ever sees `Ok(Connection)` or `Err(Error)`.
pub trait Connector {
    /// Failure connecting. Meant to be logged (`Display`), not matched on
    /// across a crate boundary -- callers don't need to know which
    /// platform-specific step failed.
    type Error: core::fmt::Display;

    type Connection<'a>: Connection
    where
        Self: 'a;

    /// Connects to `host:port`. `rx`/`tx` back the connection's own
    /// transport-layer buffers for as long as the connection is used.
    async fn connect<'a>(
        &'a self,
        host: &'a str,
        port: u16,
        rx: &'a mut [u8],
        tx: &'a mut [u8],
    ) -> Result<Self::Connection<'a>, Self::Error>;

    /// Best-effort description of the local address this connector is
    /// currently reachable on (e.g. for status reporting in an outbound
    /// payload). `None` when no address is assigned yet. Defaulted: most
    /// consumers of `connect()` have no use for their own address, so an
    /// implementation only needs to override this when something (like a
    /// heartbeat) actually reports it.
    fn local_address(&self) -> Option<alloc::string::String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::cell::RefCell;
    use core::convert::Infallible;
    use core::future::Future;
    use core::task::{Context, Poll, Waker};
    use std::collections::VecDeque;

    fn block_on<F: Future>(future: F) -> F::Output {
        let waker = Waker::noop();
        let mut cx = Context::from_waker(waker);
        let mut future = core::pin::pin!(future);

        loop {
            match future.as_mut().poll(&mut cx) {
                Poll::Ready(value) => return value,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    /// In-memory loopback connection: whatever is written is what the next
    /// read returns, byte for byte. Enough to exercise the traits' shape
    /// without a real network stack.
    #[derive(Debug)]
    struct Loopback(RefCell<VecDeque<u8>>);

    impl embedded_io_async::ErrorType for Loopback {
        type Error = Infallible;
    }

    impl Read for Loopback {
        async fn read(&mut self, out: &mut [u8]) -> Result<usize, Infallible> {
            let mut buf = self.0.borrow_mut();
            let n = out.len().min(buf.len());
            for slot in out.iter_mut().take(n) {
                *slot = buf.pop_front().expect("checked by min() above");
            }
            Ok(n)
        }
    }

    impl Write for Loopback {
        async fn write(&mut self, data: &[u8]) -> Result<usize, Infallible> {
            self.0.borrow_mut().extend(data.iter().copied());
            Ok(data.len())
        }

        async fn flush(&mut self) -> Result<(), Infallible> {
            Ok(())
        }
    }

    impl Close for Loopback {
        async fn close(&mut self) -> Result<(), Infallible> {
            Ok(())
        }
    }

    struct AlwaysConnects;

    impl Connector for AlwaysConnects {
        type Error = &'static str;
        type Connection<'a>
            = Loopback
        where
            Self: 'a;

        async fn connect<'a>(
            &'a self,
            host: &'a str,
            _port: u16,
            _rx: &'a mut [u8],
            _tx: &'a mut [u8],
        ) -> Result<Self::Connection<'a>, Self::Error> {
            if host.is_empty() {
                return Err("empty host refused");
            }
            Ok(Loopback(RefCell::new(VecDeque::new())))
        }
    }

    #[test]
    fn connect_then_round_trip_and_close() {
        let connector = AlwaysConnects;
        let mut rx = [0u8; 16];
        let mut tx = [0u8; 16];
        let mut conn = block_on(connector.connect("example.test", 443, &mut rx, &mut tx)).unwrap();

        block_on(conn.write(b"ping")).unwrap();
        let mut out = [0u8; 4];
        block_on(conn.read(&mut out)).unwrap();
        assert_eq!(&out, b"ping");
        block_on(conn.close()).unwrap();
    }

    #[test]
    fn connect_reports_the_implementation_error_without_a_connection() {
        let connector = AlwaysConnects;
        let mut rx = [0u8; 16];
        let mut tx = [0u8; 16];
        let err = block_on(connector.connect("", 443, &mut rx, &mut tx)).unwrap_err();
        assert_eq!(err, "empty host refused");
    }

    #[test]
    fn local_address_defaults_to_none() {
        assert_eq!(AlwaysConnects.local_address(), None);
    }

    /// Hands out each queued connection once, then reports "nothing produced".
    struct Queue(VecDeque<Loopback>);

    impl ConnectionListener for Queue {
        type Connection<'a>
            = Loopback
        where
            Self: 'a;

        async fn accept(&mut self) -> Result<Self::Connection<'_>, ()> {
            self.0.pop_front().ok_or(())
        }
    }

    #[test]
    fn listener_yields_connections_then_reports_none() {
        let mut listener = Queue(VecDeque::from([Loopback(RefCell::new(VecDeque::new()))]));
        let mut conn = block_on(listener.accept()).unwrap();
        block_on(conn.write(b"hi")).unwrap();
        let mut out = [0u8; 2];
        block_on(conn.read(&mut out)).unwrap();
        assert_eq!(&out, b"hi");
        assert!(block_on(listener.accept()).is_err());
    }
}
