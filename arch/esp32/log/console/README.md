# ESP32 console logging sink

This ESP32-specific crate provides `console_print(&log::Record)` using `esp-println`. It does **not** install a competing global logger.

The firmware composition root installs the single `iobewi-log` logger once, before starting tasks:

```rust
iobewi_log::install(iobewi_esp_console::console_print, "my_application");
```

Other libraries emit through the upstream `log` facade and do **not** initialize any logger. Filtering, bounded capture, runtime Off/Debug/Trace policy and optional later ConfigSpace persistence are owned by `common/log/core` and `common/log/config`. This console sink does not require HTTP, network access, or streaming.

The feature-selected console targets ESP32-C3 and ESP32-S3. ESP boot/peripheral setup and validation of physical serial output remain application/platform responsibilities.
