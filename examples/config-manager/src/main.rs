#![no_std]
#![no_main]

// Exactly one target feature must be selected.
#[cfg(all(feature = "esp32c3", feature = "esp32s3"))]
compile_error!("Select only one chip: esp32c3 or esp32s3");
#[cfg(not(any(feature = "esp32c3", feature = "esp32s3")))]
compile_error!("Select a target: --features esp32c3 or esp32s3");

extern crate alloc;

use embassy_executor::Spawner;
use esp_hal::{interrupt::software::SoftwareInterruptControl, timer::timg::TimerGroup};
use iobewi_config_space::{Budget, ConfigManager};
use iobewi_esp_config_space::NvsConfigBackend;
use iobewi_esp_flash::SharedFlash;
use static_cell::StaticCell;

esp_bootloader_esp_idf::esp_app_desc!();

#[embassy_executor::task]
async fn config_manager_demo(flash: &'static SharedFlash) {
    // The partition is discovered by label, not by a hardcoded address.
    // Flash access is shared through the one ESP32 flash owner.
    let backend = NvsConfigBackend::from_label(flash, "nvs")
        .await
        .expect("NVS partition missing or unavailable");

    let mut manager = ConfigManager::new(backend);
    let space = manager.claim("demo", Budget::new(32)).expect("configuration capacity");

    // On first boot, initialize the value. On later boots, read it back.
    // Do not erase or reformat NVS during startup.
    match space.load().await.expect("configuration read") {
        Some(snapshot) => {
            assert_eq!(snapshot.data.as_slice(), b"config-ready");
            core::hint::black_box(snapshot.generation);
        }
        None => {
            let generation = space.commit(b"config-ready").await.expect("configuration write");
            assert_eq!(generation, 1);
        }
    }

    // Verify the value through the same backend.
    let snapshot = space.load().await.expect("configuration verification").expect("missing data");
    assert_eq!(snapshot.data.as_slice(), b"config-ready");
    core::future::pending::<()>().await;
}

#[esp_hal::main]
fn main() -> ! {
    let peripherals = esp_hal::init(esp_hal::Config::default());

    // The only physical flash owner in the firmware.
    esp_alloc::heap_allocator!(size: 64 * 1024);
    let flash = iobewi_esp_flash::init(peripherals.FLASH);

    let sw_int = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);

    static EXECUTOR: StaticCell<esp_rtos::embassy::Executor> = StaticCell::new();
    let executor = EXECUTOR.init(esp_rtos::embassy::Executor::new());
    executor.run(|spawner: Spawner| {
        spawner.spawn(config_manager_demo(flash).unwrap());
    })
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop { core::hint::spin_loop(); }
}
