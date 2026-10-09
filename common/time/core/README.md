# iobewi-time

Portable `no_std` Unix epoch clock, backed by Embassy monotonic time and critical-section state. Before initial synchronization, `now()` returns `None`. Consumers requiring X.509 certificate validity must fail closed if UTC time is unavailable.

A platform or network time source performs synchronization; this crate does not access NTP, sockets, or a platform HAL. Validate host compilation, Rustfmt and Clippy in CI, then qualify time initialization and monotonic behavior on hardware.
