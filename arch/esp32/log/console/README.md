# ESP32 console connector

Physical console for ESP32-C3 and ESP32-S3, built on `esp-println` and the portable contract of `iobewi-console`. It does **not** install a competing global logger.

## Primary console

`Primary` is zero-sized and stateless: no initialisation, no allocator, no runtime, no `iobewi-log`. A second-stage bootloader writes to it from its first instruction and a panic handler can use it at any time (it takes no lock of its own).

One transport per image, chosen at build time:

| `IOBEWI_CONSOLE_PRIMARY` | Cargo feature | Output |
| --- | --- | --- |
| `uart0` (default) | `primary-uart0` (default) | UART0 through the ROM routine; GPIO and baud rate are the ROM's |
| `usb-serial-jtag` | `primary-usb-serial-jtag` | native USB Serial/JTAG |
| `none` | `primary-none` | nothing |

The variable is the platform's setting and the feature is how this crate implements it (`esp-println` selects its transport by Cargo feature and cannot switch at run time). They must agree: otherwise the build fails. `uart1` as primary, no feature, or two features are compile errors; there is no silent fallback. A transport other than UART0 needs `default-features = false` on the dependency (see the bootloader's `console-*` features).

An image keeps `core::hint::black_box(&iobewi_esp_console::STAMP)`; `.github/scripts/esp_console.py` compares the stamps of the images of a platform.

## Logger

```rust
iobewi_log::install(iobewi_esp_console::console_print, "my_application");
```

`console_print` writes `LEVEL - message` on the primary console. Other libraries emit through the `log` facade and initialise no logger. Filtering, capture and distribution stay in `common/log/core`.

## Secondary console (feature `uart-secondary`, application only)

`SecondaryUart::new(peripherals.UART1, pin, config)` takes UART1 and a pin by value: nobody else can hold them, and UART0 cannot be passed. It implements `Console`, so the application fans a record out to several consoles from its own `print` function; the connector never replaces the primary console. If UART0 is the primary console, the application must not take `peripherals.UART0` for another driver.

The ROM keeps its own console output before the bootloader runs; neither the ROM output nor the continuity of the physical peripheral across the jump to the application is controlled here. Physical validation of the serial output remains a bench matter.
