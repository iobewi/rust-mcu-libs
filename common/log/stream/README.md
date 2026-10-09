# log/stream

Optional best-effort outbound log delivery, built from `iobewi-log`, `iobewi-http-client/websocket`, an authenticated `SecureClientTransport`, and a platform-provided entropy source. No ESP-specific dependency. No mandatory lifecycle service.

The consumer implements `StreamConfig` and `LogMetadata` and explicitly starts `run`. Connection and RFC 6455 upgrade require TLS; no plaintext fallback. Log production must never wait on network I/O: captured lines are removed from the bounded ring and sent asynchronously. Failed sessions discard stale records; reconnects back off with jitter. A changed bearer token triggers reconnect.

This module uses a volatile bounded ring and does not guarantee durable or lossless delivery. The application decides authorization, URL, token provisioning, timestamping and whether streaming runs at all. `LogPolicy::Off` stops log capture, but does not grant or revoke permission to stream; that requires separate application policy. No secrets are logged.

Security: use an authenticated TLS connector with verified CA, hostname and trustworthy UTC clock. Test the actual network behavior on ESP32-C3/S3 during the later network integration campaign. Host unit tests and CI are required prior to merge.
