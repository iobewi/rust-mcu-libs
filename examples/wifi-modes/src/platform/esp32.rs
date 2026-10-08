//! ESP32-specific HAL, Embassy runtime and radio resource composition.
use embassy_executor::Spawner;
use embassy_net::StackResources;
use esp_hal::{interrupt::software::SoftwareInterruptControl, timer::timg::TimerGroup};
use iobewi_esp_wifi::WifiManager;
use static_cell::StaticCell;

const STA_SOCKETS: usize = 2;
const AP_SOCKETS: usize = 2; // One for DHCP, one for a future product service.

#[embassy_executor::task]
async fn wifi_task(
    peripheral: esp_hal::peripherals::WIFI<'static>,
    spawner: Spawner,
    mode: crate::app::Mode,
) {
    static STA_RESOURCES: StaticCell<StackResources<STA_SOCKETS>> = StaticCell::new();
    static AP_RESOURCES: StaticCell<StackResources<AP_SOCKETS>> = StaticCell::new();

    let mut wifi = WifiManager::<STA_SOCKETS, AP_SOCKETS>::new(
        peripheral,
        spawner,
        STA_RESOURCES.init(StackResources::new()),
    ).with_access_point(AP_RESOURCES.init(StackResources::new()));

    crate::app::run(&mut wifi, mode).await;
}

pub fn boot(mode: crate::app::Mode) -> ! {
    let peripherals = esp_hal::init(esp_hal::Config::default());
    esp_alloc::heap_allocator!(size: 96 * 1024);

    let sw_int = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);

    static EXECUTOR: StaticCell<esp_rtos::embassy::Executor> = StaticCell::new();
    let executor = EXECUTOR.init(esp_rtos::embassy::Executor::new());
    executor.run(|spawner: Spawner| {
        spawner.spawn(wifi_task(peripherals.WIFI, spawner, mode).unwrap());
    })
}
