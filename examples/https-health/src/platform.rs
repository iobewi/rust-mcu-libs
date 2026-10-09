use alloc::string::String;
use embassy_executor::Spawner;
use embassy_net::StackResources;
use esp_hal::timer::timg::TimerGroup;
use iobewi_esp_tls::EspTlsListener;
use iobewi_esp_wifi::WifiManager;
use iobewi_wifi_core::WifiTransport;
use static_cell::StaticCell;

const SOCKETS: usize = 3;
const CERT: &str = match option_env!("IOBEWI_TLS_CERT_PEM") { Some(v) => v, None => "" };
const KEY: &str = match option_env!("IOBEWI_TLS_KEY_PEM") { Some(v) => v, None => "" };

fn now_unix() -> Option<u64> {
    None // No wall-clock source required for this server-only smoke test.
}

async fn load_identity() -> Option<iobewi_esp_tls::mbedtls_rs::SessionConfig<'static>> {
    iobewi_crypto_mbedtls::server_config_from_pem(CERT, KEY).ok()
}

#[embassy_executor::task]
async fn https_task(peripheral: esp_hal::peripherals::WIFI<'static>, spawner: Spawner) {
    static RESOURCES: StaticCell<StackResources<SOCKETS>> = StaticCell::new();
    let mut wifi =
        WifiManager::<SOCKETS>::new(peripheral, spawner, RESOURCES.init(StackResources::new()));
    let ssid = option_env!("IOBEWI_STA_SSID").expect("Set IOBEWI_STA_SSID for device use");
    let password = option_env!("IOBEWI_STA_PASSWORD").expect("Set IOBEWI_STA_PASSWORD for device use");
    assert!(
        wifi.connect(ssid, String::from(password)).await,
        "STA join failed"
    );
    let stack = wifi
        .network_handle()
        .expect("STA network stack unavailable");
    log::info!("HTTPS example: Wi-Fi ready; HTTPS :443 only");

    let tls = iobewi_esp_tls::init(now_unix);
    let mut rx = [0u8; 4096];
    let mut tx = [0u8; 4096];
    let mut listener = EspTlsListener::new(stack, tls, load_identity, &mut rx, &mut tx);

    let router = iobewi_http_server::HttpRouter::new().route(
        "/health",
        iobewi_http_server::routing::get(|| async { "ok" }),
    );
    iobewi_http_server::serve_forever_tls(&mut listener, &router).await;
}

pub fn boot() -> ! {
    let peripherals = esp_hal::init(esp_hal::Config::default());
    iobewi_log::install(iobewi_esp_console::console_print, "example_https_health");
    esp_alloc::heap_allocator!(size: 160 * 1024);
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);

    static EXECUTOR: StaticCell<esp_rtos::embassy::Executor> = StaticCell::new();
    let executor = EXECUTOR.init(esp_rtos::embassy::Executor::new());
    executor.run(|spawner: Spawner| {
        spawner.spawn(https_task(peripherals.WIFI, spawner).unwrap());
    })
}
