# WS2812 example

Shows red, green, blue, off, then yellow, cyan, magenta, orange and white (one second each, low
intensity) on the board's RGB LED, in a loop. It exercises `iobewi-esp-ws2812` and is the support
for its hardware qualification. It uses no executor, no logging and no network.

The caller composes the resources: `Rmt::new(RMT, RMT_FREQ)`, `rmt.channel0` and the LED pin go
into `Ws2812::new`. The driver has no pin number.

## Boards and LED pin

| Build | Board | LED pin |
| --- | --- | --- |
| `--features esp32s3` | ESP32-S3-DevKitC-1 **v1.0** | GPIO48 |
| `--features esp32s3,devkitc-1-v1-1` | ESP32-S3-DevKitC-1 **v1.1** | GPIO38 |
| `--features esp32c3` | ESP32-C3-DevKitM-1 | GPIO8 |

Both DevKitC-1 revisions are on sale and differ by this pin only (Espressif user guides). Choose
the build that matches the board; a wrong choice leaves the LED dark. See
[ESP32 GPIO reference](../../docs/hardware/esp32-gpio.md). The LED of the C3-DevKitM-1 is on
GPIO8, a strapping pin.

## Build

```sh
# ESP32-S3 (use +esp)
cargo +esp build -Zbuild-std=core,alloc --target xtensa-esp32s3-none-elf \
  -p example-ws2812 --features esp32s3 --bins --release
# ESP32-C3
cargo +nightly build -Zbuild-std=core,alloc --target riscv32imc-unknown-none-elf \
  -p example-ws2812 --features esp32c3 --bins --release
```

The CI profile is `esp32-linked-bins`, so both are built and linked there.

## Flashing warning

This is a bare image. Writing it with a full flash (bootloader, partition table and application)
replaces the partition table and the application of whatever the board carries, which destroys a
product layout with its stored configuration. Use a board with nothing to keep, or back the board
up first.

## Memory

Measured on this example, release build, against the same program without the LED (`size`; the
difference includes the `esp-hal` RMT code pulled in):

| | text | data | bss |
| --- | --- | --- | --- |
| ESP32-C3 | +7 462 | +1 468 | +4 |
| ESP32-S3 | +10 604 | +2 700 | -2 700 |

On the S3 the data and bss figures move together because of how the linker places the sections,
so only their sum means anything: data + bss is unchanged there. The driver object itself is 132 bytes (`rustc -Zprint-type-sizes`,
C3): the 26-pulse RMT buffer (104 bytes) plus the channel and pulse codes. There is no heap
allocation.

## Hardware qualification

Not run yet. Compilation and the host test of the write guard are not hardware evidence. What the
historical code in `iobewi_old` recorded on an ESP32-S3 (GPIO48 board, so a v1.0): GRB order
confirmed visually, 80 MHz RMT clock and `WS2812_TIMING`. This example repeats the checks: the red
step must look red (a wrong colour order shows green), then green, blue, no light between them, no
flicker. C3: compile only.
