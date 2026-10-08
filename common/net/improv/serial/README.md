# Improv Serial codec (portable)

This crate is the portable, transport-independent protocol codec migrated from [iobewi/improv-serial](https://github.com/iobewi/improv-serial). It parses the Improv Serial byte stream and builds reply frames, using `no_std + alloc`. No UART, USB, Wi-Fi, HAL, or Embassy implementation is embedded in this crate.

The public Rust package remains named `improv-serial` to minimize downstream changes.

```rust
use improv_serial::{Parser, ParsedCommand};

let mut parser = Parser::new();
// Feed bytes received from a serial or USB transport.
for byte in b"unrelated output" {
    if let Some(ParsedCommand::WifiSettings(settings)) = parser.feed(*byte) {
        // Application decides how to persist/connect using the credentials.
        let _ = settings;
    }
}
```

An application owns serial I/O, provisioning authorization, Wi-Fi connection, credential storage and the response transmission. Future MCU adapters belong under `arch/<family>/` only when they add actual reusable integration value. No firmware or physical Improv qualification is claimed by this migration.

See [the original repository](https://github.com/iobewi/improv-serial) for its source history. Migration copies the codec and its existing unit tests; new development takes place in this workspace.
