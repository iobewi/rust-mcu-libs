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

The primary console needs no state of ours: no logger, no allocator, no error handling (no circular dependency). **Observed on the bench**: bootloader lines on UART0 after `esp_hal::init`. **Not measured**: output before `esp_hal::init` (the ROM routine it uses does not depend on it, but this was not tested), forced HALT, forced panic.

- Bootloader: `boot: PANIC` and its HALT reasons go through the connector. The panic location is not printed: keeping every panic site's file name costs about 2.5 KiB of a 32 KiB image.
- Applications: `iobewi_esp_console::panic_print(info)` writes `PANIC: <info>` and nothing else. It does not use `iobewi-log`, so it works whether or not the logger is installed or filtering. `ota-system`, `https-client`, `https-health`, `config-manager`, `wifi-modes` and `state-indicator` use it; `indicator` and `ws2812` do not use the console and keep their silent handler.

## Synchronisation: what is guaranteed

| | |
| --- | --- |
| bytes | never torn (application, `lock`): every chunk of at most 64 bytes is written under `esp-println`'s lock |
| short messages | a message of 64 bytes or less is never split |
| long messages | formatted in a 256-byte buffer (cut with `...`), written in chunks; another writer can interleave at a 64-byte boundary |
| interrupts | masked for one chunk at a time, not a line |
| interrupt context, panic | allowed; the lock is reentrant on a core, so no self-deadlock; the other core's holder releases within one chunk |
| bootloader | no lock (one core, no interrupts) |
| `SecondaryUart` | locks per non-blocking FIFO fill, never while waiting; may interleave at fill boundaries; bounded wait on a stuck UART |

Limits: nothing makes a multi-chunk message atomic against another task (that would mean holding interrupts off for the whole message, or a blocking lock that is unsafe in interrupt and panic context); a UART that never drains stalls the writer holding the lock, as before. Whether the truncated lines seen in the old bench capture came from missing locking (the old build used none) or from the `auto` transport was not established.

## Secondary consoles

See `arch/esp32/log/console/README.md`. The connector only moves bytes; fan-out remains the application's composition (`print` function passed to `iobewi_log::install`).

## What is validated how

| Property | Evidence |
| --- | --- |
| variable parsing, stamp, `Writer` | host tests of `iobewi-console` |
| accepted selections and rejected combinations (variable ≠ feature, `uart1`, unknown value, no feature, two features, bootloader/application mismatch), ESP32-C3 and S3 | `console_matrix.py`, CI step in both MCU jobs (a refusal must fail with its reason) |
| line formatting and truncation | host tests of `iobewi-console` (`LineBuf`) |
| stamp agreement bootloader/application | `esp_console.py` and its tests, CI step |
| output on the wire (UART0, USB Serial/JTAG, none), HALT/panic, ROM/bootloader/application ordering | **bench only** (not covered by software tests) |
| locking, interleaving, interrupt and panic context | by construction (esp-println lock, reentrant); **not tested on hardware** |
| UART1 secondary output | compiles only; needs a USB-UART adapter on the chosen pin, which the bench does not have |

## Impact (ESP32-S3, measured; bootloader profile / release application)

| | before | after |
| --- | --- | --- |
| bootloader image (`.bin`) | 29808 B | 29760 B |
| bootloader `.data` / `.bss` | 1452 / 96 B | 1452 / 92 B |
| bootloader text+rodata (`size`) | 28251 B | 28203 B |
| application flash (`elf_size.py`) | 597975 B | 599079 B (+1104 B: panic message printing, `lock`, line buffer) |
| application `.data` / `.bss` (`size`) | 9892 / 462936 B | 9900 / 462928 B |

The stamp is about 30 bytes per image. `console_print` and `panic_print` put a 256-byte line buffer on the stack of the calling task (the panic handler runs on the panicking stack); stack high-water marks were not re-measured.
