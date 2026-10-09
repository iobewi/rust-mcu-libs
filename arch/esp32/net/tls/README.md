# tls

ESP32 MbedTLS integration: process-global TLS, DNS/TCP client dialer, TLS session streams, server listener with certificate identity. No HTTP routes or TLS trust persistence.

This adapter is hardware-specific. The portable contracts and certificate persistence live in `common/`. A successful cross-build does not replace hardware checks for secure entropy, certificate verification, fail-closed handshake or transport shutdown.
