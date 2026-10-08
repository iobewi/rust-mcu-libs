//! ESP32-C3 / ESP32-S3 target entry and storage integration.
use embassy_executor::Spawner;
use esp_hal::{interrupt::software::SoftwareInterruptControl, timer::timg::TimerGroup};
use iobewi_esp_config_space::NvsConfigBackend;
use iobewi_esp_flash::SharedFlash;
use static_cell::StaticCell;

esp_bootloader_esp_idf::esp_app_desc!();

#[embassy_executor::task]
async fn config_task(flash: &'static SharedFlash) {
    let backend = NvsConfigBackend::from_label(flash, "nvs")
        .await
        .unwrap_or_else(|_| panic!("NVS partition unavailable"));
    crate::app::run(backend).await;
    core::future::pending::<()>().await;
}

#[esp_hal::main]
fn main() -> ! {
    let peripherals = esp_hal::init(esp_hal::Config::default());
    esp_alloc::heap_allocator!(size: 64 * 1024);
    let flash = iobewi_esp_flash::init(peripherals.FLASH);
    let sw_int = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);

    static EXECUTOR: StaticCell<esp_rtos::embassy::Executor> = StaticCell::new();
    let executor = EXECUTOR.init(esp_rtos::embassy::Executor::new());
    executor.run(|spawner: Spawner| {
        spawner.spawn(config_task(flash).unwrap());
    })
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop { core::hint::spin_loop(); }
}
