# iobewi-crypto-core

Portable `no_std` cryptography contracts for TLS certificate material: `TlsCrypto`, `Identity` and `PairError`. This crate validates and generates X.509 identity material through a platform-provided implementation; it does not own TLS sockets, persistence, certificate policy or cryptographic primitives.

MbedTLS implements this contract in the ESP32-specific layer and requires target-level qualification. Validation here covers host compilation, Rustfmt and Clippy.
