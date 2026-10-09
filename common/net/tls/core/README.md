# iobewi-net-tls-core

Portable, `no_std` contracts for authenticated TLS connections, extending `iobewi-net-io` without embedding a particular cryptography stack.

## Public contracts

- `SecureClientTransport: Connector` is an **explicit opt-in** marker for outbound connectors that guarantee encrypted, peer-verified TLS connections with fail-closed trust policy.
- `TlsListener: ConnectionListener` is an explicit marker for inbound listeners that return only successfully handshaken, encrypted connections with the configured server identity.
- `TlsDialer` opens outbound TLS connections with caller-provided CA material and transport buffers.

## Security boundary

Implementing these marker traits is a security assertion by the adapter author, **not runtime TLS validation performed by this crate**. A plain TCP connector/listener must never implement them. Callers must review certificate verification, identity handling, clock policy, error cases and downgrade prevention in each concrete adapter.

The TLS identity and trust service, cryptographic implementation, HTTP integration and ESP32-specific adapters are separate components.

## Validation

The host CI builds and lints these contracts. TLS handshake, certificate verification and network behavior require validation of the concrete adapter.
