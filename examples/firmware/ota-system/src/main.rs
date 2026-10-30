#![no_std]
#![no_main]
#![recursion_limit = "256"]
extern crate alloc;
mod ota;
#[cfg(feature = "supervised")]
#[path = "../../ota-supervised/src/supervised.rs"]
mod supervised;
mod wifi;
#[cfg(all(feature = "esp32c3", feature = "esp32s3"))]
compile_error!("choose exactly one MCU feature");
#[cfg(not(any(feature = "esp32c3", feature = "esp32s3")))]
compile_error!("choose esp32c3 or esp32s3");
// The component domain (`/workload/*`, OTM2) is not authenticated by #90: a build that
// claims `signed` while serving it would be misleading, so the combination does not compile.
#[cfg(all(feature = "signed", feature = "supervised"))]
compile_error!(
    "`signed` is not supported with `supervised`: the /workload/* OTM2 routes carry no image authentication"
);
#[cfg(all(feature = "signed-rsa", feature = "supervised"))]
compile_error!(
    "`signed-rsa` is not supported with `supervised`: the /workload/* OTM2 routes carry no image authentication"
);
#[cfg(all(feature = "signed", feature = "signed-rsa"))]
compile_error!(
    "choose one signature algorithm: `signed` (ECDSA P-256) or `signed-rsa` (RSA-PSS 3072 with boot proofs)"
);
#[cfg(all(feature = "reject-self-check", feature = "hang-self-check"))]
compile_error!("choose at most one self-check fault");

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_hal::timer::timg::TimerGroup;
use iobewi_config_space::ConfigManager;
use iobewi_config_space::storage::{EkvBackend, SpaceSpec, Store};
use iobewi_esp_config_space::EspConfigFlash;
use iobewi_esp_firmware_a::{platform::SystemBoot, service::BootRuntime};
use iobewi_esp_firmware_storage::ota_meta::{self, EspOtaMeta};
use iobewi_esp_flash::SharedFlash;
use iobewi_firmware_update::{
    metadata::PendingCheckAction,
    service::boot::{self, BootSelfCheck, BootStatus, Confirmation},
};
use static_cell::StaticCell;

esp_bootloader_esp_idf::esp_app_desc!();
const WATCHDOG_MS: u64 = 20_000;
const SELF_CHECK_MS: u64 = 15_000;

/// Health of what this image needs to run, as the confirmation gate sees it:
/// the OTA metadata store must be readable and the application configuration
/// must be mounted and healthy. An image whose configuration is unusable is
/// therefore never confirmed (it is rejected and rolled back while pending).
struct Check<'a> {
    ota: &'a EspOtaMeta<'a>,
    config: Option<&'a EkvBackend<'static, EspConfigFlash>>,
}
impl BootSelfCheck for Check<'_> {
    async fn self_check(&self) -> bool {
        if cfg!(feature = "hang-self-check") {
            core::future::pending::<()>().await;
        }
        let healthy = self.ota.is_healthy().await
            && match self.config {
                Some(config) => config.self_check().await,
                None => false,
            };
        healthy && !cfg!(feature = "reject-self-check")
    }
}
async fn reset_for_rollback() -> ! {
    #[cfg(feature = "supervised")]
    supervised::clean_reboot();
    // Same settling delay as the historical application binding. Watchdog stays
    // armed even if rejection or this delay cannot complete.
    Timer::after(Duration::from_millis(200)).await;
    esp_hal::system::software_reset()
}
/// Mounts the application configuration (Wi-Fi). `None` on any failure: the
/// reason is logged and no region is reformatted except a blank one.
async fn mount_config(flash: &'static SharedFlash) -> Option<&'static Store<EspConfigFlash>> {
    let config_flash = EspConfigFlash::from_label(flash, "config")
        .await
        .map_err(|error| log::error!("config: partition unavailable {error:?}"))
        .ok()?;
    let blank = config_flash
        .is_blank()
        .await
        .map_err(|error| log::error!("config: read failed {error:?}"))
        .ok()?;
    static CONFIG: StaticCell<Store<EspConfigFlash>> = StaticCell::new();
    const SPACES: &[SpaceSpec] = &[SpaceSpec::new(
        "wifi",
        iobewi_wifi_manager::CONFIG_BUDGET.max_bytes(),
    )];
    let config = CONFIG.init(
        Store::new(config_flash, esp_hal::rng::Rng::new().random(), SPACES)
            .expect("config admission failed"),
    );
    if blank && let Err(error) = config.format().await {
        log::error!("config: initialization failed {error:?}");
        return None;
    }
    if let Err(error) = config.mount().await {
        log::error!(target: "example_firmware_ota_system", "config: mount failed {error:?}; explicit reset required");
        return None;
    }
    Some(config)
}
#[embassy_executor::task]
async fn boot_task(
    flash: &'static SharedFlash,
    peripheral: esp_hal::peripherals::WIFI<'static>,
    spawner: Spawner,
    #[cfg(feature = "supervised")] cpu: esp_hal::system::CpuControl<'static>,
) {
    // The OTA metadata has its own partition and store: it neither depends on
    // nor is affected by the application configuration mounted below.
    let ota_region = ota_meta::discover(flash)
        .await
        .expect("ota_meta partition unavailable");
    let store = EspOtaMeta::new(flash, ota_region);
    // A configuration that cannot be mounted must not hide the boot state:
    // the boot decision below still runs and can reject the image.
    let config = mount_config(flash).await;
    let backend = config.map(Store::backend);
    let wifi = backend.and_then(|backend| {
        ConfigManager::new(backend)
            .claim("wifi", iobewi_wifi_manager::CONFIG_BUDGET)
            .map_err(|error| log::error!("config: Wi-Fi claim failed {error:?}"))
            .ok()
    });
    let check = Check {
        ota: &store,
        config: backend.as_ref(),
    };
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
    #[cfg(feature = "supervised")]
    let component = match supervised::Component::new(flash, cpu).await {
        Ok(mut component) => {
            if component.recover().await.is_err() {
                log::error!("B: recovery failed; system A remains available");
            }
            static COMPONENT: StaticCell<supervised::SharedComponent> = StaticCell::new();
            let shared = COMPONENT.init(embassy_sync::mutex::Mutex::new(component));
            spawner.spawn(supervised::monitor(shared).unwrap());
            Some(&*shared)
        }
        Err(()) => {
            log::error!("B: unavailable; system A remains available");
            None
        }
    };
    let Some(wifi) = wifi else {
        log::error!("config: unavailable; Wi-Fi and OTA service cannot start");
        core::future::pending::<()>().await;
        unreachable!()
    };
    ota::serve(
        flash,
        system,
        &store,
        wifi,
        peripheral,
        spawner,
        #[cfg(feature = "supervised")]
        component,
    )
    .await;
}
fn log_status(status: &str) {
    log::info!(target: "example_firmware_ota_system", "boot: {status}");
}
#[esp_hal::main]
fn main() -> ! {
    // The partition table is where this build was configured to find it
    // (ESP_BOOTLOADER_ESP_IDF_CONFIG_PARTITION_TABLE_OFFSET); stamp the image so the packaging
    // tools can refuse it next to a second stage or a table built for another geometry.
    core::hint::black_box(&iobewi_esp_boot::geometry::STAMP);
    // The platform's primary console is shared with the bootloader: keep its stamp in the image so
    // the packaging tools can refuse a second stage built for another console.
    core::hint::black_box(&iobewi_esp_console::STAMP);
    let p = esp_hal::init(esp_hal::Config::default());
    #[cfg(all(feature = "console-ring", feature = "esp32s3"))]
    let console_tx = p.GPIO43;
    #[cfg(all(feature = "console-ring", feature = "esp32c3"))]
    let console_tx = p.GPIO21;
    #[cfg(feature = "console-ring")]
    iobewi_esp_console::install(
        p.UART0,
        console_tx,
        esp_hal::uart::Config::default().with_baudrate(115_200),
    )
    .expect("console UART configuration");
    iobewi_log::install(
        iobewi_esp_console::console_print,
        "example_firmware_ota_system",
    );
    // Untargeted records carry the binary's module path, not the package name;
    // the station IP is logged by the Wi-Fi crate. Both are needed to operate OTA.
    let mut policy = iobewi_log::default_policy("example_firmware_ota_system").unwrap();
    for target in [
        "firmware_ota_system_esp32",
        "firmware_ota_supervised_esp32",
        "iobewi_esp_wifi",
        "iobewi_wifi_manager",
    ] {
        policy
            .add_target(target, iobewi_log::LevelFilter::Info)
            .unwrap();
    }
    iobewi_log::apply_policy(policy);
    esp_alloc::heap_allocator!(size:160*1024);
    let timg0 = TimerGroup::new(p.TIMG0);
    esp_rtos::start(timg0.timer0, p.FROM_CPU_INTR0);
    // After HAL and TIMG0 resets, before discovery/EKV/OTM1/self-check awaits.
    iobewi_esp_watchdog::arm_ms(WATCHDOG_MS);
    let flash = iobewi_esp_flash::init(p.FLASH);
    static EXECUTOR: StaticCell<esp_rtos::embassy::Executor> = StaticCell::new();
    let executor = EXECUTOR.init(esp_rtos::embassy::Executor::new());
    executor.run(|spawner: Spawner| {
        spawner.spawn(
            boot_task(
                flash,
                p.WIFI,
                spawner,
                #[cfg(feature = "supervised")]
                esp_hal::system::CpuControl::new(p.CPU_CTRL),
            )
            .unwrap(),
        );
    })
}
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    // Independent of the logger: works whatever state it is in.
    iobewi_esp_console::panic_print(info);
    // If startup/confirmation failed before disarm, hardware expiry still resets.
    loop {
        core::hint::spin_loop();
    }
}
