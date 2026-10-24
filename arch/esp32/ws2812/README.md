# ESP32 WS2812

`iobewi-esp-ws2812` shows a colour on one WS2812 LED through the RMT peripheral of the ESP32-C3 or
ESP32-S3. It adds no policy: no status, no pattern, no brightness, no task, no logging, no
allocation. The caller says which colour, when.

It is a thin adapter over [`esp-hal-smartled`](https://crates.io/crates/esp-hal-smartled) 0.18,
which does the RMT work, and carries the hardware settings qualified on an ESP32-S3 in
`iobewi_old`: 80 MHz RMT clock, GRB order, `WS2812_TIMING`.

```rust,ignore
let rmt = Rmt::new(peripherals.RMT, iobewi_esp_ws2812::RMT_FREQ)?;
let mut led = Ws2812::new(rmt.channel0, pins.rgb_led)?;

led.set(RGB8::new(32, 0, 0))?; // red
led.off()?;
```

Enable `esp32c3` or `esp32s3`; without a chip feature only the error type and the write guard
exist (this is what the host tests run).

## What the caller owns

- The RMT peripheral and its clock: the `Rmt` must be created with `RMT_FREQ`. The driver cannot
  read the rate back from the channel; another rate gives wrong pulse widths.
- The TX channel: the driver takes the channel it is given and does not choose one. The RX-only
  channels of the S3 do not satisfy `TxChannelCreator`, so they do not compile.
- The data pin, passed as `impl PeripheralOutput`. The crate holds no GPIO number. Which pin the LED
  is wired to is board knowledge; see [ESP32 GPIO reference](../../../docs/hardware/esp32-gpio.md).
  The DevKitC-1 LED is on GPIO48 (v1.0) or GPIO38 (v1.1).
- The colour order is fixed to GRB. A strip with another order is not supported.

## Errors

The first failed write returns `WriteError::Transmit` and disables the driver. Every later call
returns `WriteError::Disabled` without touching the hardware.

Why: in `esp-hal-smartled` 0.18 the blocking write takes the RMT channel out of the driver, and if
the transmission fails to start the channel is not put back; the next write would panic on the
missing channel. A status LED must never take the firmware down, so no write is attempted after an
error. The driver has no `unwrap` or `expect` on a recoverable path; the firmware decides whether
to log the error or ignore it. A disabled driver stays disabled until it is rebuilt.

Limits: a blocking write waits for the end of the transmission (24 bits of about 1.25 us, so
about 30 us, plus the 80 us reset). `esp-hal` gives no time limit on that wait. Panics inside `esp-hal` itself are not
covered by this guard.

## Memory

No allocation. The driver holds a 26-pulse RMT buffer (104 bytes) plus the channel and a few words.
The RMT memory used is one block of the channel it is given.

## Validation

- Host unit tests: the guard, with a writer that fails on demand.
- Library compile for ESP32-C3 and ESP32-S3 (`host-esp32` CI profile).
- [`examples/ws2812`](../../../examples/ws2812) links for both chips.
- Colour order, timings and the RMT clock were checked on an ESP32-S3 DevKitC-1 in `iobewi_old`
  (GRB confirmed visually). Evidence for this crate is in the example README. There is no C3
  hardware qualification.
