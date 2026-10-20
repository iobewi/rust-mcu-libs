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

Image `public-pki`, Wi-Fi from the board's stored credentials, 2026-10-19. The board's `factory` and `config` partitions were backed up first; after the runs they read back identical to the backup (SHA-256 equal) and StreamBeWI boots. Raw console traces are in `traces/`. The PSRAM is not used: the design stays valid on S3 without it.

**Reference: 160 KiB of heap** (the example's default). Four consecutive runs of the final scenario set: 23/23 each (`traces/board-160k-run1..4.txt`; run 4 is the one without the fragmentation probe's side effect on the peak).

| Heap | Result | Notes |
| --- | --- | --- |
| 96 KiB (StreamBeWI's `HEAP_BYTES` today) | **FAIL**, 8 pass / 15 fail | `valid-radio`: `MBEDTLS_ERR_X509_ALLOC_FAILED` (-0x2880). Valid chains then rejected. The 6 refusal cases "passed" but proved nothing, every certificate being refused. Peak 98 196 of 98 304 bytes. Earlier scenario layout, no trace saved |
| 128 KiB | **inconclusive** | Earlier layout: FAIL twice, identically (5 `badssl.com` RSA cases end in the 20 s connect timeout, `traces/board-128k-run-earlier-layout-*.txt`). Final layout: PASS 23/23 once (`board-128k-run-current.txt`, peak 121 644 of 131 072 bytes = 93 %). Not reproducible, cause not established (the failing runs peaked at only 69 KiB when they stalled, and the same RSA servers connected later in the same runs): not adopted, a margin of 9 KiB is not one |
| 160 KiB | **PASS** 23/23, four runs | |
| 192 KiB | PASS 23/23, earlier layout | not needed: no measurement justifies it |

**Memory at 160 KiB (run 4).**

| Point | Heap used | Peak so far |
| --- | --- | --- |
| Wi-Fi, network stack and config up, before any TLS | 49 524 B | 50 028 B |
| After first RSA handshake (TLS 1.2, ECDHE-RSA) | 48 016 B | 117 728 B |
| After Radio France stream (TLS 1.3) | 48 016 B | 119 400 B |
| End of run | 48 016 B | **119 940 B** (73 % of 163 840) |

TLS costs about 70 KiB of heap at peak on top of the 48 KiB the Wi-Fi, network and config already hold. The heap returns to 48 016 bytes after each connection and the drift over 10 close/reconnect cycles is 0 bytes. After a rejected certificate 8 520 bytes more stay in use until the next connection (56 536 B, seen after `wrong-host`, `unknown-ca`, `untrusted-root`, `expired-clock-2099`, `not-yet-valid-clock-2020`), then are released: bounded, not accumulating.

**Fragmentation.** At the end of the run 114 120 bytes are free and the largest block available is 111 KiB. The 16 KiB MbedTLS record buffer and the handshake structures were allocated and freed 23 times with no loss of contiguity.

**Stack.** Main stack (the Embassy executor and its tasks): 76 672 bytes reserved at 160 KiB of heap, high-water mark 16 112 bytes during the handshakes and the stream, **margin 60 560 bytes**. A bigger heap shrinks the stack (shared DRAM): 109 440 reserved at 128 KiB, 43 904 at 192 KiB. The Wi-Fi manager runs in its own task, as in `ota-system`, where composing it with the TLS server in one task overflowed the S3 main stack. Radio-task stacks live in the heap and are not covered.

**Timings (run 4, `connect` = DNS + TCP + TLS handshake together).**

| Case | connect | + response head | body |
| --- | --- | --- | --- |
| RSA, TLS 1.2 (`badssl.com`) | 1 875 ms | 516 ms | |
| ECDSA P-384 (`ecc384.badssl.com`) | 4 252 ms | 126 ms | |
| Radio France, TLS 1.3 | 1 425 ms | 247 ms | 131 426 B in 3 109 ms (about 41 KB/s; 128 kbit/s needs 16 KB/s) |
| 10 reconnections (Radio France) | 1 394 to 1 559 ms | 245 to 310 ms | |

SNTP: first valid time 1.05 s after the network is up. `clock-unsynced` refused before any network activity.

**Refusals.** Wrong host, unknown CA, untrusted root, self-signed, clock in 2099, clock in 2020: all refused with the certificate-verification error, each against a server whose valid chain passed in the same run. Whether a refusal is a name, issuer or date fault is told by the control, not by the stack.

## Limits and findings

- The default `iobewi-crypto-mbedtls` build is for the device's own server identity (P-256 ECDSA). It cannot verify any of the public servers above. `public-pki` is opt-in so that existing firmware does not pay for it.
- `iobewi-tls-service`'s `SecureConnector` takes its CA from a config space, one PEM of at most 2 048 bytes. It was left alone; `StaticTrustConnector` is its fixed-trust counterpart.
- The dialer parses the whole PEM bundle on **every** connection (`Certificate::new` in `connect_client`): this is a per-connection cost in time and heap that grows with the number of roots.
- **StreamBeWI's 96 KiB heap does not hold this client.** Measured peak about 120 KiB for the client alone; 160 KiB is the reference, 128 KiB left unadopted. Raising the heap shrinks the stack (shared DRAM). The product's own figures with USB MSC and the stream running at the same time are not measured here: that is the integration's measurement. PSRAM: not used on purpose. Nothing was changed in StreamBeWI.
- `iobewi-ntp` had no user before this example; on the board it synchronized in about 1 s.
- A first Wi-Fi join often fails (`DisassociatedDueToInactivity`, the known #72); the bench's automatic EN reset retried until the join succeeded. Not a TLS matter.
- Not measured: DNS, TCP and handshake separately; Wi-Fi/WAN loss and recovery; USB MSC during the handshake (this example has no USB); following a redirect or decoding a chunked body on the device.
- `https-health`'s CI profile only does `cargo check`; the binary does not link (no `build.rs` with `-Tlinkall.x`). This example links in CI (`host-esp32s3-linked-bins`). Not fixed here.
- The five root fingerprints were compared with their CAs' own publications (see `iobewi-tls-roots`).
