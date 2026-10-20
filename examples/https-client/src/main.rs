#![no_std]
#![no_main]
#![recursion_limit = "256"]

#[cfg(not(feature = "esp32s3"))]
compile_error!("This example runs on ESP32-S3 only: build with --features esp32s3");

extern crate alloc;

mod mem;
mod platform;

esp_bootloader_esp_idf::esp_app_desc!();

#[esp_hal::main]
fn main() -> ! {
    // Before anything else: the stack is painted from here to measure its high-water mark.
    mem::paint();
    platform::boot()
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    log::error!("panic: {info}");
    loop {
        core::hint::spin_loop();
    }
}
