//! Wi-Fi composition shared by the A and A+B reference firmwares.
use alloc::string::String;
use embassy_net::Stack;
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, signal::Signal};
use embassy_time::{Duration, Timer};
use iobewi_config_space::{ConfigSpace, storage::EkvBackend};
use iobewi_esp_config_space::EspConfigFlash;
use iobewi_wifi_manager::{LinkObserver, Sleep, WifiManager, backoff_ms, is_provisioned};

struct Clock;
impl Sleep for Clock {
    async fn sleep_ms(&self, ms: u32) {
        Timer::after(Duration::from_millis(ms.into())).await;
    }
}
pub type Ready = Signal<NoopRawMutex, Stack<'static>>;

struct Observer(&'static Ready);
impl LinkObserver<Stack<'static>> for Observer {
    fn ready(&mut self, network: Stack<'static>) {
        log::info!("Wi-Fi ready; HTTPS can use the station stack");
        self.0.signal(network);
    }
    fn link_down(&mut self) {
        log::warn!("Wi-Fi link lost; manager is reconnecting without reboot");
    }
}

#[embassy_executor::task]
pub async fn maintain(
    transport: iobewi_esp_wifi::WifiManager<3>,
    config: ConfigSpace<EkvBackend<'static, EspConfigFlash>>,
    ready: &'static Ready,
) -> ! {
    let provisioned = is_provisioned(&config).await;
    let mut manager = WifiManager::new(transport, config);
    if !provisioned {
        // Build credentials only bootstrap an unprovisioned reference firmware.
        // provision() stores WFC1 only after association, DHCP and commit succeed.
        let ssid = option_env!("IOBEWI_STA_SSID").expect("IOBEWI_STA_SSID required");
        let password = option_env!("IOBEWI_STA_PASSWORD").expect("IOBEWI_STA_PASSWORD required");
        assert!(!ssid.is_empty(), "IOBEWI_STA_SSID must not be empty");
        let mut failed = 0u32;
        while !manager.provision(ssid, String::from(password)).await {
            failed = failed.saturating_add(1);
            let wait = backoff_ms(failed);
            log::warn!("Wi-Fi bootstrap failed; retry in {wait} ms without reboot");
            Clock.sleep_ms(wait).await;
        }
    }
    // Own the transport for the full server lifetime. No concurrent driver access,
    // one-shot join assertion, or second application reconnection loop.
    let result = manager.maintain(&Clock, &mut Observer(ready)).await;
    panic!("Wi-Fi credentials became unavailable: {result:?}");
}
