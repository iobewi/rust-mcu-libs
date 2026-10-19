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
export IOBEWI_STA_SSID=... IOBEWI_STA_PASSWORD=...   # disposable credentials; the image contains them: never commit or publish it
cargo +esp build -Z build-std=core,alloc --release --target xtensa-esp32s3-none-elf \
  -p example-https-client --features esp32s3,public-pki --bin https-client-esp32
espflash save-image --chip esp32s3 target/xtensa-esp32s3-none-elf/release/https-client-esp32 https-client.bin
```

The console prints one `RESULT <name> PASS|FAIL ...` line per scenario, `MEM` lines (heap, stack) after each, and a final `SUMMARY`.

## Scenarios

Public test servers (`badssl.com`) are used so that each refusal has the same server as a passing control, with one fault.

| Scenario | Expected | What it proves |
| --- | --- | --- |
| `clock-unsynced` | refused, no network activity | the guard, before SNTP |
| `valid-rsa` (`badssl.com`) | 200 | RSA chain, ECDHE-RSA, TLS 1.2 |
| `valid-ecdsa-p384` (`ecc384.badssl.com`) | 200 | ECDSA P-384 chain to ISRG Root X2 |
| `valid-radio` (Radio France MP3, 128 KiB read) | 200 | the real target: TLS 1.3 / X25519 / RSA-PSS, body throughput |
| `wrong-host` (`wrong.host.badssl.com`) | certificate rejected | host name check (same server and chain as `valid-rsa`) |
| `unknown-ca` | certificate rejected | `badssl.com` with a root set that does not contain its root |
| `untrusted-root`, `self-signed` | certificate rejected | chain to a CA outside the set |
| `expired-clock-2099`, `not-yet-valid-clock-2020` | certificate rejected | validity dates, by moving the clock the TLS stack reads (the connector's guard still sees a set clock) |
| `valid-again` | 200 | the refusals did not leave the stack broken |
| `reconnect-0..9`, `reconnect-heap` | 200, heap drift <= 512 B | close and reconnect, no leak |

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

The five roots add 6 774 bytes of PEM (`iobewi-tls-roots::BUNDLE`), included in all rows. `.bss` is 523 796 bytes in all four builds (it includes the 96 KiB heap and the radio).

### Heap, stack, handshake time, throughput

**Not yet measured on hardware.** The run on the Metronic 477144 is pending; this section is filled from its `MEM` and `RESULT` lines. The heap is set to 96 KiB, StreamBeWI's `HEAP_BYTES`, on purpose: the question is whether the client fits in the budget the product already has. Nothing was enlarged.

## Limits and findings

- The default `iobewi-crypto-mbedtls` build is for the device's own server identity (P-256 ECDSA). It cannot verify any of the public servers above. `public-pki` is opt-in so that existing firmware does not pay for it.
- `iobewi-tls-service`'s `SecureConnector` takes its CA from a config space, one PEM of at most 2 048 bytes. It was left alone; `StaticTrustConnector` is its fixed-trust counterpart.
- The dialer parses the whole PEM bundle on **every** connection (`Certificate::new` in `connect_client`): this is a per-connection cost in time and heap that grows with the number of roots.
- The MbedTLS input record buffer is 16 384 bytes (`ssl-in-content-len-16384`), per session. Whether the 96 KiB heap holds it with Wi-Fi, the handshake peak and the buffers is what the hardware run decides.
- `iobewi-ntp` had no user and no cross-build before this example; it builds and links for ESP32-S3 here, its behaviour on the device is part of the pending run.
- `https-health`'s CI profile only does `cargo check`; the binary does not link (no `build.rs` with `-Tlinkall.x`). This example links in CI (`host-esp32s3-linked-bins`). Not fixed here.
- The root fingerprints other than ISRG Root X1 were not compared with their CAs' publications (see `iobewi-tls-roots`).
