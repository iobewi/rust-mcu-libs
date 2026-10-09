# TLS HTTP provisioning

Portable certificate and CA provisioning API, ported from `iobewi_old/net/http/tls`. It uses the HTTP server's JSON response types and the TLS trust service. The application must provide a real authorization backend and mount these routes **only** over an authenticated `TlsListener`.

`iobewi-http-server::serve_forever_tls` accepts exclusively TLS listener implementations; no plaintext fallback is permitted. The integration deliberately does not instantiate a Wi-Fi stack, certificate store, or router at the application layer.

Run host tests and Clippy via the `host` CI profile. Hardware validation of secure handshakes and authenticated route access is deferred to the end-to-end ESP example.
