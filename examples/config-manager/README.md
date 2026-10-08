# Config manager — portable example

A minimal, runnable illustration of `iobewi-config-space` using an **in-memory backend** on a host computer.

It demonstrates independent claims for `wifi` and `device`, budget admission, an opaque configuration commit, snapshot generation, reading, and rejection of an oversized value.

```sh
cargo run -p example-config-manager
```

**Scope:** This example tests the *portable API only*. The backend does **not** persist data across runs and offers **no power-loss guarantees**; its capacity accounting is deliberately simplified. The local `run_ready` helper only accepts instantly-ready in-memory futures and must not be reused as an async runtime. For actual ESP32 persistence, compose `arch/esp32/config/space` with the flash and NVS bricks, and use Embassy as the async runtime.

This is a learning/reference example, **not a production-qualified firmware**.
