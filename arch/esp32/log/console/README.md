# ESP32 console connector

Physical console for ESP32-C3 and ESP32-S3, built on `esp-println` and the portable contract of `iobewi-console`. It does **not** install a competing global logger.

## Primary console

`Primary` is zero-sized and stateless: no initialisation, no allocator, no runtime, no `iobewi-log`. A second-stage bootloader and a panic handler can use it without setting anything up. Observed on the bench: bootloader output on UART0 after `esp_hal::init`. Not measured: output before `esp_hal::init`.

### Concurrency

| Context | Guarantee |
| --- | --- |
| application, feature `lock` (default) | each chunk of at most 64 bytes is written under `esp-println`'s lock: interrupts masked, other core excluded, reentrant on the same core. Bytes are never torn; a message of 64 bytes or less is never split. A longer one can be interleaved with another writer's at a chunk boundary |
| `console_print` / `panic_print` | format the record into a 256-byte stack buffer first (longer: cut, ends with `...`), then write it, so concurrent messages interleave whole chunks, not formatting fragments |
| bootloader (`default-features = false`, no `lock`) | one core, no interrupts: no lock needed, none taken |
| interrupt handler | an interrupt cannot run while the lock is held (it is masked); the lock is keyed by core, not by context, so a handler above the masked level (S3 level 6/7, NMI) re-enters like the owner and may interleave bytes |
| cost | esp-println flushes the UART every 32 bytes and after each call (ROM `uart_tx_flush`), so a write lasts its wire time with interrupts masked: 64 bytes ≈ 5.6 ms at 115200 baud, a 256-byte line ≈ 22 ms in four chunks (interrupts open for a few cycles between chunks). This is not "FIFO fill time" |
| panic | no extra lock: reentrant on the same core, so a panic inside a write does not deadlock. On ESP32-S3 a panic on one core waits for the other core to release (one chunk, ~5.6 ms); if the other core died while holding the lock the message never appears and the watchdog resets. Not excluded, not tested |

`print!`/`println!` re-exported from `esp-println` hold the lock for the whole line; prefer `console_print` where interrupts must not stay masked that long. No writer waits for another outside that lock, so there is no lock-ordering deadlock; a stuck UART would stall the writer holding the lock, as it did before.

### Panic output

`panic_print(info)` writes `PANIC: <info>` and depends on nothing but the primary console (no `iobewi-log`). The handler keeps deciding what happens next.

One transport per image, chosen at build time:

| `IOBEWI_CONSOLE_PRIMARY` | Cargo feature | Output |
| --- | --- | --- |
| `uart0` (default) | `primary-uart0` (default) | UART0 through the ROM routine; GPIO and baud rate are the ROM's |
| `usb-serial-jtag` | `primary-usb-serial-jtag` | native USB Serial/JTAG |
| `none` | `primary-none` | nothing |

The variable is the platform's setting and the feature is how this crate implements it (`esp-println` selects its transport by Cargo feature and cannot switch at run time). They must agree: otherwise the build fails. `uart1` as primary, no feature, or two features are compile errors; there is no silent fallback. The default features are `primary-uart0` and `lock` (below); a transport other than UART0, or no lock (the bootloader), needs `default-features = false` on the dependency (see the bootloader's `console-*` features).

An image keeps `core::hint::black_box(&iobewi_esp_console::STAMP)`; `.github/scripts/esp_console.py` compares the stamps of the images of a platform.

## Logger

```rust
iobewi_log::install(iobewi_esp_console::console_print, "my_application");
```

`console_print` writes `LEVEL - message` on the primary console. Other libraries emit through the `log` facade and initialise no logger. Filtering, capture and distribution stay in `common/log/core`.

## Secondary console (feature `uart-secondary`, application only)

`SecondaryUart::new(peripherals.UART1, pin, config)` takes UART1 and a pin by value: nobody else can hold them, and UART0 cannot be passed. Writes lock per non-blocking FIFO fill, never while waiting for FIFO space (interrupts masked for microseconds, bytes of concurrent writers can interleave at fill boundaries); a UART that stays full gives up after a bounded wait. `flush` does nothing: `write_bytes` returns once bytes are in the TX FIFO. It implements `Console`, so the application fans a record out to several consoles from its own `print` function; the connector never replaces the primary console. If UART0 is the primary console, the application must not take `peripherals.UART0` for another driver.

The ROM keeps its own console output before the bootloader runs; neither the ROM output nor the continuity of the physical peripheral across the jump to the application is controlled here. Physical validation of the serial output remains a bench matter.
