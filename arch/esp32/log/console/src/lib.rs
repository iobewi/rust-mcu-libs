#![no_std]

//! ESP console connector. The platform's primary console is stateless: it needs no
//! initialisation, no allocator, no runtime and no `iobewi-log`, so a second-stage bootloader
//! and a panic handler can use it without setting anything up. (Output has so far been
//! observed only after `esp_hal::init`; the ROM path it uses does not depend on it, but a
//! write before it is not measured.) It knows
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

/// Bytes written per lock acquisition. With the `lock` feature each chunk is atomic and masks
/// interrupts for its whole transmission: esp-println flushes the UART (ROM `uart_tx_flush`)
/// every 32 bytes and after each call, so a chunk costs its wire time (64 bytes: ~5.6 ms at
/// 115200 baud), not the time to fill the FIFO. Chunking bounds the interrupt latency, not
/// the total time masked while a long line is written.
#[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
const CHUNK: usize = 64;

/// Longest record, line end included; a longer one is cut and ends with `...`.
#[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
const LINE: usize = 256;

/// The platform's primary console. Zero-sized and stateless: it needs no initialisation, so it
/// can be used in a panic handler.
///
/// Concurrency: with the `lock` feature every chunk of at most 64 bytes is written under
/// esp-println's lock (interrupts masked, other core excluded, reentrant on the same core),
/// so bytes are never torn and a short message is never split. A message longer than a chunk can
/// be interleaved with another writer's at a chunk boundary. Without `lock` (bootloader: one
/// core, no interrupts) nothing is taken. There is no wait on another writer outside the
/// lock, hence no lock-ordering deadlock; the lock is held only while the UART drains.
#[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
#[derive(Clone, Copy, Debug, Default)]
pub struct Primary;

#[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
impl Console for Primary {
    fn write_bytes(&self, bytes: &[u8]) {
        for chunk in bytes.chunks(CHUNK) {
            esp_println::Printer::write_bytes(chunk);
        }
    }

    fn flush(&self) {}
}

/// `println!` / `print!` on the primary console, for examples that print formatted text
/// directly. With `lock`, `println!` holds the lock for the whole line: use [`console_print`]
/// or [`panic_print`] where interrupts must not stay masked that long.
#[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
pub use esp_println::{print, println};

/// Formats `args` into one line and writes it in chunks.
#[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
fn write_line(args: core::fmt::Arguments<'_>) {
    use core::fmt::Write as _;
    let mut line = iobewi_console::LineBuf::<LINE>::new();
    let _ = line.write_fmt(args);
    line.finish();
    Primary.write_bytes(line.as_bytes());
}

/// Writes `LEVEL - message` on the primary console, one line at a time.
#[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
pub fn console_print(record: &log::Record<'_>) {
    write_line(format_args!("{} - {}", record.level(), record.args()));
}

/// Minimal panic output: `PANIC: <info>` on the primary console. It depends on nothing but
/// the primary console (no `iobewi-log`, no allocator, no runtime); what the handler does next
/// (spin, reset, indicate) stays its own business.
#[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
pub fn panic_print(info: &core::panic::PanicInfo<'_>) {
    write_line(format_args!("PANIC: {info}"));
}

#[cfg(feature = "uart-secondary")]
mod secondary;
#[cfg(feature = "uart-secondary")]
pub use secondary::SecondaryUart;
