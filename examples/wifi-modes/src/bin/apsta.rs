#![no_std]
#![no_main]

#[cfg(all(feature = "esp32c3", feature = "esp32s3"))]
compile_error!("Select exactly one chip");
#[cfg(not(any(feature = "esp32c3", feature = "esp32s3")))]
compile_error!("Select --features esp32c3 or esp32s3");
extern crate alloc;
#[path = "../shared.rs"]
mod shared;

esp_bootloader_esp_idf::esp_app_desc!();

#[esp_hal::main]
fn main() -> ! {
    shared::boot(shared::Mode::ApSta)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop { core::hint::spin_loop(); }
}
