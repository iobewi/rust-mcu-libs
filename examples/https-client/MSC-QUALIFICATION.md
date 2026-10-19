# TLS and USB MSC on StreamBeWI: board qualification (#21)

Board: Metronic 477144 chain, ESP32-S3 (DevKitC-1 class board with 8 MB PSRAM, **not used**), real USB host = the Metronic player. 2026-10-19. Qualification-only firmware, branch `qual/21-tls-msc` of `iobewi/streambewi` (from `origin/main` a262015, never to be merged as is). Raw traces: `traces/msc/`.

**Listening is not confirmed.** Everything below is device counters and logs; the audible result at the reconnection instants is pending the human's confirmation. The MSC counters themselves are technically valid.

## What was built (qualification only)

- HTTPS in the producer through the bricks of this PR: `StaticTrustConnector<EspTlsDialer>`, `iobewi-tls-roots`, `iobewi-http-client::{get, read_head, Target}`, `iobewi-ntp`; `https://` stations accepted.
- Chunked decoding with `edge-http` (`Body`): Radio France answers HTTPS with `Transfer-Encoding: chunked`.
- Second heap region in DRAM2 (73 744 B) beside the 80 KiB main heap; audio ring (96 KiB) and stack untouched; no PSRAM. Bootloader: ESP-IDF 2nd stage (boot log "ESP-IDF v6.1-beta1 2nd stage bootloader"), which is the precondition of `#[esp_hal::ram(reclaimed)]`.
- Light instrumentation: executor lateness (10 ms heartbeat), MSC counters (sectors, inter-sector gap, Pending, ZEROFILL, stalls, per-`Expired` detail), per-handshake window (`qual-hs`), heap/DRAM2/stack probe every 30 s, controlled drops on a schedule (`QUAL_DROP_LIVES`, alternately a clean close and an abrupt drop).
- A log monitor (`tools/qual/watch21.py`) that stops at the first anomaly.

## Findings that changed the product work

1. **The network socket set must grow by one.** SNTP adds a UDP socket to a set sized for four (DHCP, DNS, stream, local UI): `smoltcp` panicked ("socket set full"). `SOCKETS` is 5 with HTTPS.
2. **Chunked bodies are the norm over HTTPS.** Without a decoder nothing enters the window. `edge-http` decodes them, including the bytes that arrived with the response head.
3. **A TLS handshake keeps the executor for about 270 ms** at 240 MHz (261..300 ms measured), because the whole product runs in one task. It is not a persistent blockage and it did not starve the MSC read path in the runs below.

## Results

### HTTPS, nominal (run A): 11 min 42 s with the real host

No `ZEROFILL`, `Pending`, stall, `Expired` or panic. Host MSC counters at the end: `pending=0 zerofill=0 stalls=0`. One station change (HTTP to HTTPS) at the start: the product detaches the disk for 1 793 ms by design, the host re-enumerates (bus reset, new session) and the largest gap between two served sectors is 4 324 ms; nothing else for the next 11.5 min. TLS up in 554 ms; executor lateness at that handshake 272 ms; body-read poll at most 24 ms.

### HTTPS, six controlled reconnections (run B), no station change, disk not detached

Three abrupt (session dropped without close) and three graceful (close-notify and FIN), at irregular lifetimes of 150 to 180 s, then 120..160 s. Every reconnection came back by itself.

| # | Time | Type | TLS handshake | Sectors served during it | Largest gap between sectors | Lead after |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | 21:08:43 | abrupt | 551 ms | 8 | 216 ms | 61 440 B |
| 2 | 21:11:46 | graceful (closed in 11 ms) | 546 ms | 1 | 340 ms | 61 440 B |
| 3 | 21:13:59 | abrupt | 537 ms | 8 | 234 ms | 62 976 B |
| 4 | 21:16:41 | graceful (54 ms) | 550 ms | 1 | 311 ms | 61 952 B |
| 5 | 21:19:03 | abrupt | 532 ms | 8 | 262 ms | 62 976 B |
| 6 | 21:21:05 | graceful (51 ms) | 539 ms | 2 | 278 ms | 60 928 B |

- The host kept reading during every handshake. The usual gap between sectors is 200..250 ms; a reconnection adds up to about 100 ms to the largest gap. The buffer lead falls to about 61 KB while reconnecting and goes back to 81 920 B (the 1.25 s of audio taken from the buffer is refilled).
- Over the whole window (18 min): MSC `cmds=8233 sectors=32350 pending=0 zerofill=0 stalls=0`; no `Expired`; USB events only at the boot of 21:06 (suspend, bus reset, new session after the flash), none during any reconnection.
- Executor lateness: 266..300 ms at each handshake, 594 ms once at boot (the Wi-Fi and config start-up, seen at every boot); no other sample above 100 ms.
- Memory: heap in use 110 608..111 032 B between reconnections (no drift; +424 B from first to last sample), 119 524 B at the busiest sample, allocator peak 135 580 B; DRAM2 in use 28 820..45 556 B; minimum stack margin 11 552 B (local UI active). The old session is released: the heap returns to its baseline after each reconnection.

### HTTP, two reconnections (same image, no simulated reader)

One abrupt (21:28:31) and one graceful (21:31:13), the same conditions. Connected again (HTTP 200) 1.6 s and 2.4 s after the drop; `Pending`, `ZEROFILL`, stalls and `Expired` all 0; largest gap between sectors 234..259 ms; the lead is back at 81 920 B within a minute.

### The earlier `Expired` bursts (item 7)

In an earlier run (`traces/msc/run-simulated-reader-and-60s-drops-HTTP.txt`), with a fixed drop every 60 s on HTTP, each drop was followed by a burst of 3..21 `ZEROFILL status=Expired` sectors (7 of 7), while a **simulated reader of my qualification image was reading the same stream session as the real host**. The two controlled-reconnection runs above, without the simulated reader, show none, for HTTP as for HTTPS. This **strengthens the hypothesis** that the bursts came from the second reader on the single session, a limit the product already documents (`docs/concurrent-readers.md`), and not from the reconnection or from TLS. It is **not formally demonstrated**: the earlier run also differed by the fixed 60 s period.

## Invalid or partial runs (kept as evidence, not used above)

- `run-0-chunked-refused-invalid.txt`: the first HTTPS path refused chunked bodies, so no data entered the window and the simulated reader starved; this was a defect of the qualification code, not a cost of TLS.
- A second campaign had no forced reconnection (the build variable was missing) and a simulated reader that restarted its session before the disk was back; its post-switch underruns were artifacts of the simulation.
- A third campaign was stopped by the human when the real host started to hear cuts: the image then in the board dropped the stream every 60 s by design, with the simulated reader still on.

## Limits

- The reconnections are triggered by the firmware (clean close, abrupt drop), not by a real WAN or Wi-Fi outage; a real outage was not tested.
- The numbers are from counters and logs of the board; no host-side capture (`verify_mp3.py` style) was taken and there is no confirmation by ear yet.
- One radio, one board, 18 + 8 minutes. The P-384 case (3.5 s executor stall at 80 MHz, `README.md`) was not exercised in the product.
- The simulated-reader hypothesis for the bursts is not formally proven.

## State of the board afterwards

`factory` and `config` were restored from the backups taken before the first write (SHA-256 `adc6c5010d1075664f9f97a314b2bf8f4a2dbdd327d50f7bafe52dde4a5c974c` and `7e68dc471588a601d18e2ad8e76dfba7295acf337d41afa4d0d8e87655833cb1`) and read back identical at 21:34; the original StreamBeWI started again (network up, HTTP station, MSC session with the host). The bench's automatic reset was set back on.
