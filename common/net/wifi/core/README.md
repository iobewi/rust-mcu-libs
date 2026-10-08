
# iobewi-wifi-core

## Summary

Portable Wi-Fi contracts: station transport port, soft access point port and the provisioning capability

## Responsibilities

- Define `WifiTransport` (station), `WifiProvisioning` (what a provisioning workflow may do) and `WifiAccessPoint` (a soft access point hosted by the same radio).
- Define `AccessPointConfig`: SSID 1 to 32 bytes, WPA2 passphrase 8 to 63 bytes, channel 1 to 13. An open access point is not representable, and `Debug` never prints the passphrase.
- Preserve the `portable-contract` boundary.

## Non-responsibilities

- Does not access a platform HAL directly.
- Does not own platform-specific device mechanics.
- Does not implement DHCP, routing, HTTP or any service on the access point's network, and does not decide when an access point opens or for how long (product policy).

## Architecture

Path: `net/wifi/core`. Layer: **portable-contract**.

## Public API

Exported Rust items are the code-level API authority. Package features and dependency declarations remain canonical in `Cargo.toml`.

`WifiAccessPoint` has fallible-by-`bool` `start_access_point(&AccessPointConfig)` and `stop_access_point()`, `is_access_point_active()` and `access_point_handle()` (the handle of the access point's own network, valid only while active). A platform implements it on the same object as `WifiTransport`: one radio, one owner.

**Mode changes may restart the radio.** Starting or stopping the access point may drop the station link (the ESP driver does, because esp-radio applies a mode change that way). The port does not hide this; recovery is the manager's reconnection loop. Starting persists nothing.

## Invariants

- `INV-001`

## Validation

- `cargo test -p iobewi-wifi-core`: configuration limits, WPA2-only, redacted `Debug`.
- No hardware baseline gate is declared here; radio behavior is validated at the platform adapter.

## Known limitations

The port cannot express an access point that is toggled without touching the station: whether a platform can do that is a property of its radio library, not of this contract. See ADR-0017.

## Related components

- Root `ARCHITECTURE.md` and `INVARIANTS.md`.
- `Cargo.toml` for package features/dependency facts.
