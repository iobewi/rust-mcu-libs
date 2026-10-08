#![no_std]
#![no_main]

// The application scenario is common. Target-specific entry points and
// storage initialization are selected at compile time.
#[cfg(all(feature = "esp32c3", feature = "esp32s3"))]
compile_error!("Select exactly one MCU feature");
#[cfg(not(any(feature = "esp32c3", feature = "esp32s3")))]
compile_error!("Select an implemented MCU feature: esp32c3 or esp32s3");

extern crate alloc;

mod app;

#[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
#[path = "platform/esp32.rs"]
mod platform;
