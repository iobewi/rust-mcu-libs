# iobewi-tls-service

Portable `no_std` TLS identity and trust service, ported from `iobewi_old/net/tls/service`.

## Responsibilities

- Persist the server certificate/private key and trusted outbound CA in a dedicated `iobewi-config-space` allocation.
- Validate X.509 material through an injected `iobewi-crypto-core::TlsCrypto` implementation.
- Bootstrap a server identity only when none exists, without silently accepting corrupt stored material.
- Provide a `SecureConnector` using `iobewi-net-tls-core::TlsDialer`, refusing network connection before clock synchronization or without a trusted CA.

## Boundaries

No direct dependency on ESP32, MbedTLS, TCP socket implementations or HTTP routing. The application composes hardware adapters, crypto implementation, time source and isolated ConfigSpace. Cryptographic guarantees depend on the concrete `TlsCrypto` and `TlsDialer` implementations; plaintext adapters must never claim secure transport.

## Validation

Host tests exercise the configuration policy and fail-closed connector using fake crypto/network backends. The transport handshake, X.509 verification and hardware RNG must be qualified separately on ESP32-C3/S3.
