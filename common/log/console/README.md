# Portable console contract

A console is a byte sink. This `no_std` crate defines how a platform names one and how text reaches it; it owns no hardware, no buffer, no policy and no logger.

- `Console`: `write_bytes(&self, &[u8])` and `flush(&self)`. Best effort and infallible; needs no runtime, allocator or logger of its own, so a bootloader or a panic handler can write to it.
- `Writer`: `core::fmt::Write` over any `Console`.
- `ConsoleBackend`: `None | Uart(u8) | UsbSerialJtag` (`#[non_exhaustive]`). Which of them an MCU supports is the architecture crate's concern.
- `PRIMARY` / `STAMP`: the platform's **primary console**, set at build time by `IOBEWI_CONSOLE_PRIMARY` (`uart<N>`, `usb-serial-jtag` or `none`; default `uart0`). Anything else fails the build. Every image of the platform (bootloader and application) reads the same variable.
- `LineBuf<N>`: formats a message into a fixed buffer (cut with `...` past `N`) so a connector writes it in one go.
- `ring` module: portable transmit ring (`Ring`, `Platform`) with atomic whole-message deposit, a pump for the device, an emergency freeze, loss counting and a `loom` model of the freeze protocol.
- `write_record` (feature `log`): the `LEVEL - message` line of a `log` record, for any console.

It does not depend on `iobewi-log`: the logger consumes a console, never the reverse. See `docs/firmware/console.md`.
