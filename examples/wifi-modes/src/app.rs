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
        assert!(
            wifi.connect(ssid, String::from(password)).await,
            "STA join/DHCP failed"
        );
        assert!(wifi.is_online());
        assert!(wifi.ip().is_some());
        if matches!(mode, Mode::ApSta) {
            assert!(wifi.is_access_point_active(), "AP lost after STA join");
        }
    }

    core::future::pending::<()>().await;
}
