#![no_std]
#![no_main]

#[cfg(all(feature = "esp32c3", feature = "esp32s3"))]
compile_error!("Select exactly one chip");
#[cfg(not(any(feature = "esp32c3", feature = "esp32s3")))]
compile_error!("Select --features esp32c3 or esp32s3");
extern crate alloc;
#[path = "../app.rs"]
mod app;

#[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
#[path = "../platform/esp32.rs"]
mod platform;

esp_bootloader_esp_idf::esp_app_desc!();

#[esp_hal::main]
fn main() -> ! {
    platform::boot(app::Mode::Sta)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
