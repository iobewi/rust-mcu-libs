# Console connector and shared primary console (issue #102)

## Layers

| Layer | Crate | Role |
| --- | --- | --- |
| contract | `common/log/console` (`iobewi-console`) | `Console` byte sink, `ConsoleBackend`, the platform's primary console and its stamp. No hardware, no logger |
| connector | `arch/esp32/log/console` (`iobewi-esp-console`) | ESP32 transports: UART0, USB Serial/JTAG, none; optional UART1 secondary |
| application logger | `common/log/core` (`iobewi-log`) | levels, filtering, ring, distribution. Receives the connector as a `fn(&Record)`; unchanged |

The bootloader uses the connector directly (HALT, panic, diagnostics) and links neither `iobewi-log` nor the ring, the configuration manager or the network. The connector never depends on `iobewi-log`.

## Why it exists

The second stage printed on USB Serial/JTAG (`esp-println/jtag-serial`) while the bench console and the application use UART0. The application used `esp-println` in `auto` mode, which picks the transport per write from a USB "start of frame" flag that never resets: the output of one board depended on whether a USB host had been seen.

## One setting for the platform

`IOBEWI_CONSOLE_PRIMARY` (`uart<N>`, `usb-serial-jtag`, `none`; default `uart0`) is read by `iobewi-console`'s build script, the same way for every image built in the platform's environment. It becomes `PRIMARY` and the stamp `IOBEWI-CONSOLE primary=<console>\0` that each image keeps.

- `iobewi-esp-console` fails the build if the variable names a console the chip cannot use as primary (`uartN`, N ≠ 0) or one other than the `primary-*` feature it was built with.
- `esp_console.py check <images>` (CI, after the bootloader and the OTA application are built) fails if an image has no stamp or if stamps differ.

Both images must therefore be built with the same environment; a build with a different value fails or is refused at packaging, instead of producing a silent board.

### Limits

- The check covers images built and packaged together. An application distributed on its own (OTA built elsewhere) carries a stamp, but nothing at run time compares it with the installed second stage: whoever packages it must run `esp_console.py check` against the bootloader of the target platform. An OTA must not be used to change the system console; that is a re-provisioning of the second stage.
- The ROM has its own console output, which this software does not control.
- UART0 is the ROM console: GPIO and baud rate are the ROM's, the connector does not configure them.

## Selecting another console

```sh
IOBEWI_CONSOLE_PRIMARY=usb-serial-jtag cargo build -p iobewi-esp-bootloader --no-default-features \
  --features esp32s3,console-usb-serial-jtag --profile bootloader
```

The application does the same with `-p example-firmware-ota-system --features esp32s3,firmware,console-usb-serial-jtag`.

## Panic and HALT

The primary console is stateless, so it works before `esp_hal::init`, without logger and without error handling (no circular dependency). The bootloader prints `boot: PANIC` and its HALT reasons through it; the application's panic handler prints the panic message. If a panic interrupts an ongoing write, output can interleave. The panic location is not printed by the bootloader: keeping every panic site's file name costs about 2.5 KiB of a 32 KiB image.

## Secondary consoles

See `arch/esp32/log/console/README.md`. The connector only moves bytes; fan-out remains the application's composition (`print` function passed to `iobewi_log::install`).

## What is validated how

| Property | Evidence |
| --- | --- |
| variable parsing, stamp, `Writer` | host tests of `iobewi-console` |
| rejected combinations (variable ≠ feature, `uart1`, no feature, two features) | cross-compile failures (see PR) |
| stamp agreement bootloader/application | `esp_console.py` and its tests, CI step |
| output on the wire (UART0, USB Serial/JTAG, none), HALT/panic, ROM/bootloader/application ordering | **bench only** (not covered by software tests) |
| UART1 secondary output | compiles only; needs a USB-UART adapter on the chosen pin, which the bench does not have |

## Impact (ESP32-S3, measured; bootloader profile / release application)

| | before | after |
| --- | --- | --- |
| bootloader image (`.bin`) | 29808 B | 29728 B |
| bootloader `.data` / `.bss` | 1452 / 96 B | 1452 / 92 B |
| bootloader `.text` / `.rodata` | 20193 / 1868 B | 20085 / 1896 B |
| application flash (`elf_size.py`) | 597975 B | 598927 B (+952 B: panic message printing) |
| application `.data` / `.bss` | 9876 / 192092 B | 9868 / 192092 B |

The stamp is about 30 bytes per image. Stack use was not measured separately: the primary console adds no frame beyond the `esp-println` call.
