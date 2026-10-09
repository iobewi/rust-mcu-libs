
# iobewi-esp-wifi

## Summary

Reusable `no_std` ESP Wi-Fi transport built on `esp-radio` and `embassy-net`: station, soft access point, or both.

## Responsibilities

- Own lazy station initialization, scanning/strongest-BSSID selection, association, DHCP and the Embassy network runner.
- Expose the configured IP stack through the portable Wi-Fi transport boundary.
- Optionally host a soft access point (`WifiAccessPoint`) on the same radio: its own network stack and a small DHCP server (`edge-dhcp`) for its clients. There is no access-point-only mode: the access point always runs together with the station interface, which scanning and provisioning from the access point's client need (see Lifecycle).

## Non-responsibilities

- Does not own credential persistence or NVS layout.
- Does not own provisioning policy, TLS, HTTP, heartbeat/log services, OTA or application supervision.
- Does not serve any HTTP on the access point and does not choose its SSID, passphrase or lifetime: the product does, through the access point network handle.

## Architecture

Platform Wi-Fi adapter. `common/net/wifi/core` defines the portable transport/provisioning capabilities and `common/net/wifi/manager` owns durable credentials and retry/reprovision policy.

## Public API

Features `esp32c3` and `esp32s3` select the chip. The caller supplies `StackResources<N>` so socket capacity remains a composition decision.

`WifiManager<SOCKETS, AP_SOCKETS = 0>`: the second parameter defaults to 0, so existing `WifiManager<SOCKETS>` users are unchanged and carry no access point. `WifiManager::with_access_point(resources)` supplies a second, independent `StackResources<AP_SOCKETS>`; `AP_SOCKETS` must cover the DHCP server (one UDP socket) plus whatever the product serves there. The access point is `172.23.241.1/24` (`ACCESS_POINT_ADDRESS`), leases start at `.2` (four at most), no gateway and no DNS. Association limit equals the lease count.

`WifiManager::new` retains the peripheral and stack resources. `scan` and `connect` lazily initialize the radio, DHCP stack and network runner. `network_handle()` returns the stack handle after IPv4 configuration becomes available.

## Lifecycle

The first initialization consumes the supplied resources; subsequent connections reuse the same stack. Calling `connect` with identical SSID/password preserves the completed association only when the controller is still connected, the stack link is up and IPv4 configuration is available. This avoids interrupting active traffic when the portable manager resumes after an Improv request. A DHCP lease alone is insufficient: link loss, changed credentials or missing configuration follows the existing disconnect/reconfigure/associate/DHCP path. Cached successful credentials are invalidated before that path can await, so a failed or cancelled attempt cannot reuse previous connection evidence. This is in-memory state, not credential persistence.

**Access point.** `start_access_point` lazily creates the access point's stack, its runner and the DHCP task, applies `Config::AccessPointStation(station, access_point)`, waits for the access point link (5 s), then enables DHCP. From a station-only radio this is a mode change, so esp-radio stops and restarts the whole radio: the station association is lost and cached connection evidence is invalidated; the portable manager reconnects it. While the access point is active, `connect` re-applies the station configuration inside `AccessPointStation`, so reprovisioning from the access point's client does not change the mode and does not restart the radio under its clients. `stop_access_point` first stops DHCP (the task closes its socket and acknowledges, 1 s bound), then returns to `Config::Station`, again a radio restart. Starting with identical settings is a no-op; with different settings it reconfigures in the same mode and restarts DHCP (leases start empty at each activation). A failed configuration makes esp-radio stop the radio; the driver then tries to restore station mode. Nothing about the access point is persisted.

The network handle of the access point is valid only while it is active; the product must stop whatever it serves there when it stops the access point (the driver revokes only its own DHCP service).

**Choosing among access points that share an SSID** (mesh, repeaters). esp-radio's default scan dwells 10 to 20 ms per channel and its default station scan method (`Fast`) joins the first access point found, ignoring the signal sort. The driver therefore scans with a 40 to 120 ms active dwell, runs two passes and keeps the strongest access point per SSID (up to 40 records per pass); `connect` uses `ScanMethod::AllChannels` (sorted by signal) and, when a scan result exists, pins that access point's BSSID and channel. If the pinned attempt fails because the access point is missing or silent the pin is dropped, so the next attempt scans all channels and lets the radio choose instead of retrying a missing access point forever; if the access point answered but refused the credentials (handshake or authentication failure, logged by reason) the pin is kept, since the access point is fine. The station's internal retry is disabled (`failure_retry_cnt` 0): the portable manager already retries with backoff, and a second handshake would only double the time an access point's clients are disturbed by a failing join. After each association it logs the channel and signal and whether it is the pinned access point (never the BSSID). With no scan result (a normal boot with saved credentials) the driver first runs a directed scan for that SSID, with the same dwell and two passes, and pins the strongest access point it finds; only if the SSID is not seen does it fall back to the radio's own all-channel selection. That fallback is not relied on: on hardware, the first cold boot with saved credentials let the radio choose a -67 dBm access point on channel 1 although a -44 dBm one on channel 11 of the same SSID had been joined just before. The directed scan costs a few seconds before the join. Choosing the strongest at one moment is not roaming: a connected station does not move to a better access point later.

The handle identifies the reused stack, not permanent link availability. Product composition may publish it to consumers through the portable manager's `LinkObserver::ready`; `link_down` reports configuration loss. The portable manager retains reconnection policy ownership.

## AP + DHCP acceptance scope

For this migration, the access point is considered minimally useful when a WPA2 client can associate and obtain a lease from the built-in `edge-dhcp` server. Verify the assigned address belongs to `172.23.241.0/24`, the AP address is `172.23.241.1`, and clients receive no gateway or DNS. Verify lease service stops when AP stops, restarts cleanly, and station reconnection works across AP/STA mode changes. **An HTTP server or Web provisioning page is not required or part of this migration.**

## mDNS support (edge-mdns)

The Wi-Fi adapter enables the `multicast` feature of `edge-nal-embassy`, which in turn enables Embassy network multicast support. This is necessary for an mDNS responder to join the IPv4 multicast group `224.0.0.251` and listen on UDP port `5353`. Multicast is available on the Embassy stack returned by `network_handle()`, subject to radio support and physical qualification.

**Use `edge-mdns` directly in the application or composition layer.** The Wi-Fi crate does not own host names, DNS-SD service definitions, announcements, TXT records, sockets, or the lifetime of an mDNS responder. Avoid adding an `iobewi-mdns` wrapper that simply forwards the `edge-mdns` API. This matches the direct `edge-dhcp` reuse approach, except DHCP is owned by the access-point implementation whereas mDNS is application-selected.

A responder requires one UDP socket in the selected stack's `StackResources<N>` budget, a joined multicast group, a configured interface/IP address, and independent resource ownership for STA and AP when serving both networks. The product must manage re-announcements when an address changes, avoid using an AP stack after the AP stops, and check hostname collisions. The presence of the Cargo feature **does not automatically start an mDNS responder** or guarantee multicast reception.

Dependencies for a future direct consumer: `edge-mdns = { version = "0.8", default-features = false }` and `edge-nal-embassy = { version = "0.9", default-features = false, features = ["medium-ethernet", "proto-ipv4", "udp", "multicast"] }`; ensure the application selects a compatible target/network stack and appropriate `edge-mdns` features as required by its chosen I/O model. Do not add these as unused dependencies of the Wi-Fi adapter.

**Pending validation:** cross-build ESP32-C3 and ESP32-S3, then observe multicast group membership, mDNS response to queries from Avahi/Bonjour, AP/STA behavior and clean teardown/rejoin on real hardware. No stand-alone example is introduced in this increment.

## Validation

- `BG-ESP-S3`

Host regression tests compile the production connection-evidence module directly:

```sh
rustc --edition=2024 --test drivers/net/wifi/esp32/src/connection.rs -o /tmp/iobewi-wifi-keep-link-tests
/tmp/iobewi-wifi-keep-link-tests
```

They cover retained DHCP after disconnection, link/config loss, changed credentials and invalidation of prior connection evidence. ESP compilation checks the adapter call sites (`cargo +esp check -Z build-std=core,alloc --target xtensa-esp32s3-none-elf -p iobewi-esp-wifi --features esp32s3`, and the C3 equivalent). Hardware acceptance must verify Improv requests during streaming preserve the association and that access-point loss still reconnects; host tests do not prove radio behavior.

Access point acceptance, **not yet run on hardware**: a client associates with the WPA2 passphrase and receives a lease in the configured subnet; `connect` with the access point active leaves the client associated; `stop_access_point` makes the access point disappear and DHCP stop answering; the station reconnects after both start and stop through `maintain`; with no access point configured the behavior and memory are unchanged; free heap is measured before, during and after repeated start/stop cycles. The portable pieces are host-tested in `iobewi-wifi-core` and `iobewi-wifi-manager`; the DHCP server is the external `edge-dhcp` crate, so its lease logic is not re-tested here.

## Known limitations

No chip is selected by default; hardware validation is required when radio/HAL versions or connection mechanics change.

Starting or stopping the access point restarts the whole radio, so it interrupts an established station connection (streaming included); this driver does not provide an access point that can be toggled without touching the station, and the portable contract does not promise one (ADR-0017). When a station is associated the access point follows the station's channel, so the requested channel is a hint. When the station associates, the radio moves to the router's channel and the access point follows it (first run: requested 6, router 11), so an access point client can lose its link for a moment; products should expect it around `connect`, including a join that fails (a wrong passphrase still associates at the radio level before the handshake is refused). First failed-credentials run: the client's link dropped during the attempt and came back afterwards. The access point address and DHCP range are fixed constants; the DHCP server hands out no gateway and no DNS, so clients report no internet access. HTTP and Web provisioning are deliberately deferred; any future HTTP exposure requires a separate security review and explicit binding to the AP network. Association and the subsequent DHCP wait each have a 20-second timeout; the earlier disconnect and configuration-down waits are not covered by those timeouts.

## Related components

- `net/wifi/core`
- `net/wifi/manager`

