# iobewi-crypto-mbedtls

MbedTLS-backed implementation of the portable `TlsCrypto` contract. Provides X.509/PEM certificate and key validation, self-signed identity generation, server session configuration and time/entropy hooks.

## Composition

- Uses `iobewi-crypto-core` for the TLS material contract.
- Accepts an injected `iobewi-entropy::EntropySource`. The platform must provide **cryptographically secure** entropy; the example deterministic source in the RNG host test is never appropriate for TLS.
- Uses `iobewi-time` for UTC conversion; a missing wall clock must cause certificate verification to fail closed.
- `esp32s3` selects MbedTLS's GCC build path; `esp32c3` does not.

## Boundaries and validation

This component does not create TCP sockets, serve HTTPS routes, store identity material or install the underlying hardware RNG. The application owns clock initialization and hook installation. The CI profile cross-compiles on ESP32-C3 and ESP32-S3; real hardware TLS, certificate-validity and entropy checks remain separate acceptance gates.
