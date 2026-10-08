#![no_std]

//! Reusable ESP Wi-Fi transport: station, soft access point, or both.
//!
//! Owns only Wi-Fi/network mechanics:
//!
//! - lazy radio initialization;
//! - station scans with strongest-SSID/BSSID selection;
//! - association;
//! - DHCP;
//! - Embassy network runner;
//! - reporting the resulting IP-capable stack;
//! - the optional soft access point: its own network stack and a small DHCP
//!   server for its clients (see [`WifiManager::with_access_point`]).
//!
//! It deliberately does not own credential persistence, provisioning
//! protocols, TLS, HTTP, heartbeat/log services or application supervision.

extern crate alloc;

mod connection;
use connection::Connection;

use alloc::string::String;
use alloc::vec::Vec;

use core::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use edge_dhcp::server::{Server as DhcpServer, ServerOptions as DhcpOptions};
use edge_nal::UdpBind;
use edge_nal_embassy::{Udp, UdpBuffers};
use embassy_executor::Spawner;
use embassy_futures::select::{Either, select};
use embassy_net::{Ipv4Address, Ipv4Cidr, Runner, Stack, StackResources, StaticConfigV4};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embassy_time::{Duration, Instant, Timer, with_timeout};
use esp_hal::peripherals::WIFI;
use esp_radio::wifi::{
    AuthenticationMethod, AuthenticationMethodConfig, Config, ConnectionError, ControllerConfig,
    DisconnectReason, Interface, WifiController,
    ap::AccessPointConfig as EspAccessPointConfig,
    scan::{ScanConfig, ScanTypeConfig},
    sta::{ScanMethod, StationConfig},
};
use iobewi_wifi_core::{AccessPointConfig, WifiAccessPoint, WifiTransport};
use log::{info, warn};
use static_cell::StaticCell;

pub use iobewi_wifi_core::Network;

/// The access point's own address and network (`/24`). Clients receive
/// addresses from [`DHCP_FIRST`] up; there is no gateway and no DNS.
pub const ACCESS_POINT_ADDRESS: [u8; 4] = [172, 23, 241, 1];
const DHCP_FIRST: u8 = 2;
/// Clients served at once (radio association limit and DHCP lease table).
const DHCP_LEASES: usize = 4;
const DHCP_LEASE_SECS: u32 = 3600;
const DHCP_BUFFER_LEN: usize = 600;
/// Upper bound for the access point's radio and network to become ready.
const ACCESS_POINT_READY_TIMEOUT: Duration = Duration::from_secs(5);
const DHCP_STOP_TIMEOUT: Duration = Duration::from_secs(1);

type DhcpBuffers = UdpBuffers<1, DHCP_BUFFER_LEN, DHCP_BUFFER_LEN, 4>;

/// `true` while the DHCP service must answer, `false` to end it.
static DHCP_ENABLED: Signal<CriticalSectionRawMutex, bool> = Signal::new();
/// Raised by the DHCP task once it has closed its socket after a `false`.
static DHCP_STOPPED: Signal<CriticalSectionRawMutex, ()> = Signal::new();
static DHCP_BUFFERS: StaticCell<DhcpBuffers> = StaticCell::new();

struct Radio {
    controller: WifiController<'static>,
    stack: Stack<'static>,
    /// Last station configuration handed to the radio; reused whenever the
    /// mode changes so the station part is never lost.
    station: StationConfig,
}

/// Wi-Fi transport: station, plus an optional soft access point.
///
/// The caller supplies the Embassy socket resources because socket-set sizing
/// is application policy. The manager consumes them only when the radio is
/// first initialized. Access point support is opt-in through
/// [`WifiManager::with_access_point`], which supplies a second, independent
/// resource set (`AP_SOCKETS`); without it the type is a plain station.
///
/// One object owns the one radio. Switching between station-only and
/// station + access point is a mode change that restarts the radio (esp-radio
/// applies it that way), so the station link drops and must be re-established;
/// the portable manager's reconnection loop does that.
pub struct WifiManager<const SOCKETS: usize, const AP_SOCKETS: usize = 0> {
    peripheral: Option<WIFI<'static>>,
    spawner: Spawner,
    resources: Option<&'static mut StackResources<SOCKETS>>,
    radio: Option<Radio>,
    /// Per SSID, the strongest access point of the last scan: (ssid, bssid, channel).
    strongest_bssid: Vec<(String, [u8; 6], u8)>,
    connection: Connection,
    ap_resources: Option<&'static mut StackResources<AP_SOCKETS>>,
    ap_stack: Option<Stack<'static>>,
    /// `Some` exactly while the access point is active.
    ap_config: Option<AccessPointConfig>,
}

/// Strongest access point of one SSID: (ssid, bssid, signal dBm, secured, channel).
type RankedAccessPoint = (String, [u8; 6], i8, bool, u8);

/// How one connection attempt ended.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Attempt {
    Connected,
    /// The access point answered but refused the credentials (wrong passphrase): the access
    /// point is fine, so its pin must be kept.
    CredentialsRefused,
    /// Anything else: not found, no answer, DHCP: the pinned access point may be gone.
    Failed,
}

fn credentials_refused(reason: &DisconnectReason) -> bool {
    matches!(
        reason,
        DisconnectReason::FourWayHandshakeTimeout
            | DisconnectReason::HandshakeTimeout
            | DisconnectReason::AuthenticationFailed
            | DisconnectReason::MicFailure
            | DisconnectReason::_802_1xAuthenticationFailed
    )
}

/// Upper bounds for one connection attempt. Without them a rejected or
/// unanswered association (or a DHCP that never completes) would block
/// `connect` forever and the manager could never retry. These are mechanics
/// of "one attempt in, one result out"; whether and when to retry is the
/// manager's policy.
/// Several access points often share one SSID (mesh, repeaters). The default esp-radio scan
/// dwells only 10 to 20 ms per channel, so with many answering the same probe the strongest one
/// is easily missed: dwell longer, scan twice and keep the best per SSID.
const SCAN_DWELL_MIN_MS: u64 = 40;
const SCAN_DWELL_MAX_MS: u64 = 120;
const SCAN_PASSES: usize = 2;
const SCAN_MAX_RECORDS: usize = 40;
const ASSOCIATE_TIMEOUT: Duration = Duration::from_secs(20);
const DHCP_TIMEOUT: Duration = Duration::from_secs(20);

impl<const SOCKETS: usize, const AP_SOCKETS: usize> WifiManager<SOCKETS, AP_SOCKETS> {
    /// `AP_SOCKETS` defaults to 0 in the type, so `WifiManager<SOCKETS>` is a plain station until
    /// [`WifiManager::with_access_point`] is called.
    pub fn new(
        peripheral: WIFI<'static>,
        spawner: Spawner,
        resources: &'static mut StackResources<SOCKETS>,
    ) -> Self {
        Self {
            peripheral: Some(peripheral),
            spawner,
            resources: Some(resources),
            radio: None,
            strongest_bssid: Vec::new(),
            connection: Connection::default(),
            ap_resources: None,
            ap_stack: None,
            ap_config: None,
        }
    }

    /// Enables the soft access point by giving it its own socket resources.
    ///
    /// `AP_SOCKETS` must cover everything that will run on the access point's
    /// network: one socket for the built-in DHCP server plus whatever the
    /// product serves there (for example one TCP listener). The access point
    /// stack and the DHCP service are created lazily at the first
    /// [`WifiAccessPoint::start_access_point`]. Without this call (or with
    /// `AP_SOCKETS` 0) `start_access_point` reports failure.
    pub fn with_access_point(mut self, resources: &'static mut StackResources<AP_SOCKETS>) -> Self {
        if AP_SOCKETS < 2 {
            // The DHCP server takes one socket; a smaller set could only panic later.
            warn!("Wi-Fi: access point resources need at least 2 sockets; access point disabled");
            return self;
        }
        self.ap_resources = Some(resources);
        self
    }

    /// The device's current IPv4 address, if online.
    pub fn ip(&self) -> Option<embassy_net::Ipv4Address> {
        Some(self.radio.as_ref()?.stack.config_v4()?.address.address())
    }

    /// Returns the IP-capable network stack handle once DHCP has configured
    /// it. Opaque to every portable caller above `iobewi-wifi-core`'s own
    /// `WifiTransport`/`WifiProvisioning` ports -- only this crate and
    /// whatever the application composition root does with it know it's an
    /// `embassy_net::Stack`.
    pub fn network_handle(&self) -> Option<Stack<'static>> {
        let radio = self.radio.as_ref()?;
        radio.stack.config_v4()?;
        Some(radio.stack)
    }

    pub fn is_online(&self) -> bool {
        self.network_handle().is_some()
    }

    fn radio(&mut self) -> Option<&mut Radio> {
        if self.radio.is_none() {
            let mut controller =
                match WifiController::new(self.peripheral.take()?, ControllerConfig::default()) {
                    Ok(controller) => controller,
                    Err(e) => {
                        warn!("Wi-Fi init failed: {e:?}");
                        return None;
                    }
                };

            if let Err(e) = controller.set_config(&Config::Station(StationConfig::default())) {
                warn!("Wi-Fi start failed: {e:?}");
                return None;
            }

            let resources = self.resources.take()?;
            let seed = esp_hal::time::Instant::now()
                .duration_since_epoch()
                .as_micros();
            let (stack, runner) = embassy_net::new(
                Interface::station(),
                embassy_net::Config::dhcpv4(Default::default()),
                resources,
                seed,
            );
            self.spawner.spawn(net_task(runner).unwrap());
            self.radio = Some(Radio {
                controller,
                stack,
                station: StationConfig::default(),
            });
        }

        self.radio.as_mut()
    }

    /// Scans for networks, one entry per SSID, keeping the strongest BSSID.
    pub async fn scan(&mut self) -> Vec<Network> {
        let strongest = self.scan_strongest(None).await;
        if strongest.is_empty() {
            return Vec::new();
        }
        self.strongest_bssid = strongest
            .iter()
            .map(|(ssid, bssid, _, _, channel)| (ssid.clone(), *bssid, *channel))
            .collect();

        strongest
            .into_iter()
            .map(|(ssid, _, signal_strength, secured, _)| Network {
                ssid,
                signal_strength,
                secured,
            })
            .collect()
    }

    /// Scans and returns the strongest access point per SSID: (ssid, bssid, signal, secured,
    /// channel). With `only`, the scan is directed at that SSID (directed probe requests, so only
    /// its access points answer and fewer responses collide).
    async fn scan_strongest(&mut self, only: Option<&str>) -> Vec<RankedAccessPoint> {
        let Some(radio) = self.radio() else {
            return Vec::new();
        };

        let mut scan_config = ScanConfig::default()
            .with_scan_type(ScanTypeConfig::Active {
                min: esp_hal::time::Duration::from_millis(SCAN_DWELL_MIN_MS),
                max: esp_hal::time::Duration::from_millis(SCAN_DWELL_MAX_MS),
            })
            .with_max(SCAN_MAX_RECORDS);
        if let Some(ssid) = only {
            if let Ok(ssid) = ssid.try_into() {
                scan_config = scan_config.with_ssid(ssid);
            }
        }
        let mut access_points = Vec::new();
        for pass in 0..SCAN_PASSES {
            match radio.controller.scan_async(&scan_config).await {
                Ok(found) => access_points.extend(found),
                Err(e) => warn!("Wi-Fi scan pass {pass} failed: {e:?}"),
            }
        }

        let mut strongest: Vec<RankedAccessPoint> = Vec::new();
        for ap in &access_points {
            let ssid = ap.ssid.as_str();
            if ssid.is_empty() {
                continue;
            }

            let secured = !matches!(ap.auth_method, None | Some(AuthenticationMethod::None));
            let entry = (
                String::from(ssid),
                ap.bssid,
                ap.signal_strength,
                secured,
                ap.channel,
            );
            match strongest
                .iter_mut()
                .find(|(known_ssid, ..)| known_ssid == ssid)
            {
                Some((_, _, signal_strength, _, _)) if *signal_strength >= ap.signal_strength => {}
                Some(known) => *known = entry,
                None => strongest.push(entry),
            }
        }
        info!(
            "Wi-Fi: scan saw {} access points on {} networks",
            access_points.len(),
            strongest.len()
        );
        strongest
    }

    /// Connects and waits for DHCP. Pins the BSSID and channel of the strongest access point seen
    /// for this SSID in the last scan, if any. If that access point does not work (it left, or
    /// refuses us) the pin is dropped, so the next attempt lets the radio scan every channel and
    /// pick the strongest itself instead of retrying a dead access point forever.
    pub async fn connect(&mut self, ssid: &str, password: String) -> bool {
        let pinned = self.strongest_bssid.iter().any(|(known, ..)| known == ssid);
        match self.connect_once(ssid, password).await {
            Attempt::Connected => true,
            Attempt::CredentialsRefused => {
                warn!("Wi-Fi: {ssid} refused the credentials (check the passphrase)");
                false
            }
            Attempt::Failed => {
                if pinned {
                    self.strongest_bssid.retain(|(known, ..)| known != ssid);
                    warn!(
                        "Wi-Fi: the pinned access point failed; the next attempt scans all channels"
                    );
                }
                false
            }
        }
    }

    async fn connect_once(&mut self, ssid: &str, password: String) -> Attempt {
        // Test current controller state as well as DHCP: a retained lease alone
        // does not prove that an association survived a link loss.
        if let Some(radio) = self.radio.as_ref() {
            if self.connection.can_reuse(
                radio.controller.is_connected(),
                radio.stack.is_link_up(),
                radio.stack.is_config_up(),
                ssid,
                &password,
            ) {
                return Attempt::Connected;
            }
        }
        // Before the first await: failures or cancellation cannot retain proof
        // of a previous successful connection.
        self.connection.invalidate();

        let mut pin = self
            .strongest_bssid
            .iter()
            .find(|(known_ssid, ..)| known_ssid == ssid)
            .map(|(_, bssid, channel)| (*bssid, *channel));

        if pin.is_none() {
            // Several access points often share one SSID. Left alone, the radio's own pick among
            // them was seen choosing a -67 dBm access point over a -44 dBm one (first cold boot
            // with saved credentials), so rank them ourselves with a directed scan.
            info!("Wi-Fi: no scan result for {ssid}, scanning for it before joining");
            let found = self.scan_strongest(Some(ssid)).await;
            if let Some((_, bssid, _, _, channel)) =
                found.into_iter().find(|(known, ..)| known == ssid)
            {
                self.strongest_bssid.retain(|(known, ..)| known != ssid);
                self.strongest_bssid
                    .push((String::from(ssid), bssid, channel));
                pin = Some((bssid, channel));
            } else {
                info!("Wi-Fi: {ssid} not seen; the radio will scan all channels");
            }
        }

        // While the access point is active the station configuration is applied
        // as station + access point: a station-only config would be a mode change
        // and would restart the radio under the access point's clients.
        let access_point = self.ap_config.as_ref().and_then(esp_access_point);

        let Some(radio) = self.radio() else {
            return Attempt::Failed;
        };

        // Reprovisioning may happen while the station is already associated
        // (for example through Improv Serial). esp-radio does not treat
        // set_config()+connect_async() as a roam/reconfigure operation on an
        // already-connected station: explicitly tear the old association down
        // first, and wait until embassy-net has dropped the old DHCP config so
        // wait_config_up() below cannot return immediately with a stale lease.
        if radio.controller.is_connected() {
            info!("Wi-Fi: disconnecting current association before reconfiguration");
            if let Err(e) = radio.controller.disconnect_async().await {
                warn!("Wi-Fi: disconnect before reconfiguration failed: {e:?}");
                return Attempt::Failed;
            }
        }
        if radio.stack.is_config_up() {
            radio.stack.wait_config_down().await;
        }

        let Ok(ssid_cfg) = ssid.try_into() else {
            warn!("Wi-Fi: invalid SSID {ssid}");
            return Attempt::Failed;
        };
        let authentication = if password.is_empty() {
            AuthenticationMethodConfig::Open
        } else {
            match password.as_str().try_into() {
                Ok(password) => AuthenticationMethodConfig::Wpa2Personal(password),
                Err(_) => {
                    warn!("Wi-Fi: invalid password for {ssid}");
                    return Attempt::Failed;
                }
            }
        };
        // `Fast` (the default) joins the FIRST access point found with this SSID and ignores the
        // signal sort; scanning every channel lets the radio choose the strongest.
        let mut config = StationConfig::default()
            .with_ssid(ssid_cfg)
            .with_authentication(authentication)
            .with_scan_method(ScanMethod::AllChannels)
            // The portable manager already retries with backoff; an internal retry would only
            // double the time the access point is disturbed by a failing join.
            .with_failure_retry_cnt(0);
        if let Some((bssid, channel)) = pin {
            config = config.with_bssid(bssid).with_channel(channel);
        }

        let mode = match access_point {
            Some(access_point) => Config::AccessPointStation(config.clone(), access_point),
            None => Config::Station(config.clone()),
        };
        if radio.controller.set_config(&mode).is_err() {
            warn!("Wi-Fi: connection to {ssid} failed");
            return Attempt::Failed;
        }
        radio.station = config;
        match with_timeout(ASSOCIATE_TIMEOUT, radio.controller.connect_async()).await {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => {
                // The reason names why the association ended; it carries no credential.
                match &error {
                    ConnectionError::Failed(info) => {
                        warn!("Wi-Fi: connection to {ssid} failed: {:?}", info.reason);
                        if credentials_refused(&info.reason) {
                            return Attempt::CredentialsRefused;
                        }
                    }
                    other => warn!("Wi-Fi: connection to {ssid} failed: {other:?}"),
                }
                return Attempt::Failed;
            }
            Err(_) => {
                warn!("Wi-Fi: association with {ssid} timed out");
                return Attempt::Failed;
            }
        }

        if with_timeout(DHCP_TIMEOUT, radio.stack.wait_config_up())
            .await
            .is_err()
        {
            warn!("Wi-Fi: DHCP on {ssid} timed out");
            return Attempt::Failed;
        }
        info!("Wi-Fi connected, ip = {:?}", radio.stack.config_v4());
        // Which access point we really joined (never its address): channel, signal and whether it
        // is the one the scan ranked first.
        if let Ok(joined) = radio.controller.ap_info() {
            let how = match pin {
                Some((bssid, _)) if bssid == joined.bssid => "the pinned strongest access point",
                Some(_) => "NOT the pinned access point",
                None => "chosen by the radio among all channels",
            };
            info!(
                "Wi-Fi: associated on channel {} at {} dBm ({how})",
                joined.channel, joined.signal_strength
            );
        }
        self.connection.established(ssid, password);
        Attempt::Connected
    }
}

impl<const SOCKETS: usize, const AP_SOCKETS: usize> WifiTransport
    for WifiManager<SOCKETS, AP_SOCKETS>
{
    type Address = embassy_net::Ipv4Address;
    type NetworkHandle = Stack<'static>;

    async fn connect(&mut self, ssid: &str, password: String) -> bool {
        WifiManager::connect(self, ssid, password).await
    }

    async fn scan(&mut self) -> Vec<Network> {
        WifiManager::scan(self).await
    }

    async fn wait_down(&mut self) {
        if let Some(radio) = self.radio.as_ref() {
            radio.stack.wait_config_down().await;
        }
    }

    fn ip(&self) -> Option<Self::Address> {
        WifiManager::ip(self)
    }

    fn network_handle(&self) -> Option<Self::NetworkHandle> {
        WifiManager::network_handle(self)
    }

    fn is_online(&self) -> bool {
        WifiManager::is_online(self)
    }
}

fn esp_access_point(config: &AccessPointConfig) -> Option<EspAccessPointConfig> {
    let ssid = config.ssid().try_into().ok()?;
    let password = config.password().try_into().ok()?;
    Some(
        EspAccessPointConfig::default()
            .with_ssid(ssid)
            .with_authentication(AuthenticationMethodConfig::Wpa2Personal(password))
            .with_channel(config.channel())
            .with_max_connections(DHCP_LEASES as u16),
    )
}

impl<const SOCKETS: usize, const AP_SOCKETS: usize> WifiManager<SOCKETS, AP_SOCKETS> {
    /// The access point's network stack, created on first use together with its
    /// runner and DHCP task. Needs the radio (the interfaces belong to it) and
    /// the resources supplied through [`WifiManager::with_access_point`].
    fn access_point_stack(&mut self) -> Option<Stack<'static>> {
        if let Some(stack) = self.ap_stack {
            return Some(stack);
        }
        self.radio.as_ref()?;
        let resources = self.ap_resources.take()?;
        let seed = esp_hal::time::Instant::now()
            .duration_since_epoch()
            .as_micros();
        let [a, b, c, d] = ACCESS_POINT_ADDRESS;
        let (stack, runner) = embassy_net::new(
            Interface::access_point(),
            embassy_net::Config::ipv4_static(StaticConfigV4 {
                address: Ipv4Cidr::new(Ipv4Address::new(a, b, c, d), 24),
                gateway: None,
                dns_servers: Default::default(),
            }),
            resources,
            seed ^ 0x4150,
        );
        self.spawner.spawn(net_task(runner).unwrap());
        let buffers: &'static DhcpBuffers = DHCP_BUFFERS.init(DhcpBuffers::new());
        self.spawner.spawn(dhcp_task(stack, buffers).unwrap());
        self.ap_stack = Some(stack);
        Some(stack)
    }
}

impl<const SOCKETS: usize, const AP_SOCKETS: usize> WifiAccessPoint
    for WifiManager<SOCKETS, AP_SOCKETS>
{
    type NetworkHandle = Stack<'static>;

    async fn start_access_point(&mut self, config: &AccessPointConfig) -> bool {
        if self.ap_config.as_ref() == Some(config) {
            return true;
        }
        let Some(access_point) = esp_access_point(config) else {
            warn!("Wi-Fi: access point settings refused by the radio library");
            return false;
        };
        if self.radio().is_none() {
            return false;
        }
        let Some(stack) = self.access_point_stack() else {
            warn!("Wi-Fi: no access point resources (see with_access_point)");
            return false;
        };
        // A mode change restarts the radio: no earlier association can be trusted.
        self.connection.invalidate();
        let Some(radio) = self.radio.as_mut() else {
            return false;
        };
        let mode = Config::AccessPointStation(radio.station.clone(), access_point);
        if radio.controller.set_config(&mode).is_err() {
            warn!("Wi-Fi: access point start failed");
            // esp-radio leaves the radio stopped after a failed configuration.
            if radio
                .controller
                .set_config(&Config::Station(radio.station.clone()))
                .is_err()
            {
                warn!("Wi-Fi: station mode could not be restored");
            }
            self.ap_config = None;
            return false;
        }
        if with_timeout(ACCESS_POINT_READY_TIMEOUT, stack.wait_link_up())
            .await
            .is_err()
        {
            warn!("Wi-Fi: access point radio did not come up");
            if let Some(radio) = self.radio.as_mut() {
                let _ = radio
                    .controller
                    .set_config(&Config::Station(radio.station.clone()));
            }
            self.ap_config = None;
            return false;
        }
        // (Re)starts the DHCP service: a changed configuration drops the old clients.
        DHCP_ENABLED.signal(true);
        self.ap_config = Some(config.clone());
        info!("Wi-Fi: access point active");
        true
    }

    async fn stop_access_point(&mut self) {
        if self.ap_config.take().is_none() {
            return;
        }
        // Revoke the address service first, then change the radio mode.
        DHCP_STOPPED.reset();
        DHCP_ENABLED.signal(false);
        if with_timeout(DHCP_STOP_TIMEOUT, DHCP_STOPPED.wait())
            .await
            .is_err()
        {
            warn!("Wi-Fi: DHCP service did not acknowledge the stop");
        }
        self.connection.invalidate();
        if let Some(radio) = self.radio.as_mut() {
            if radio
                .controller
                .set_config(&Config::Station(radio.station.clone()))
                .is_err()
            {
                warn!("Wi-Fi: station mode could not be restored");
            }
        }
        info!("Wi-Fi: access point stopped");
    }

    fn is_access_point_active(&self) -> bool {
        self.ap_config.is_some()
    }

    fn access_point_handle(&self) -> Option<Self::NetworkHandle> {
        self.ap_config.as_ref().and(self.ap_stack)
    }
}

/// Runs the DHCP server for the access point's clients until the stop signal.
/// Leases are RAM-only and start empty at every activation.
async fn serve_dhcp(stack: Stack<'static>, buffers: &'static DhcpBuffers) {
    let udp = Udp::new(stack, buffers);
    let Ok(mut socket) = udp
        .bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 67)))
        .await
    else {
        warn!("Wi-Fi: DHCP server could not bind");
        return;
    };
    let [a, b, c, d] = ACCESS_POINT_ADDRESS;
    let server_ip = Ipv4Addr::new(a, b, c, d);
    let mut server = DhcpServer::<_, DHCP_LEASES>::new(|| Instant::now().as_secs(), server_ip);
    server.range_start = Ipv4Addr::new(a, b, c, DHCP_FIRST);
    server.range_end = Ipv4Addr::new(a, b, c, DHCP_FIRST + DHCP_LEASES as u8 - 1);
    // No gateway and no DNS: the access point network is local only.
    let mut options = DhcpOptions::new(server_ip, None);
    options.lease_duration_secs = DHCP_LEASE_SECS;
    let mut buffer = [0u8; DHCP_BUFFER_LEN];
    if edge_dhcp::io::server::run(&mut server, &options, &mut socket, &mut buffer)
        .await
        .is_err()
    {
        warn!("Wi-Fi: DHCP server stopped on a socket error");
    }
}

#[embassy_executor::task]
async fn dhcp_task(stack: Stack<'static>, buffers: &'static DhcpBuffers) -> ! {
    let mut enabled = false;
    loop {
        if !enabled {
            enabled = DHCP_ENABLED.wait().await;
            continue;
        }
        match select(serve_dhcp(stack, buffers), DHCP_ENABLED.wait()).await {
            // The server ended on an error: retry shortly while still enabled.
            Either::First(()) => Timer::after(Duration::from_millis(500)).await,
            Either::Second(on) => {
                enabled = on;
                if !on {
                    DHCP_STOPPED.signal(());
                }
            }
        }
    }
}

#[embassy_executor::task(pool_size = 2)]
async fn net_task(mut runner: Runner<'static, Interface>) -> ! {
    runner.run().await
}
