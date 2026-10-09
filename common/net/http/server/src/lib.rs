#![no_std]
//! HTTP request handling for any connected socket. The caller chooses the
//! transport and supplies routes; this crate never opens a network port.

extern crate alloc;

use iobewi_net_io::ConnectionListener;
use picoserve::io::Socket;
use picoserve::routing::PathRouter;
use picoserve::{Config, EmbassyRuntime, Router};

pub use picoserve::Router as HttpRouter;
/// Shared HTTP routing and response types for framework services. A service
/// contributes handlers to the caller's router; the application selects the
/// complete route set and the transport listener.
pub use picoserve::{ResponseSent, io, request, response, routing};

pub mod auth;
pub mod io_socket;
pub mod json;
pub mod range;
pub mod stream;

/// Serve a connected socket with a caller-provided router.
/// Services and applications may contribute routes to the same router.
pub async fn serve_connection<R: PathRouter, S: Socket<EmbassyRuntime>>(
    router: &Router<R>,
    config: &Config,
    http_buffer: &mut [u8],
    socket: S,
) -> Result<picoserve::DisconnectionInfo<picoserve::NoGracefulShutdown>, picoserve::Error<S::Error>>
{
    picoserve::Server::new(router, config, http_buffer)
        .serve(socket)
        .await
}

/// Size of the HTTP request/response buffer reused across connections.
pub const HTTP_BUFFER_LEN: usize = 2048;

/// The server configuration used by the serve loops: picoserve defaults with
/// persistent (keep-alive) connections.
pub fn server_config() -> Config {
    Config::const_default().keep_connection_alive()
}

/// Serve one accepted connection and log (at debug level) if it ended with an
/// error. This is the body of the serve loops: a listener-specific loop
/// (such as the TLS one in `iobewi-https`) accepts, then calls this.
pub async fn serve_one<R: PathRouter, S: Socket<EmbassyRuntime>>(
    router: &Router<R>,
    config: &Config,
    http_buffer: &mut [u8],
    socket: S,
) {
    if serve_connection(router, config, http_buffer, socket)
        .await
        .is_err()
    {
        log::debug!("http: connection closed with an error");
    }
}

/// Serve one connection at a time on a protocol-free `net/io` listener,
/// adapting each accepted connection to a picoserve socket
/// ([`io_socket::IoSocket`]). The listener owns the retry delay and any
/// platform log: an `Err` from `accept` just loops.
pub async fn serve_forever_io<L: ConnectionListener, R: PathRouter>(
    listener: &mut L,
    router: &Router<R>,
) -> ! {
    let config = server_config();
    let mut http_buffer = [0u8; HTTP_BUFFER_LEN];
    loop {
        if let Ok(connection) = listener.accept().await {
            serve_one(
                router,
                &config,
                &mut http_buffer,
                io_socket::IoSocket::new(connection),
            )
            .await;
        }
    }
}
