#![no_std]
#![no_main]
#![recursion_limit = "256"]
extern crate alloc;
mod ota;
#[cfg(all(feature = "esp32c3", feature = "esp32s3"))]
compile_error!("choose exactly one MCU feature");
#[cfg(not(any(feature = "esp32c3", feature = "esp32s3")))]
compile_error!("choose esp32c3 or esp32s3");
#[cfg(all(feature = "reject-self-check", feature = "hang-self-check"))]
compile_error!("choose at most one self-check fault");

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_hal::timer::timg::TimerGroup;
use iobewi_config_space::{Budget, ConfigManager};
use iobewi_esp_config_space::NvsConfigBackend;
use iobewi_esp_firmware_a::{platform::SystemBoot, service::BootRuntime};
use iobewi_esp_flash::SharedFlash;
use iobewi_firmware_update::{
    config_space::ConfigSpaceMetadataStore,
    metadata::PendingCheckAction,
    service::boot::{self, BootSelfCheck, BootStatus, Confirmation},
};
use static_cell::StaticCell;

esp_bootloader_esp_idf::esp_app_desc!();
const WATCHDOG_MS: u64 = 20_000;
const SELF_CHECK_MS: u64 = 15_000;

struct Check<'a>(&'a NvsConfigBackend);
impl BootSelfCheck for Check<'_> {
    async fn self_check(&self) -> bool {
        if cfg!(feature = "hang-self-check") {
            core::future::pending::<()>().await;
        }
        let healthy = self.0.self_check().await;
        healthy && !cfg!(feature = "reject-self-check")
    }
}
async fn reset_for_rollback() -> ! {
    // Same settling delay as the historical application binding. Watchdog stays
    // armed even if rejection or this delay cannot complete.
    Timer::after(Duration::from_millis(200)).await;
    esp_hal::system::software_reset()
}
#[embassy_executor::task]
async fn boot_task(
    flash: &'static SharedFlash,
    peripheral: esp_hal::peripherals::WIFI<'static>,
    spawner: Spawner,
) {
    let backend = NvsConfigBackend::from_label(flash, "nvs")
        .await
        .expect("NVS unavailable");
    let mut manager = ConfigManager::new(backend);
    // Example owns its OTM1 claim; product must use its existing claim/budget.
    let ota = manager
        .claim("ota", Budget::new(2048))
        .expect("OTA claim failed");
    let store = ConfigSpaceMetadataStore(&ota);
    let check = Check(&backend);
    let mut system = SystemBoot::discover(flash)
        .await
        .expect("system discovery failed");
    {
        let runtime = BootRuntime::new(&mut system, flash, &check)
            .await
            .expect("boot observation failed");
        let result = boot::on_boot(&store, &runtime).await;
        log::info!("boot: running {} status {:?}", result.slot, result.status);
        match result.status {
            BootStatus::Rollback => reset_for_rollback().await,
            BootStatus::PendingVerify => {
                match boot::pending_check(&runtime, Duration::from_millis(SELF_CHECK_MS)).await {
                    PendingCheckAction::Confirm => {
                        match boot::confirm_pending(&store, &runtime).await {
                            Confirmation::Valid => log_status("confirmed"),
                            // Boot is already confirmed; OTM1 completion is retried at next boot.
                            Confirmation::Degraded => {
                                log_status("confirmed; metadata completion failed")
                            }
                            Confirmation::ResetRequired => reset_for_rollback().await,
                        }
                    }
                    PendingCheckAction::Reject => {
                        boot::reject_pending(&runtime).await;
                        reset_for_rollback().await;
                    }
                    PendingCheckAction::Reset => reset_for_rollback().await,
                }
            }
            BootStatus::Stable => {
                if result.storage_healthy == Some(true) {
                    log_status("stable");
                } else {
                    log_status("stable; storage self-check failed");
                }
            }
            BootStatus::Degraded => log_status("confirmed; metadata completion failed"),
        }
    }
    // Scoped adapter has released the one SystemBoot, ready for later OTA use.
    ota::serve(flash, system, &store, peripheral, spawner).await;
}
fn log_status(status: &str) {
    log::info!(target: "example_firmware_ota_system", "boot: {status}");
}
#[esp_hal::main]
fn main() -> ! {
    let p = esp_hal::init(esp_hal::Config::default());
    iobewi_log::install(
        iobewi_esp_console::console_print,
        "example_firmware_ota_system",
    );
    esp_alloc::heap_allocator!(size:160*1024);
    let timg0 = TimerGroup::new(p.TIMG0);
    esp_rtos::start(timg0.timer0, p.FROM_CPU_INTR0);
    // After HAL and TIMG0 resets, before discovery/NVS/OTM1/self-check awaits.
    iobewi_esp_watchdog::arm_ms(WATCHDOG_MS);
    let flash = iobewi_esp_flash::init(p.FLASH);
    static EXECUTOR: StaticCell<esp_rtos::embassy::Executor> = StaticCell::new();
    let executor = EXECUTOR.init(esp_rtos::embassy::Executor::new());
    executor.run(|spawner: Spawner| {
        spawner.spawn(boot_task(flash, p.WIFI, spawner).unwrap());
    })
}
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // If startup/confirmation failed before disarm, hardware expiry still resets.
    loop {
        core::hint::spin_loop();
    }
}
