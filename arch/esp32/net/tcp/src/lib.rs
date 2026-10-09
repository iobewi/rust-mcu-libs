#![no_std]

//! ESP TCP transport: an `embassy-net` listener and its accepted socket as
//! `net/io` connections. Knows no HTTP framework and no TLS; the TLS listener
//! (`iobewi-esp-tls`) and the HTTP server (`iobewi-http-server`) are layered
//! on top by the composition root. There is deliberately no plaintext HTTP
//! entry point here: the agent exposes administrative routes over TLS only,
//! and port 80 stays closed.

use embassy_net::Stack;
use embassy_net::tcp::TcpSocket;
use embedded_io_async::{ErrorType, Read, Write};
use iobewi_net_io::{Close, ConnectionListener};
use log::warn;

/// ESP TCP listener, the base of the TLS listener.
pub struct EspTcpListener<'a> {
    stack: Stack<'static>,
    port: u16,
    rx: &'a mut [u8],
    tx: &'a mut [u8],
}

impl<'a> EspTcpListener<'a> {
    pub fn new(stack: Stack<'static>, port: u16, rx: &'a mut [u8], tx: &'a mut [u8]) -> Self {
        Self { stack, port, rx, tx }
    }

    pub async fn accept_connection(&mut self) -> Result<TcpSocket<'_>, ()> {
        let mut socket = TcpSocket::new(self.stack, &mut *self.rx, &mut *self.tx);
        if let Err(e) = socket.accept(self.port).await {
            warn!("TCP: accept failed: {e:?}");
            return Err(());
        }
        Ok(socket)
    }
}

/// An accepted ESP TCP connection as a `net/io` connection: reads and writes
/// go straight to the embassy-net socket; `close` is a graceful TCP close
/// (FIN) followed by a flush.
pub struct EspTcpStream<'a>(TcpSocket<'a>);

impl ErrorType for EspTcpStream<'_> {
    type Error = embassy_net::tcp::Error;
}

impl Read for EspTcpStream<'_> {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        self.0.read(buf).await
    }
}

impl Write for EspTcpStream<'_> {
    async fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        self.0.write(buf).await
    }

    async fn flush(&mut self) -> Result<(), Self::Error> {
        self.0.flush().await
    }
}

impl Close for EspTcpStream<'_> {
    async fn close(&mut self) -> Result<(), Self::Error> {
        self.0.close();
        self.0.flush().await
    }
}

impl ConnectionListener for EspTcpListener<'_> {
    type Connection<'a> = EspTcpStream<'a> where Self: 'a;

    async fn accept(&mut self) -> Result<Self::Connection<'_>, ()> {
        self.accept_connection().await.map(EspTcpStream)
    }
}
