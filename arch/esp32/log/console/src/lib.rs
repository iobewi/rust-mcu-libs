#![no_std]

//! ESP console connector. The platform's primary console is stateless: it needs no
//! initialisation, no allocator, no runtime and no `iobewi-log`, so a second-stage bootloader
//! can use it from its first instruction and a panic handler can use it at any time. It knows
//! nothing about capture, streaming or policy; a platform passes [`console_print`] to the
//! logger at installation time.
//!
//! One transport per image, chosen at build time: `IOBEWI_CONSOLE_PRIMARY` names it, and the
//! `primary-*` feature of this crate implements it. They must agree, and the transport must
//! exist on the chip; anything else is a compile error, never a silent fallback.

use iobewi_console::ConsoleBackend;
pub use iobewi_console::{Console, Writer};

#[cfg(all(
    any(feature = "esp32c3", feature = "esp32s3"),
    feature = "primary-uart0",
    feature = "primary-usb-serial-jtag"
))]
compile_error!(
    "iobewi-esp-console: `primary-uart0` and `primary-usb-serial-jtag` are both enabled; use default-features = false to pick one"
);
#[cfg(all(
    any(feature = "esp32c3", feature = "esp32s3"),
    feature = "primary-none",
    any(feature = "primary-uart0", feature = "primary-usb-serial-jtag")
))]
compile_error!(
    "iobewi-esp-console: `primary-none` is enabled with another primary console; use default-features = false to pick one"
);
#[cfg(all(
    any(feature = "esp32c3", feature = "esp32s3"),
    not(any(
        feature = "primary-uart0",
        feature = "primary-usb-serial-jtag",
        feature = "primary-none"
    ))
))]
compile_error!(
    "iobewi-esp-console: select a primary console feature (primary-uart0, primary-usb-serial-jtag or primary-none)"
);
#[cfg(all(feature = "esp32c3", feature = "esp32s3"))]
compile_error!("iobewi-esp-console: select exactly one ESP target feature");

/// The transport this build implements.
pub const IMPLEMENTED: ConsoleBackend = if cfg!(feature = "primary-usb-serial-jtag") {
    ConsoleBackend::UsbSerialJtag
} else if cfg!(feature = "primary-none") {
    ConsoleBackend::None
} else {
    ConsoleBackend::Uart(0)
};

// The primary console of an image is a platform property, not a choice of this crate.
const _: () = assert!(
    !matches!(iobewi_console::PRIMARY, ConsoleBackend::Uart(n) if n != 0),
    "IOBEWI_CONSOLE_PRIMARY: only UART0 can be the primary console on ESP32 (the ROM console); other UARTs are secondary consoles"
);
const _: () = assert!(
    iobewi_console::PRIMARY.same(IMPLEMENTED),
    "IOBEWI_CONSOLE_PRIMARY does not match the primary-* feature of iobewi-esp-console: change one so that both name the same console"
);

/// Greppable stamp of the platform's primary console. An image keeps it with
/// `core::hint::black_box(iobewi_esp_console::STAMP)`; `esp_console.py` compares the images.
pub use iobewi_console::STAMP;

/// The platform's primary console. Zero-sized; safe to use before any initialisation.
#[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
#[derive(Clone, Copy, Debug, Default)]
pub struct Primary;

#[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
impl Console for Primary {
    fn write_bytes(&self, bytes: &[u8]) {
        esp_println::Printer::write_bytes(bytes);
    }

    fn flush(&self) {}
}

/// `println!` / `print!` on the primary console, for examples that print formatted text directly.
#[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
pub use esp_println::{print, println};

/// Writes `LEVEL - message` on the primary console, one line at a time.
#[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
pub fn console_print(record: &log::Record<'_>) {
    esp_println::println!("{} - {}", record.level(), record.args());
}

#[cfg(feature = "uart-secondary")]
mod secondary;
#[cfg(feature = "uart-secondary")]
pub use secondary::SecondaryUart;
