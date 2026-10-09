# iobewi-net-io

Portable, `no_std` connection contracts built on the standard `embedded-io-async` traits.

## Responsibilities

- `Close`: asynchronous orderly shutdown, using the connection's I/O error type.
- `Connection`: combines `Read + Write + Close` with a blanket implementation.
- `ConnectionListener`: accepts inbound connections; retry and platform logging remain the listener's responsibility.
- `Connector`: opens outbound connections to `host:port`, borrowing caller-provided RX/TX buffers.
- `Connector::local_address`: optional best-effort address, defaulting to `None`.

## Boundaries

This crate does not implement TCP, DNS, Wi-Fi, TLS, HTTP, authentication or platform-specific I/O. It adds only contracts used for composing transport-agnostic services. Its connection traits do **not** imply confidentiality or authentication. Higher-level secure transport contracts must express those guarantees separately.

## Validation

`cargo test -p iobewi-net-io` tests loopback connections, connection and listener behavior and error handling on the host. Compile-time consumers and hardware adapters are qualified in their respective crates.
