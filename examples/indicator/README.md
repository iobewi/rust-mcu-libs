# Indicator example

Shows the patterns of [`iobewi-indicator`](../../common/indicator) on the board's WS2812 LED
through [`iobewi-esp-ws2812`](../../arch/esp32/ws2812), in a loop. It needs no `common/state`, no
executor and no logging: a blocking loop that asks the engine when the colour can next change and
sleeps until then. It is the support for the hardware qualification of the indicator.

The caller owns everything: the RMT and its TX channel, the LED pin, the clock (`esp_hal` time in
milliseconds) and the wait. Neither crate holds a pin number.

## The show

| # | Pattern | Intensity | Duration |
| --- | --- | --- | --- |
| 1 | steady red | 32 | 2 s |
| 2 | green blink, 150 ms lit / 150 ms dark | 32 | 3 s |
| 3 | blue blink, 500 / 500 ms | 32 | 4 s |
| 4 | blue pulse, 2 s period (full, dark, full) | 48 | 5 s |
| 5 | orange pulse, 2 s period, **cut after 1.3 s** | 48 | 1.3 s |
| 6 | orange blink, 150 / 150 ms | 48 | 3 s |
| 7 | off | - | 2 s |
| 8 | steady white | 8 | 1.5 s |
| 9 | the same steady white, brighter | 64 | 1.5 s |

Scene 5 changes the pattern in the middle of an animation: the orange pulse starts at once, at
full. Scene 9 asks for the same pattern as scene 8 with another intensity: the colour changes, the
pattern does not restart. In every scene the loop asks for its pattern again at each pass, which
must not restart an animation. A failed write stops the show (the LED driver has disabled itself);
the firmware would go on without the LED.

## Boards and LED pin

| Build | Board | LED pin |
| --- | --- | --- |
| `--features esp32s3` | ESP32-S3-DevKitC-1 **v1.0** | GPIO48 |
| `--features esp32s3,devkitc-1-v1-1` | ESP32-S3-DevKitC-1 **v1.1** | GPIO38 |
| `--features esp32c3` | ESP32-C3-DevKitM-1 | GPIO8 |

Choose the build that matches the board; a wrong choice leaves the LED dark. See the
[ESP32 GPIO reference](../../docs/hardware/esp32-gpio.md).

## Build

```sh
# ESP32-S3 (use +esp)
cargo +esp build -Zbuild-std=core,alloc --target xtensa-esp32s3-none-elf \
  -p example-indicator --features esp32s3 --bins --release
# ESP32-C3
cargo +nightly build -Zbuild-std=core,alloc --target riscv32imc-unknown-none-elf \
  -p example-indicator --features esp32c3 --bins --release
```

The CI profile is `esp32-linked-bins`: both are built and linked there.

## Flashing warning

This is a bare application image. Write it to the application partition only (for example
`factory` at 0x50000) and never erase the whole flash: a full flash with a partition table
replaces the layout and the stored configuration of the board.

## Memory

Measured on release builds against `examples/ws2812` (same driver, no engine, no show; `size`):

| | text | data | bss |
| --- | --- | --- | --- |
| ESP32-C3 | +1 188 | 0 | 0 |
| ESP32-S3 | +984 | 0 | 0 |

The engine is 20 bytes of state (`Indicator`), 8 bytes per `Pattern`. No allocation.

## Hardware qualification

See the pull request for what was run. Compilation and the host tests of the engine are not
hardware evidence.
