# Improv provisioning

Portable Improv Serial command handling coordinated with `WifiProvisioning`. Based on the reusable command workflows in `embewi-agent` and `streambewi`, without importing product-specific boot policy, recovery, LED control, transport allocation, or service startup.

The application owns serial transport, authorization, and any recovery policy. The codec is `improv-serial`; the Wi-Fi manager remains responsible for persistence and reconnect. This crate returns ordered response frames and a semantic event, so a caller can send results over UART/USB and update its own UI.

The first increment provides the host-testable coordination API. Serial integration, real-device qualification, and multiport session ownership must be validated separately.
