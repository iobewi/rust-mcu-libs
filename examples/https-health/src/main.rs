#![no_std]
#![no_main]

#[cfg(all(feature = "esp32c3", feature = "esp32s3"))]
compile_error!("Choose exactly one MCU feature");
#[cfg(not(any(feature = "esp32c3", feature = "esp32s3")))]
compile_error!("Choose --features esp32c3 or esp32s3");

extern crate alloc;

#[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
mod platform;

esp_bootloader_esp_idf::esp_app_desc!();

#[esp_hal::main]
fn main() -> ! {
    platform::boot()
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
