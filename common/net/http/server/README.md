# iobewi-http-server

Portable HTTP server routing and request handling built on `picoserve`.

## Responsibilities

- Expose routing and response primitives for caller-owned HTTP routes.
- Serve a connected socket or an inbound `iobewi-net-io::ConnectionListener`.
- Adapt portable `Read + Write + Close` connections into `picoserve` sockets.
- Provide bounded streaming and small request helpers.

## Boundaries

This crate does not bind TCP ports, initialize Wi-Fi or install a logger. The application supplies its listener and route set. Diagnostic output uses the standard `log` facade, served by `iobewi-log` when the application installs it.

**HTTPS is not yet exposed:** the former `serve_forever_tls` entry point requires the separate, not-yet-ported `TlsListener` contract. It must be restored only when that contract exists, to retain its compile-time guarantee that TLS handshakes have completed.

## Validation

Host tests and Clippy via the workspace CI. ESP32 socket integration and end-to-end HTTP/HTTPS qualification belong to follow-up adaptations.
