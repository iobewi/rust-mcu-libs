# mDNS host responder

This crate provides **a concrete, optional composition** of `edge-mdns` with an existing `edge-nal::UdpBind` stack. It exposes one `respond` future which serves `hostname.local` using a caller-supplied IPv4 address; it does not reimplement the mDNS protocol or impose a network lifecycle on applications.

The application supplies the stack (for ESP32, an `edge-nal-embassy::Udp` adapter on top of its Embassy stack), receive/send packet buffers implementing `edge_mdns::buf::BufferAccess`, a suitable RNG, and a change signal. The app also owns cancellation/restart after Wi-Fi/DHCP changes. The caller allocates **one UDP socket** for this responder and enables multicast; on ESP32 the Wi-Fi adapter already enables the multicast feature. The handler uses the mDNS IPv4 group and UDP port 5353 via `edge_mdns::io::bind`.

The hostname is application-owned; no credentials, configuration storage or policy are embedded here. No `avahi` daemon is necessary on the device. A desktop Linux client may use Avahi or Bonjour to resolve the announced name.

DNS-SD / TXT / SRV: `respond_service` advertises one caller-defined `edge_mdns::host::Service` (PTR, SRV, TXT and host address records). `respond` only answers host address queries. More elaborate service sets can use `edge-mdns` upstream handlers directly. No collision-detection guarantee is made.

## Status

The initial crate must pass host Cargo compilation and lint gates before merge. Later hardware acceptance must verify multicast and .local resolution with Avahi/Bonjour across link loss and STA/AP transitions; firmware cross-compilation and that physical check are not claimed by this crate.
