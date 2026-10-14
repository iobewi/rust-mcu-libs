//! ESP32-C3 / ESP32-S3 target entry and storage integration.
use embassy_executor::Spawner;
use esp_hal::timer::timg::TimerGroup;
use iobewi_config_space::storage::{SpaceSpec, Store};
use iobewi_esp_config_space::{EspConfigFlash, FlashPeripheral};
use static_cell::StaticCell;

esp_bootloader_esp_idf::esp_app_desc!();

#[embassy_executor::task]
async fn config_task(flash: FlashPeripheral<'static>) {
    let shared = iobewi_esp_flash::init(flash);
    let flash = EspConfigFlash::from_label(shared, "config")
        .await
        .expect("config partition unavailable");
    let blank = flash.is_blank().await.expect("config read failed");
    static STORE: StaticCell<Store<EspConfigFlash>> = StaticCell::new();
    const SPACES: &[SpaceSpec] = &[SpaceSpec::new("demo", 32)];
    let store = STORE.init(
        Store::new(flash, esp_hal::rng::Rng::new().random(), SPACES)
            .expect("config admission failed"),
    );
    if blank {
        store.format().await.expect("config initialization failed");
    }
    store
        .mount()
        .await
        .unwrap_or_else(|error| {
            log::error!(target: "example_config_manager", "config: mount failed {error:?}; explicit reset required");
            panic!("config mount failed");
        });
    crate::app::run(store.backend()).await;
    core::future::pending::<()>().await;
}

#[esp_hal::main]
fn main() -> ! {
    let peripherals = esp_hal::init(esp_hal::Config::default());
    iobewi_log::install(iobewi_esp_console::console_print, "example_config_manager");
    esp_alloc::heap_allocator!(size: 64 * 1024);
    let flash = peripherals.FLASH;
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);

    static EXECUTOR: StaticCell<esp_rtos::embassy::Executor> = StaticCell::new();
    let executor = EXECUTOR.init(esp_rtos::embassy::Executor::new());
    executor.run(|spawner: Spawner| {
        spawner.spawn(config_task(flash).unwrap());
    })
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
