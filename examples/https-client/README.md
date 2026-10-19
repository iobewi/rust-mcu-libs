# ESP32-S3 HTTPS client: reference and qualification

An outbound HTTPS client of **public** servers on ESP32-S3: Wi-Fi STA, SNTP, then `GET` over TLS with full certificate verification (chain to a built-in public root, host name, validity dates). It is the reference for using `iobewi-esp-tls`'s `EspTlsDialer` as a client, and the run that measures what that costs. No `insecure` mode exists; there is no switch to turn verification off, and `https` never falls back to `http`.

Composition (all existing bricks, plus the small additions of this change):

| Need | Brick |
| --- | --- |
| DNS + TCP + TLS handshake, verified against the given roots | `iobewi-esp-tls` `EspTlsDialer` |
| Refuse before any network activity if the clock is unset or no root | `iobewi-tls-service` `StaticTrustConnector` (new; the config-space `SecureConnector` is unchanged) |
| Public roots | `iobewi-tls-roots` (new) |
| Time | `iobewi-ntp` + `iobewi-time` |
| `GET`, response head, URL and redirect policy | `iobewi-http-client` `get` / `read_head` / `Target` (new; `drain_response` discards the body so it cannot serve a stream) |
| Verdicts | this crate's library: a refusal test passes only on the *certificate* error |

## Build and run

The TLS stack needs the opt-in crypto features: without them the handshake with every server below fails (RSA chains, P-384 chains, X25519). `public-pki` enables all three.

```sh
cargo +esp build -Z build-std=core,alloc --release --target xtensa-esp32s3-none-elf \
  -p example-https-client --features esp32s3,public-pki --bin https-client-esp32
espflash save-image --chip esp32s3 target/xtensa-esp32s3-none-elf/release/https-client-esp32 https-client.bin
```

**Wi-Fi and credentials.** The example uses `iobewi-wifi-manager` (reconnection with bounded backoff) over the credentials held by the board's `config` partition, provisioned beforehand with Improv Serial. **No credential is built into the image.** The example mounts that partition with the complete manifest of the StreamBeWI board it was qualified on (`stations`, `wifi`, `usb_boot`, `log`; `Store::mount` rejects stored records unknown to the manifest) and **never formats**: a blank or unmountable region stops it with a log line. On another board, change `SPACES`.

**Logs.** `iobewi_log::install` takes the application target; only targets starting with it log below WARN. It is the binary's module path, `https_client_esp32`.

The console prints one `RESULT <name> PASS|FAIL ...` line per scenario, `MEM` lines (heap, stack) after each, and a final `SUMMARY`. `IOBEWI_HEAP_KIB=<n>` at build time sets the heap of the run (default 96, StreamBeWI's).

## Scenarios

`badssl.com` is a third party's test site: it is used only by the cases that need a faulty certificate (6 connections per run). Every case that needs a *valid* chain, the clock cases and the reconnection loop go to the radio server. Please do not loop the run against it.

| Scenario | Expected | What it proves |
| --- | --- | --- |
| `clock-unsynced` | refused, no network activity | the guard, before SNTP |
| `valid-rsa` (`badssl.com`) | 200 | RSA chain, ECDHE-RSA, TLS 1.2 |
| `valid-ecdsa-p384` (`ecc384.badssl.com`) | 200 | ECDSA P-384 chain to ISRG Root X2 |
| `valid-radio` (Radio France MP3, 128 KiB read) | 200 | the real target: TLS 1.3 / X25519 / RSA-PSS, body throughput |
| `wrong-host` (`wrong.host.badssl.com`) | certificate rejected | host name check (same server and chain as `valid-rsa`) |
| `unknown-ca` | certificate rejected | `badssl.com` with a root set that does not contain its root |
| `untrusted-root`, `self-signed` | certificate rejected | chain to a CA outside the set |
| `expired-clock-2099`, `not-yet-valid-clock-2020` (radio server) | certificate rejected | validity dates, by moving the clock the TLS stack reads (the connector's guard still sees a set clock) |
| `valid-again` (radio server) | 200 | the refusals did not leave the stack broken |
| `reconnect-0..9`, `reconnect-heap` (radio server) | 200, heap growth <= 512 B | close and reconnect, no leak |

A refusal is a pass only when MbedTLS reports `MBEDTLS_ERR_X509_CERT_VERIFY_FAILED`. A DNS failure, a reset, or a handshake failure because the stack lacks a cipher is a FAIL. The stack does not say *which* verification check failed (unknown issuer, name, dates): the controls are what attribute each refusal to its fault.

Not covered here: following a redirect on the device (the policy, including `https` to `http` refused and the new host connecting afresh, is host-tested in `iobewi-http-client`), chunked bodies, and Wi-Fi/WAN loss.

## Measurements

### Flash (measured)

Image size from `espflash save-image`, this example, release, `xtensa-esp32s3-none-elf`:

| TLS features | Image (bytes) | vs. base |
| --- | --- | --- |
| none (the repository default) | 850 032 | |
| `rsa` (RSA, RSA-PSS, ECDHE-RSA) | 862 928 | +12 896 |
| `rsa`, `p384` | 868 448 | +18 416 |
| `rsa`, `p384`, `x25519` (`public-pki`) | 870 832 | +20 800 |

The five roots add 8 028 bytes of PEM (`iobewi-tls-roots::BUNDLE`), included in all rows. `.bss` is 523 796 bytes in all four builds (it includes the 96 KiB heap and the radio).

### On the board (Metronic 477144, ESP32-S3 QFN56 v0.2, 8 MB embedded PSRAM, 4 MB flash)

Image `public-pki`, Wi-Fi from the board's stored credentials, 2026-10-19. The board's `factory` and `config` partitions were backed up first and, after the runs, read back identical to the backup (SHA-256 equal); StreamBeWI was restored and boots.

**Results by heap size.** The same image logic, only `IOBEWI_HEAP_KIB` differs:

| Heap | Result | What failed |
| --- | --- | --- |
| 96 KiB (StreamBeWI's `HEAP_BYTES`) | **FAIL**, 8 pass / 15 fail | `valid-radio`: `MBEDTLS_ERR_X509_ALLOC_FAILED` (-0x2880), out of memory. `valid-rsa`, `valid-ecdsa-p384` and the 10 reconnections: `CertificateRejected` on servers whose chain is valid. The 6 refusal cases "passed" but proved nothing, since every certificate was refused. Heap peak 98 196 of 98 304 bytes |
| 128 KiB | **FAIL**, 18 pass / 5 fail, twice, identical | the 5 cases against `badssl.com`'s RSA chains end in the 20 s connect timeout. Cause not established |
| 160 KiB | **PASS** 23/23 | |
| 192 KiB | **PASS** 23/23 | |

The 160 and 192 KiB runs, and the 96 and 128 KiB ones, ran the earlier layout of the scenarios (`badssl.com` also used for the clock cases, the control and the 10 reconnections). The layout in this README (less load on `badssl.com`) compiles and is host-checked but **has not been run on the board**.

**Memory (192 KiB run).** Heap in use before any TLS: 47 868 bytes (Wi-Fi, network stack, config). Peak: 117 728 bytes at the first RSA handshake (TLS 1.2, ECDHE-RSA), 119 920 bytes with the Radio France stream (TLS 1.3). So TLS costs about 68 to 70 KiB of heap at peak on top of what the product already uses. After each connection the heap returns to 48 016 bytes. Over 10 close/reconnect cycles the drift is 0 bytes. Main stack high-water mark: 16 112 bytes during the handshakes, 16 800 bytes with the stream; the stack was 43.9 KiB at 192 KiB heap and 76.7 KiB at 160 KiB (heap and stack share the DRAM: a bigger heap shrinks the stack).

**Timings (192 KiB run).** `connect` is DNS + TCP + TLS handshake together; the example does not separate them.

| Case | connect | to response head | body |
| --- | --- | --- | --- |
| RSA, TLS 1.2 (`badssl.com`) | 1 858 ms | +524 ms | |
| ECDSA P-384 (`ecc384.badssl.com`) | 4 201 ms | +123 ms | |
| Radio France, TLS 1.3 | 2 469 ms | +253 ms | 131 432 bytes in 3 170 ms (about 41 KB/s, a 128 kbit/s stream needs 16 KB/s) |
| 10 reconnections (RSA) | 1 839 to 1 884 ms | +507 to +577 ms | |

SNTP: first valid time 1.05 to 1.1 s after the network is up. The clock guard refused before any network activity (`clock-unsynced` PASS).

Refusals (160 and 192 KiB): wrong host, unknown CA, untrusted root, self-signed, clock in 2099, clock in 2020 were all refused with the certificate-verification error, each against a server whose valid chain passed in the same run.

## Limits and findings

- The default `iobewi-crypto-mbedtls` build is for the device's own server identity (P-256 ECDSA). It cannot verify any of the public servers above. `public-pki` is opt-in so that existing firmware does not pay for it.
- `iobewi-tls-service`'s `SecureConnector` takes its CA from a config space, one PEM of at most 2 048 bytes. It was left alone; `StaticTrustConnector` is its fixed-trust counterpart.
- The dialer parses the whole PEM bundle on **every** connection (`Certificate::new` in `connect_client`): this is a per-connection cost in time and heap that grows with the number of roots.
- **StreamBeWI's 96 KiB heap does not hold this client.** Measured: about 120 KiB needed at peak, 160 KiB passes, 128 KiB does not reliably. The MbedTLS input record buffer is 16 384 bytes per session (`ssl-in-content-len-16384`). Raising the heap shrinks the stack (shared DRAM); the board has 8 MB of PSRAM that was not tried. Nothing was changed in StreamBeWI.
- `iobewi-ntp` had no user before this example; on the board it synchronized in about 1 s.
- A first Wi-Fi join often fails (`DisassociatedDueToInactivity`, the known #72); the bench's automatic EN reset retried until the join succeeded. Not a TLS matter.
- Not measured: DNS, TCP and handshake separately; Wi-Fi/WAN loss and recovery; USB MSC during the handshake (this example has no USB); following a redirect or decoding a chunked body on the device.
- `https-health`'s CI profile only does `cargo check`; the binary does not link (no `build.rs` with `-Tlinkall.x`). This example links in CI (`host-esp32s3-linked-bins`). Not fixed here.
- The root fingerprints other than ISRG Root X1 were not compared with their CAs' publications (see `iobewi-tls-roots`).
