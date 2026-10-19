# iobewi-tls-roots

A short list of public root CA certificates (PEM, in flash) to verify public HTTPS servers from an outbound TLS client. Data only: no parser, no policy, no platform type.

- `ALL`: the roots, each with its subject, the reason it is there and the SHA-256 of its DER (checked by the host tests).
- `BUNDLE`: the same roots concatenated, built at compile time, to pass as the `ca_pem` of a `TlsDialer`.

## Provenance and maintenance

The PEM files in `pem/` were copied from the Debian `ca-certificates` bundle (`20250419~deb12u1`), itself derived from Mozilla's. The ISRG Root X1 fingerprint was compared with the one published by Let's Encrypt. **The four others were not compared with their issuers' publications: do this when reviewing.** The tests only prove that each file matches the fingerprint recorded in this crate, not that the fingerprint is the CA's.

Why a short list: each root is a trust decision and costs flash plus, on every connection, parse time and heap in the TLS stack. Roots (not intermediates) because intermediates rotate often. To add a root, add its PEM, a `Root` constant with its reason, and an entry in `ALL` and `BUNDLE`; the tests fail until all agree.

This crate does not change `iobewi-tls-service`: a device with a CA provisioned by an operator keeps its single-CA flow. A client of public servers passes `BUNDLE` (or a subset) to the dialer directly.

Needs on the MbedTLS side (`iobewi-crypto-mbedtls` features): `rsa` for the RSA roots, `p384` for ISRG Root X2.
