# Wi-Fi modes — ESP32 reference examples

Three **separate firmware binaries**, one portable application scenario in `src/app.rs`, a conditional ESP32 implementation in `src/platform/esp32.rs`, and the existing Wi-Fi components (no custom radio, DHCP, or HTTP implementation):

| Binary | Exercises | Acceptance |
| --- | --- | --- |
| `wifi-sta` | Station association + router DHCP | Joins existing router and gets a station IPv4 address |
| `wifi-ap` | WPA2 soft AP + `edge-dhcp` | A phone/laptop associates and receives `172.23.241.2` or another address in `172.23.241.0/24` |
| `wifi-apsta` | Concurrent AP + station | STA gets router IPv4; AP remains active and serves client DHCP |

All three binaries select the concrete platform with `#[cfg]` and reuse the same `src/app.rs` scenario (generic over the existing `WifiTransport` and `WifiAccessPoint` traits). `src/platform/esp32.rs` alone owns `esp-hal`, Embassy startup, the physical Wi-Fi peripheral and socket resource sizing. No new HAL abstraction is added. The platform driver already contains the AP DHCP server; no HTTP server or web form is started. Credentials are application/build inputs, not responsibilities of the Wi-Fi driver. For persisted credentials, compose `iobewi-wifi-manager` with `ConfigSpace` in your product.

## Source layout

```text
src/
├── app.rs                 # portable STA / AP / AP+STA scenario
├── platform/
│   └── esp32.rs          # ESP32 + Embassy + Wi-Fi transport wiring
└── bin/
    ├── sta.rs            # select platform + mode
    ├── ap.rs             # select platform + mode
    └── apsta.rs          # select platform + mode
```

Additional MCUs will add their own platform module and feature selection, without changing `app.rs`. No RP or Teensy platform is currently implemented.

## Builds (examples, not yet verified)

Select exactly one chip feature. Install the relevant ESP target toolchain and linker/bootloader setup. Set credentials in the **build environment** (values will be embedded in the firmware image: not a secret storage mechanism).

```sh
export IOBEWI_AP_PASSWORD='replace-with-a-unique-secret'
export IOBEWI_STA_SSID='your-ssid'
export IOBEWI_STA_PASSWORD='your-wifi-password'

cargo build --release -p example-wifi-modes --bin wifi-sta --features esp32c3 --target riscv32imc-unknown-none-elf
cargo build --release -p example-wifi-modes --bin wifi-ap --features esp32c3 --target riscv32imc-unknown-none-elf
cargo build --release -p example-wifi-modes --bin wifi-apsta --features esp32c3 --target riscv32imc-unknown-none-elf

cargo +esp build --release -p example-wifi-modes --bin wifi-apsta --features esp32s3 --target xtensa-esp32s3-none-elf
```

**Provisioning:** this example intentionally does not persist STA credentials. No ConfigSpace or flash is initialized. That is covered by `examples/config-manager` and the reusable portable Wi-Fi manager.

## Hardware validation checklist

1. Flash `wifi-sta`: verify successful association and IPv4 via the upstream network DHCP client.
2. Flash `wifi-ap`: connect a phone/laptop using the build-time WPA2 password; inspect the client's assigned address, gateway and DNS (AP should offer neither).
3. Flash `wifi-apsta`: verify both interfaces, test AP client's lease while the station connects, and observe channel changes/disconnects during STA association.
4. Exercise AP stop/start and STA reconnect independently after nominal bring-up; measure heap/stack and inspect DHCP socket cleanup.
5. Rebuild for both C3 and S3, then repeat appropriate hardware tests.

**Status:** source-only reference examples; no Cargo builds or physical radio/DHCP acceptance has been performed in this migration session. A build alone does not prove that client DHCP works. Existing source has assertions but no operational console trace backend; hardware validation must include suitable diagnostics.
