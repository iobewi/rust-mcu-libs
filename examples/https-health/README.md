# ESP32 HTTPS /health smoke test

Minimal on-device integration example for ESP32-C3 and ESP32-S3. Wi-Fi STA + Embassy TCP + MbedTLS + `EspTlsListener` + `iobewi-http-server::serve_forever_tls`; no HTTP port 80 and no plaintext fallback.

## Build-time inputs

Set `IOBEWI_STA_SSID`, `IOBEWI_STA_PASSWORD`, `IOBEWI_TLS_CERT_PEM`, and `IOBEWI_TLS_KEY_PEM` as environment variables before building. The PEM values contain the full certificate and key, including newlines. **Use disposable test credentials only:** embedding a private key in firmware is unsuitable for production.

Build from the repository root using the same C3/S3 compiler setup as existing examples:

```sh
cargo +nightly build -Z build-std=core,alloc --release --target riscv32imc-unknown-none-elf -p example-https-health --features esp32c3 --bin https-health-esp32
cargo +esp build -Z build-std=core,alloc --release --target xtensa-esp32s3-none-elf -p example-https-health --features esp32s3 --bin https-health-esp32
```

Flash using the project's usual `espflash` procedure. Find the assigned DHCP IP in the device/route table. Then test `curl --cacert cert.pem https://DEVICE_IP/health` if the certificate SAN includes the device IP, or `curl --resolve example.local:443:DEVICE_IP --cacert cert.pem https://example.local/health` when the certificate includes `example.local` as a DNS SAN. Expected body: `ok`.

Check that port 80 is closed, a plaintext request to 443 fails, the wrong CA/hostname is rejected, and the service refuses to start a TLS session when the PEM material is invalid. The initial example intentionally does **not** persist credentials or mount provisioning routes; those are follow-up tests after the handshake smoke test.

The clock callback returns `None` because this example tests server-side TLS only; client-side certificate verification must later use synchronized UTC and fail closed until synchronization.
