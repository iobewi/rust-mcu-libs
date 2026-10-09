#![no_std]

//! ESP console sink: the raw physical output of one log record. It knows
//! nothing about log capture, streaming or any policy; a platform passes
//! [`console_print`] to the logger at installation time.

/// Writes `LEVEL - message` on the ESP console.
#[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
pub fn console_print(record: &log::Record<'_>) {
    esp_println::println!("{} - {}", record.level(), record.args());
}
