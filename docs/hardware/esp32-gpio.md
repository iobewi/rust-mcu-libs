# ESP32 GPIO: SoC capabilities, board wiring and composition

Reference for issue #77. It documents what the SoC imposes, what a board wires, and how an
application hands typed resources to library components. It is documentation only: there is no
GPIO crate, no board profile and no mapping API in this repository, and none is planned.

Sources and what was checked:

| Fact | Source |
| --- | --- |
| Exposed GPIO, RMT channels, `esp-hal` types | `esp-hal` 1.2.2 and `esp-metadata-generated` 0.5.3 (the versions in the workspace lock) |
| Strapping pins, USB pins, flash/PSRAM pins | Espressif ESP32-S3 Series Datasheet v2.2 and ESP32-C3 Series Datasheet; ESP-IDF GPIO API reference (esp32s3, esp32c3) |
| Board LED and wiring | Espressif ESP32-S3-DevKitC-1 user guide (v1.0 and v1.1) and ESP32-C3-DevKitM-1 user guide |

Anything marked **(not stated by Espressif)** is an inference, to be confirmed on the schematic or
the board before relying on it.

## Three levels, three owners

| Level | Defines | Lives in |
| --- | --- | --- |
| SoC | Which GPIO exist, which signals are fixed, which pins have a boot-time role | The datasheet and the `esp-hal` types |
| Board | What is soldered to which pin: LED, button, USB-UART bridge, module flash/PSRAM | The board composition of the application or example |
| Application | Which function receives which pin | The application, by moving typed resources into components |

A library component never chooses a pin. It receives the `esp-hal` resource it needs from its
caller and states its requirements in its README (see `arch/esp32/usb`).

## What `esp-hal` already guarantees

- Every pin is its own type (`peripherals::GPIO48<'d>`) and is moved, not borrowed by default.
  Using the same pin for two functions is a compile error. This is the conflict detection.
- `Pin::degrade()` and `AnyPin` erase the pin type when a component must accept any pin.
- Routable signals accept `impl PeripheralOutput` or `impl PeripheralInput` (GPIO Matrix), so a
  component taking those traits works with any pin that the chip can route.
- Fixed signals take the concrete pin types. `arch/esp32/usb` takes `GPIO20` and `GPIO19`, so a
  different pin does not compile.
- RMT transmit goes through `Rmt::new(RMT, Rate)`, then a TX-capable channel creator, then
  `.with_pin(impl PeripheralOutput)`. `TxChannelCreator` is only implemented for TX channels, so
  taking an RX-only channel fails to compile.

What `esp-hal` does **not** guard: `peripherals.GPIO26` exists on the S3 although that pin carries
the flash. Strapping, flash and PSRAM pins are not marked in the types. Avoiding them is a matter
of this document and of review.

## SoC capabilities

| | ESP32-S3 | ESP32-C3 |
| --- | --- | --- |
| GPIO exposed by `esp-hal` | 0-21, 26-48 (22-25 do not exist) | 0-21 |
| Input-only pins | none | none |
| GPIO Matrix routing of RMT and UART | yes, any pin | yes, any pin |
| RMT TX channels | 4 (channels 0-3); RX channels 4-7 | 2 (channels 0-1); RX channels 2-3 |
| UART0 default (IO MUX) | TX GPIO43, RX GPIO44 | TX GPIO21, RX GPIO20 |
| JTAG pad signals (IO MUX) | MTCK 39, MTDO 40, MTDI 41, MTMS 42 | MTMS 4, MTDI 5, MTCK 6, MTDO 7 |
| Strapping pins | GPIO0, GPIO3, GPIO45, GPIO46 | GPIO2, GPIO8, GPIO9 |
| Strapping defaults | GPIO0 weak pull-up; GPIO3 floating; GPIO45 and GPIO46 weak pull-down | GPIO2 and GPIO8 floating; GPIO9 weak pull-up |
| Flash and PSRAM pins | GPIO26-32 (SPI0/1) not recommended for other uses; GPIO33-37 also taken with octal flash or PSRAM (ESP32-S3R8, R8V, WROOM-2) | GPIO12-17 (SPI0/1) not recommended for other uses |
| Native USB | USB_D- GPIO19, USB_D+ GPIO20: full-speed OTG **and** USB Serial/JTAG | USB_D- GPIO18, USB_D+ GPIO19: USB Serial/JTAG only, no OTG |

Notes:

- Strapping pins are sampled at reset and become ordinary IO afterwards. The constraint is the
  level present at reset: an external circuit that pulls the pin the wrong way at power-up changes
  the boot mode (GPIO0 and GPIO46 on the S3, GPIO2, GPIO8 and GPIO9 on the C3).
- The S3 datasheet lists additional boot roles: GPIO45 selects the VDD_SPI voltage, GPIO46 and
  GPIO8 (C3) the ROM message printing, GPIO3 the JTAG source.
- The USB pins are connected to the USB Serial/JTAG controller by default. On the S3 the same two
  pins also serve the OTG PHY. Reconfiguring them as plain GPIO disables USB-JTAG.
- Whether a given module exposes GPIO26-37 depends on the module variant, not on the S3 itself.
  Check the module datasheet of the board in use.

## USB is not remappable

The internal PHY fixes the pins. On the ESP32-S3, D+ is GPIO20 and D- is GPIO19, whatever the
board silkscreen says, and `arch/esp32/usb` receives `USB_FS`, `GPIO20` and `GPIO19` as concrete
types. The ESP32-C3 has no OTG controller, so that crate is S3-only and the C3 offers only the
USB Serial/JTAG on GPIO18 and GPIO19. Nothing here makes USB pins selectable.

## Boards

### ESP32-S3-DevKitC-1 (reference board)

| Function | Pin | Notes |
| --- | --- | --- |
| Addressable RGB LED | **GPIO48 on v1.0, GPIO38 on v1.1** | **Depends on the board revision.** Both revisions are on the market. Identify the revision before assigning the LED |
| BOOT button | GPIO0 (not stated by Espressif; the guide only calls it the download button, and GPIO0 is the boot-mode strapping pin) | Confirm on the schematic |
| USB-to-UART port | UART0 (GPIO43 TX, GPIO44 RX are the UART0 defaults and the header TX/RX pins) | The bridge wiring itself is not spelled out in the guide |
| USB port (native) | GPIO19 D-, GPIO20 D+ | Full-speed OTG, also used for JTAG debugging |
| Reset | EN, not a GPIO | Reset button; no `esp-hal` type, never assignable |
| Octal flash or PSRAM variants | GPIO35, GPIO36, GPIO37 unavailable | Boards with octal-memory WROOM-1 modules and WROOM-2 |

Composition example, with the pins moved from the `esp-hal` peripherals and named by role:

```rust
use esp_hal::peripherals::{GPIO0, GPIO19, GPIO20, GPIO43, GPIO44, GPIO48};

/// Board wiring of an ESP32-S3-DevKitC-1 v1.0. The application owns it.
pub struct Pins {
    pub rgb_led: GPIO48<'static>, // GPIO38 on v1.1
    pub boot: GPIO0<'static>,
    pub uart_tx: GPIO43<'static>,
    pub uart_rx: GPIO44<'static>,
    pub usb_dp: GPIO20<'static>,
    pub usb_dm: GPIO19<'static>,
}

let p = esp_hal::init(esp_hal::Config::default());
let pins = Pins {
    rgb_led: p.GPIO48,
    boot: p.GPIO0,
    uart_tx: p.GPIO43,
    uart_rx: p.GPIO44,
    usb_dp: p.GPIO20,
    usb_dm: p.GPIO19,
};
```

StreamBeWI already composes its pins this way (button, UART0 and USB pins in its ESP32 target).

### ESP32-C3-DevKitM-1 (documentary example, not qualified on hardware)

| Function | Pin | Notes |
| --- | --- | --- |
| Addressable RGB LED | GPIO8 | A strapping pin. The board is designed around it, but any other circuit added to GPIO8 must leave its reset level intact |
| BOOT button | GPIO9 (not stated by Espressif; GPIO9 is the boot-mode strapping pin, weak pull-up) | Confirm on the schematic |
| USB-to-UART bridge | UART0 (GPIO21 TX, GPIO20 RX) | Micro-USB port |
| USB Serial/JTAG | GPIO18 D-, GPIO19 D+ | No OTG |
| Flash | GPIO12-17 | Not broken out to the headers |
| Reset | EN, not a GPIO | |

```rust
use esp_hal::peripherals::{GPIO8, GPIO9, GPIO20, GPIO21};

pub struct Pins {
    pub rgb_led: GPIO8<'static>, // strapping pin
    pub boot: GPIO9<'static>,    // strapping pin
    pub uart_tx: GPIO21<'static>,
    pub uart_rx: GPIO20<'static>,
}
```

This composition was not run on a board. The RMT for the C3 has two TX channels, not four.

### Other boards

The Waveshare ESP32-S3 Zero and other boards are not documented here: no wiring was verified. Add
a section only with a source and, if it is used, a hardware check.

## Rules for components and applications

1. A component states its pin requirements in its README, and takes the `esp-hal` resource as a
   parameter. No pin number appears in a component.
2. Take `impl PeripheralOutput` or `impl PeripheralInput` when the chip can route the signal. Take
   the concrete pin type only when the hardware fixes it.
3. Do not pass strapping, flash or PSRAM pins to a component without a documented reason.
4. Keep the board `Pins` structure in the application or example. This repository holds no board
   profiles.
5. A feature that exists on one SoC only (such as OTG on the S3) is gated by that chip's feature
   and says so in its README.
