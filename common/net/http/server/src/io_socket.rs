//! Adapter: any `net/io` connection as a picoserve socket.
//!
//! picoserve wants to split a socket into concurrently usable read and write
//! halves; a generic `Read + Write` stream cannot be split, so both halves
//! share the connection behind an async mutex (the same technique the TLS
//! session adapter uses). `shutdown` is the connection's `Close`; `abort`
//! drops it.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use iobewi_net_io::{Connection, ErrorType, Read, Write};
use picoserve::mem::BorrowedBuffer;
use picoserve::time::Timer;
use picoserve::{EmbassyRuntime, Timeouts};

/// The I/O error of an [`IoSocket`]: the `kind` of the underlying
/// connection's error. picoserve needs a `'static` socket error; a generic
/// connection's own error type may borrow, so only its kind crosses over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IoError(pub embedded_io_async::ErrorKind);

impl core::fmt::Display for IoError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "connection I/O error: {:?}", self.0)
    }
}

impl core::error::Error for IoError {}

impl embedded_io_async::Error for IoError {
    fn kind(&self) -> embedded_io_async::ErrorKind {
        self.0
    }
}

fn io_error<E: embedded_io_async::Error>(error: E) -> IoError {
    IoError(error.kind())
}

/// A `net/io` connection presented as a picoserve socket.
pub struct IoSocket<C> {
    connection: Mutex<CriticalSectionRawMutex, C>,
}

impl<C> IoSocket<C> {
    pub fn new(connection: C) -> Self {
        Self {
            connection: Mutex::new(connection),
        }
    }
}

/// One half (read or write) of an [`IoSocket`].
pub struct IoHalf<'a, C> {
    connection: &'a Mutex<CriticalSectionRawMutex, C>,
}

impl<C> ErrorType for IoHalf<'_, C> {
    type Error = IoError;
}

impl<C: Read> Read for IoHalf<'_, C> {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        self.connection
            .lock()
            .await
            .read(buf)
            .await
            .map_err(io_error)
    }
}

impl<C: Write> Write for IoHalf<'_, C> {
    async fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        self.connection
            .lock()
            .await
            .write(buf)
            .await
            .map_err(io_error)
    }

    async fn flush(&mut self) -> Result<(), Self::Error> {
        self.connection.lock().await.flush().await.map_err(io_error)
    }
}

impl<C: Write> picoserve::io::Write for IoHalf<'_, C> {
    async fn write_with<F: FnOnce(picoserve::mem::BorrowedCursor<'_>) -> R, R>(
        &mut self,
        f: F,
    ) -> Result<R, Self::Error> {
        let mut buffer = [0u8; 1024];
        let mut buffer = BorrowedBuffer::new(&mut buffer);
        let output = f(buffer.unfilled());
        self.connection
            .lock()
            .await
            .write_all(buffer.filled())
            .await
            .map_err(io_error)?;
        Ok(output)
    }
}

impl<C: Connection> picoserve::io::Socket<EmbassyRuntime> for IoSocket<C> {
    type Error = IoError;
    type ReadHalf<'b>
        = IoHalf<'b, C>
    where
        Self: 'b;
    type WriteHalf<'b>
        = IoHalf<'b, C>
    where
        Self: 'b;

    fn split(&mut self) -> (Self::ReadHalf<'_>, Self::WriteHalf<'_>) {
        (
            IoHalf {
                connection: &self.connection,
            },
            IoHalf {
                connection: &self.connection,
            },
        )
    }

    async fn abort<T: Timer<EmbassyRuntime>>(
        self,
        _timeouts: &Timeouts,
        _timer: &T,
    ) -> Result<(), picoserve::Error<Self::Error>> {
        // Dropping the connection is the only abort a generic stream has.
        Ok(())
    }

    async fn shutdown<T: Timer<EmbassyRuntime>>(
        self,
        _timeouts: &Timeouts,
        _timer: &T,
    ) -> Result<(), picoserve::Error<Self::Error>> {
        self.connection
            .into_inner()
            .close()
            .await
            .map_err(|e| picoserve::Error::Write(io_error(e)))
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use alloc::collections::VecDeque;
    use alloc::rc::Rc;
    use alloc::vec::Vec;
    use core::cell::RefCell;
    use core::future::Future;
    use core::task::{Context, Poll, Waker};
    use iobewi_net_io::Close;
    use picoserve::Timeouts;
    use picoserve::io::Socket;
    use picoserve::time::EmbassyTimer;

    /// Polls to completion; the in-memory connection below is always ready,
    /// so only the (never-firing) picoserve timers can yield `Pending`.
    fn block_on<F: Future>(future: F) -> F::Output {
        let mut future = core::pin::pin!(future);
        let mut cx = Context::from_waker(Waker::noop());
        loop {
            match future.as_mut().poll(&mut cx) {
                Poll::Ready(value) => return value,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    #[derive(Debug, Clone, Copy)]
    struct Reset;

    impl core::fmt::Display for Reset {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            write!(f, "reset")
        }
    }

    impl core::error::Error for Reset {}

    impl embedded_io_async::Error for Reset {
        fn kind(&self) -> embedded_io_async::ErrorKind {
            embedded_io_async::ErrorKind::ConnectionReset
        }
    }

    #[derive(Default)]
    struct State {
        input: VecDeque<u8>,
        output: Vec<u8>,
        max_write: Option<usize>,
        fail_read: bool,
        fail_close: bool,
        close_calls: usize,
    }

    /// A connection whose state stays inspectable after `IoSocket` consumes it.
    #[derive(Clone, Default)]
    struct Conn(Rc<RefCell<State>>);

    impl ErrorType for Conn {
        type Error = Reset;
    }

    impl Read for Conn {
        async fn read(&mut self, out: &mut [u8]) -> Result<usize, Reset> {
            let mut st = self.0.borrow_mut();
            if st.fail_read {
                return Err(Reset);
            }
            let n = out.len().min(st.input.len());
            for slot in out.iter_mut().take(n) {
                *slot = st.input.pop_front().unwrap();
            }
            Ok(n) // 0 = end of stream
        }
    }

    impl Write for Conn {
        async fn write(&mut self, data: &[u8]) -> Result<usize, Reset> {
            let mut st = self.0.borrow_mut();
            let n = st.max_write.map_or(data.len(), |m| m.min(data.len()));
            st.output.extend_from_slice(&data[..n]);
            Ok(n)
        }

        async fn flush(&mut self) -> Result<(), Reset> {
            Ok(())
        }
    }

    impl Close for Conn {
        async fn close(&mut self) -> Result<(), Reset> {
            let mut st = self.0.borrow_mut();
            st.close_calls += 1;
            if st.fail_close { Err(Reset) } else { Ok(()) }
        }
    }

    #[test]
    fn halves_share_the_connection() {
        let conn = Conn::default();
        let state = conn.0.clone();
        let mut socket = IoSocket::new(conn);
        let (mut rx, mut tx) = socket.split();
        state.borrow_mut().input.extend(b"ping");
        let mut out = [0u8; 4];
        assert_eq!(block_on(rx.read(&mut out)).unwrap(), 4);
        assert_eq!(&out, b"ping");
        block_on(Write::write(&mut tx, b"pong")).unwrap();
        assert_eq!(state.borrow().output, b"pong");
    }

    #[test]
    fn end_of_stream_is_zero_and_errors_keep_their_kind() {
        let conn = Conn::default();
        let state = conn.0.clone();
        let mut socket = IoSocket::new(conn);
        let (mut rx, _tx) = socket.split();
        let mut out = [0u8; 8];
        assert_eq!(block_on(rx.read(&mut out)).unwrap(), 0);
        state.borrow_mut().fail_read = true;
        let err = block_on(rx.read(&mut out)).unwrap_err();
        assert_eq!(err, IoError(embedded_io_async::ErrorKind::ConnectionReset));
    }

    #[test]
    fn write_with_delivers_everything_even_through_short_writes() {
        let conn = Conn::default();
        let state = conn.0.clone();
        state.borrow_mut().max_write = Some(1);
        let mut socket = IoSocket::new(conn);
        let (_rx, mut tx) = socket.split();
        block_on(picoserve::io::Write::write_with(&mut tx, |mut cursor| {
            cursor.try_append(b"hello world").unwrap();
        }))
        .unwrap();
        assert_eq!(state.borrow().output, b"hello world");
    }

    #[test]
    fn shutdown_closes_once_and_abort_just_drops() {
        let conn = Conn::default();
        let state = conn.0.clone();
        block_on(IoSocket::new(conn).shutdown(&Timeouts::const_default(), &EmbassyTimer)).unwrap();
        assert_eq!(state.borrow().close_calls, 1);

        let conn = Conn::default();
        let state = conn.0.clone();
        block_on(IoSocket::new(conn).abort(&Timeouts::const_default(), &EmbassyTimer)).unwrap();
        assert_eq!(
            state.borrow().close_calls,
            0,
            "abort must not attempt a clean close"
        );
    }

    #[test]
    fn a_failing_close_is_reported_as_a_write_error_of_the_same_kind() {
        let conn = Conn::default();
        conn.0.borrow_mut().fail_close = true;
        let result =
            block_on(IoSocket::new(conn).shutdown(&Timeouts::const_default(), &EmbassyTimer));
        match result {
            Err(picoserve::Error::Write(e)) => {
                assert_eq!(e, IoError(embedded_io_async::ErrorKind::ConnectionReset))
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    /// The plaintext path end to end (what `serve_forever_io` does per
    /// connection): accept -> IoSocket -> picoserve -> response -> clean close.
    #[test]
    fn plaintext_request_response_then_clean_close() {
        use picoserve::routing::get;
        let router = picoserve::Router::new().route("/", get(|| async { "hi" }));
        let config = crate::server_config();
        let conn = Conn::default();
        let state = conn.0.clone();
        state
            .borrow_mut()
            .input
            .extend(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n");
        let mut buffer = [0u8; crate::HTTP_BUFFER_LEN];
        block_on(crate::serve_one(
            &router,
            &config,
            &mut buffer,
            IoSocket::new(conn),
        ));
        let st = state.borrow();
        let text = alloc::string::String::from_utf8_lossy(&st.output).into_owned();
        assert!(text.starts_with("HTTP/1.1 200"), "got: {text}");
        assert!(text.ends_with("hi"), "got: {text}");
        assert_eq!(
            st.close_calls, 1,
            "the connection is closed cleanly after the exchange"
        );
    }
}
