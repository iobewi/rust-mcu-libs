//! Platform-independent STA, AP and AP+STA scenarios.
use alloc::string::String;
use iobewi_wifi_core::{AccessPointConfig, WifiAccessPoint, WifiTransport};

#[derive(Clone, Copy)]
pub enum Mode {
    Sta,
    Ap,
    ApSta,
}

pub async fn run<T>(wifi: &mut T, mode: Mode)
where
    T: WifiTransport + WifiAccessPoint,
{
    if matches!(mode, Mode::Ap | Mode::ApSta) {
        let password =
            option_env!("IOBEWI_AP_PASSWORD").expect("Set IOBEWI_AP_PASSWORD at build time");
        let config = AccessPointConfig::new("IOBEWI-Setup", password, 6)
            .expect("Invalid WPA2 AP credentials");
        assert!(wifi.start_access_point(&config).await, "AP startup failed");
        assert!(wifi.is_access_point_active());
        assert!(wifi.access_point_handle().is_some());
        // DHCP is already provided by the platform's Wi-Fi implementation.
    }

    if matches!(mode, Mode::Sta | Mode::ApSta) {
        let ssid = option_env!("IOBEWI_STA_SSID").expect("Set IOBEWI_STA_SSID at build time");
        let password =
            option_env!("IOBEWI_STA_PASSWORD").expect("Set IOBEWI_STA_PASSWORD at build time");
        #[cfg(not(feature = "association-diagnostic"))]
        assert!(
            wifi.connect(ssid, String::from(password)).await,
            "STA join/DHCP failed"
        );
        #[cfg(feature = "association-diagnostic")]
        {
            let mut connected = false;
            for attempt in 1..=2 {
                let start = embassy_time::Instant::now();
                log::info!(target: "example_wifi_modes", "STA attempt {attempt} start");
                connected = wifi.connect(ssid, String::from(password)).await;
                log::info!(
                    target: "example_wifi_modes",
                    "STA attempt {attempt} result={connected} elapsed_ms={}",
                    start.elapsed().as_millis()
                );
                if connected {
                    break;
                }
                if attempt == 1 {
                    embassy_time::Timer::after(embassy_time::Duration::from_secs(1)).await;
                }
            }
            if !connected {
                log::warn!(target: "example_wifi_modes", "STA diagnostic exhausted");
                core::future::pending::<()>().await;
            }
        }
        assert!(wifi.is_online());
        assert!(wifi.ip().is_some());
        if matches!(mode, Mode::ApSta) {
            assert!(wifi.is_access_point_active(), "AP lost after STA join");
        }
    }

    core::future::pending::<()>().await;
}
