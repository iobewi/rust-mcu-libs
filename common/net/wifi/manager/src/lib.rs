#![no_std]
#![allow(async_fn_in_trait)]

//! Portable Wi-Fi component: durable credentials, reconnection and
//! reprovisioning. Radio, DHCP and network-stack types belong to an adapter.

extern crate alloc;
#[cfg(test)]
extern crate std;

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Debug;
use iobewi_config_space::{Budget, ConfigBackend, ConfigSpace};
use iobewi_wifi_core::{AccessPointConfig, Network, WifiAccessPoint, WifiProvisioning, WifiTransport};
use log::{info, warn};

const CONFIG_MAGIC: &[u8; 4] = b"WFC1";
const CONFIG_HEADER_LEN: usize = 6;
/// Unchanged durable budget for station credentials.
pub const CONFIG_BUDGET: Budget = Budget::new(128);

#[derive(Clone)]
struct WifiConfig {
    ssid: String,
    password: String,
}

impl WifiConfig {
    fn encode(&self) -> Option<Vec<u8>> {
        let ssid_len = u8::try_from(self.ssid.len()).ok()?;
        let password_len = u8::try_from(self.password.len()).ok()?;
        let total = CONFIG_HEADER_LEN.checked_add(self.ssid.len())?.checked_add(self.password.len())?;
        if total > CONFIG_BUDGET.max_bytes() { return None; }
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(CONFIG_MAGIC);
        out.push(ssid_len);
        out.push(password_len);
        out.extend_from_slice(self.ssid.as_bytes());
        out.extend_from_slice(self.password.as_bytes());
        Some(out)
    }

    fn decode(raw: &[u8]) -> Option<Self> {
        if raw.len() < CONFIG_HEADER_LEN || &raw[..4] != CONFIG_MAGIC { return None; }
        let ssid_len = raw[4] as usize;
        let password_len = raw[5] as usize;
        let expected = CONFIG_HEADER_LEN.checked_add(ssid_len)?.checked_add(password_len)?;
        if raw.len() != expected { return None; }
        let ssid_end = CONFIG_HEADER_LEN + ssid_len;
        let ssid = core::str::from_utf8(&raw[CONFIG_HEADER_LEN..ssid_end]).ok()?;
        let password = core::str::from_utf8(&raw[ssid_end..]).ok()?;
        Some(Self { ssid: String::from(ssid), password: String::from(password) })
    }
}

pub async fn is_provisioned<B: ConfigBackend>(space: &ConfigSpace<B>) -> bool {
    match space.load().await {
        Ok(Some(snapshot)) => WifiConfig::decode(&snapshot.data)
            .is_some_and(|config| !config.ssid.is_empty()),
        _ => false,
    }
}

/// Reconnect backoff: the wait after the Nth consecutive failed attempt is
/// `1 s << (N-1)`, bounded at [`BACKOFF_MAX_MS`] (1, 2, 4, 8, 10, 10, ... s).
/// No jitter (no synchronised-fleet problem is being solved) and no attempt
/// limit: a lost AP may come back at any time.
pub const BACKOFF_BASE_MS: u32 = 1_000;
pub const BACKOFF_MAX_MS: u32 = 10_000;

pub fn backoff_ms(failed_attempts: u32) -> u32 {
    let shift = failed_attempts.saturating_sub(1).min(16);
    (BACKOFF_BASE_MS << shift).min(BACKOFF_MAX_MS)
}

/// Timer port for the backoff (the portable manager carries no executor).
pub trait Sleep {
    async fn sleep_ms(&self, ms: u32);
}

/// What [`WifiManager::maintain`] reports to its owner. Informational only:
/// the manager stays the single owner of the connection policy.
pub trait LinkObserver<H> {
    /// The link/IP configuration was lost; reconnection is under way.
    fn link_down(&mut self);
    /// Association and DHCP are up (also called for the first connection).
    fn ready(&mut self, network: H);
}

/// The only terminal outcome of [`WifiManager::maintain`].
#[derive(Debug, PartialEq, Eq)]
pub enum MaintainError {
    /// No usable saved credentials: retrying cannot help.
    NotProvisioned,
}

pub struct WifiManager<T, B: ConfigBackend> {
    transport: T,
    config: ConfigSpace<B>,
}

impl<T: WifiTransport, B: ConfigBackend> WifiManager<T, B>
where
    B::Error: Debug,
{
    pub fn new(transport: T, config: ConfigSpace<B>) -> Self {
        Self { transport, config }
    }

    async fn saved_config(&self) -> Option<WifiConfig> {
        match self.config.load().await {
            Ok(Some(snapshot)) => match WifiConfig::decode(&snapshot.data) {
                Some(config) => Some(config),
                None => {
                    warn!("Wi-Fi: stored config generation={} has an unsupported/corrupt schema", snapshot.generation);
                    None
                }
            },
            Ok(None) => None,
            Err(e) => { warn!("Wi-Fi: config-space load failed: {e:?}"); None }
        }
    }

    pub async fn reconnect_saved(&mut self) -> bool {
        let Some(config) = self.saved_config().await else { return false; };
        info!("Wi-Fi: reconnecting to saved SSID={}", config.ssid);
        self.transport.connect(&config.ssid, config.password).await
    }

    /// Keeps the saved network connected, forever: connect (with bounded
    /// backoff between failed attempts), report `ready`, wait for the
    /// transport's link-down event, report `link_down`, repeat. Handles both
    /// the first connection at boot (an absent AP is just a failed attempt)
    /// and later link loss. Returns only when the saved credentials are
    /// absent/invalid.
    ///
    /// Ownership/cancellation: it borrows the manager exclusively, so it can
    /// never overlap a reprovisioning; to reprovision, drop this future (it
    /// holds no state worth keeping, the attempt counter restarts), call
    /// [`WifiManager::provision`], then call `maintain` again.
    pub async fn maintain<S: Sleep, O: LinkObserver<T::NetworkHandle>>(
        &mut self,
        sleep: &S,
        observer: &mut O,
    ) -> MaintainError {
        loop {
            let mut failed = 0u32;
            let network = loop {
                let Some(config) = self.saved_config().await else {
                    return MaintainError::NotProvisioned;
                };
                if config.ssid.is_empty() {
                    return MaintainError::NotProvisioned;
                }
                info!("wifi: connect attempt {} to SSID={}", failed + 1, config.ssid);
                if self.transport.connect(&config.ssid, config.password).await {
                    if let Some(network) = self.transport.network_handle() {
                        break network;
                    }
                    warn!("wifi: connected but no network handle");
                }
                failed += 1;
                let wait = backoff_ms(failed);
                warn!("wifi: connect failed, backoff {wait} ms");
                sleep.sleep_ms(wait).await;
            };
            info!("wifi: ready after {} failed attempt(s)", failed);
            observer.ready(network);
            self.transport.wait_down().await;
            warn!("wifi: link down");
            observer.link_down();
        }
    }

    async fn restore_previous(&mut self, previous: Option<WifiConfig>) {
        let Some(previous) = previous else { return; };
        info!("Wi-Fi: restoring previous SSID={} after failed reprovision", previous.ssid);
        if !self.transport.connect(&previous.ssid, previous.password).await {
            warn!("Wi-Fi: previous network could not be restored");
        }
    }

    /// Publish credentials only after association, DHCP and atomic commit.
    pub async fn provision(&mut self, ssid: &str, password: String) -> bool {
        let previous = self.saved_config().await;
        if !self.transport.connect(ssid, password.clone()).await {
            self.restore_previous(previous).await;
            return false;
        }
        let candidate = WifiConfig { ssid: String::from(ssid), password };
        let Some(encoded) = candidate.encode() else {
            warn!("Wi-Fi: candidate credentials exceed config-space schema limits");
            self.restore_previous(previous).await;
            return false;
        };
        match self.config.commit(&encoded).await {
            Ok(generation) => { info!("Wi-Fi: configuration committed generation={generation}"); true }
            Err(e) => {
                warn!("Wi-Fi: connected, but durable config commit failed: {e:?}");
                self.restore_previous(previous).await;
                false
            }
        }
    }

    pub async fn scan(&mut self) -> Vec<Network> { self.transport.scan().await }
    pub fn ip(&self) -> Option<T::Address> { self.transport.ip() }
    pub fn network_handle(&self) -> Option<T::NetworkHandle> { self.transport.network_handle() }
    pub fn is_online(&self) -> bool { self.transport.is_online() }
}

/// Access point control for a transport that can host one. Pure delegation:
/// *when* to open it (an unprovisioned device, a button, repeated failures) and
/// for how long is product policy, and nothing here persists anything -- the
/// access point lives in RAM only.
///
/// All of these borrow the manager exclusively, so they never overlap
/// [`WifiManager::maintain`]. Starting or stopping the access point may restart
/// the radio and drop the station link (see [`WifiAccessPoint`]); the intended
/// sequence is: drop the `maintain` future, change the access point, call
/// `maintain` again -- it reconnects from the saved credentials. For a device
/// with no saved credentials (`MaintainError::NotProvisioned`): start the
/// access point, serve the product's provisioning page, call
/// [`WifiManager::provision`] (it connects first and commits only on success,
/// with the access point still up), stop the access point, then `maintain`.
impl<T: WifiTransport + WifiAccessPoint, B: ConfigBackend> WifiManager<T, B> {
    pub async fn start_access_point(&mut self, config: &AccessPointConfig) -> bool {
        // The SSID and passphrase are never logged.
        info!("Wi-Fi: starting access point on channel {}", config.channel());
        let started = self.transport.start_access_point(config).await;
        if !started {
            warn!("Wi-Fi: access point could not be started");
        }
        started
    }

    pub async fn stop_access_point(&mut self) {
        info!("Wi-Fi: stopping access point");
        self.transport.stop_access_point().await;
    }

    pub fn is_access_point_active(&self) -> bool {
        self.transport.is_access_point_active()
    }

    pub fn access_point_handle(&self) -> Option<<T as WifiAccessPoint>::NetworkHandle> {
        self.transport.access_point_handle()
    }
}

/// Delegates straight to `WifiManager`'s own methods -- no policy lives
/// here, this only narrows the surface a provisioning workflow sees.
impl<T: WifiTransport, B: ConfigBackend> WifiProvisioning for WifiManager<T, B>
where
    B::Error: Debug,
    T::Address: core::fmt::Display,
{
    type Address = T::Address;
    type NetworkHandle = T::NetworkHandle;

    async fn scan(&mut self) -> Vec<Network> {
        self.transport.scan().await
    }

    async fn provision(&mut self, ssid: &str, password: String) -> bool {
        // Explicit associated-function syntax, not `self.provision(...)`:
        // this impl and the inherent one share the method name, and this
        // makes unambiguous which one carries the real
        // connect+commit+restore-on-failure policy.
        Self::provision(self, ssid, password).await
    }

    fn address(&self) -> Option<Self::Address> {
        self.transport.ip()
    }

    fn network_handle(&self) -> Option<Self::NetworkHandle> {
        self.transport.network_handle()
    }

    fn is_online(&self) -> bool {
        self.transport.is_online()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::{BTreeMap, VecDeque};
    use alloc::string::ToString;
    use alloc::rc::Rc;
    use core::cell::RefCell;
    use core::future::Future;
    use core::task::{Context, Poll, Waker};
    use iobewi_config_space::{ConfigManager, Snapshot};

    fn block_on<F: Future>(future: F) -> F::Output {
        let mut future = core::pin::pin!(future);
        let mut cx = Context::from_waker(Waker::noop());
        loop {
            if let Poll::Ready(v) = future.as_mut().poll(&mut cx) {
                return v;
            }
        }
    }

    #[derive(Default)]
    struct Mem {
        values: BTreeMap<String, Snapshot>,
        generation: u64,
        fail_commit: bool,
    }

    #[derive(Clone, Default)]
    struct MemBackend(Rc<RefCell<Mem>>);

    impl ConfigBackend for MemBackend {
        type Error = ();
        fn capacity_units(&self) -> usize { 4096 }
        fn reservation_units(&self, _: &str, b: Budget) -> Option<usize> { Some(b.max_bytes()) }
        async fn load(&self, space: &str) -> Result<Option<Snapshot>, ()> {
            Ok(self.0.borrow().values.get(space).cloned())
        }
        async fn commit(&self, space: &str, data: &[u8]) -> Result<u64, ()> {
            let mut m = self.0.borrow_mut();
            if m.fail_commit { return Err(()); }
            m.generation += 1;
            let generation = m.generation;
            m.values.insert(space.to_string(), Snapshot { generation, data: data.to_vec() });
            Ok(generation)
        }
        async fn clear(&self, space: &str) -> Result<u64, ()> {
            let mut m = self.0.borrow_mut();
            m.generation += 1;
            m.values.remove(space);
            Ok(m.generation)
        }
    }

    /// Scripted transport: each `connect` pops the next outcome (default: fail)
    /// and records `(ssid, password)`.
    #[derive(Default)]
    struct FakeState {
        outcomes: VecDeque<bool>,
        connects: std::vec::Vec<(String, String)>,
        online: bool,
        down_events: u32,
        ap_active: bool,
        ap_fail: bool,
        /// Models a radio that restarts when the access point starts or stops.
        ap_restarts_station: bool,
        ap_starts: std::vec::Vec<(String, String, u8)>,
        ap_stops: u32,
    }

    #[derive(Clone, Default)]
    struct Fake(Rc<RefCell<FakeState>>);

    impl WifiTransport for Fake {
        type Address = u32;
        type NetworkHandle = u8;
        async fn connect(&mut self, ssid: &str, password: String) -> bool {
            let mut s = self.0.borrow_mut();
            s.connects.push((ssid.to_string(), password));
            let ok = s.outcomes.pop_front().unwrap_or(false);
            s.online = ok;
            ok
        }
        async fn scan(&mut self) -> Vec<Network> {
            alloc::vec![Network { ssid: "lab".to_string(), signal_strength: -40, secured: true }]
        }
        async fn wait_down(&mut self) {
            core::future::poll_fn(|_| {
                let mut s = self.0.borrow_mut();
                if s.down_events > 0 {
                    s.down_events -= 1;
                    s.online = false;
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            })
            .await
        }
        fn ip(&self) -> Option<u32> { self.0.borrow().online.then_some(7) }
        fn network_handle(&self) -> Option<u8> { self.0.borrow().online.then_some(1) }
        fn is_online(&self) -> bool { self.0.borrow().online }
    }

    impl WifiAccessPoint for Fake {
        type NetworkHandle = u16;
        async fn start_access_point(&mut self, config: &AccessPointConfig) -> bool {
            let mut s = self.0.borrow_mut();
            s.ap_starts.push((config.ssid().to_string(), config.password().to_string(), config.channel()));
            if s.ap_fail { return false; }
            s.ap_active = true;
            if s.ap_restarts_station { s.online = false; }
            true
        }
        async fn stop_access_point(&mut self) {
            let mut s = self.0.borrow_mut();
            s.ap_stops += 1;
            if s.ap_active && s.ap_restarts_station { s.online = false; }
            s.ap_active = false;
        }
        fn is_access_point_active(&self) -> bool { self.0.borrow().ap_active }
        fn access_point_handle(&self) -> Option<u16> { self.0.borrow().ap_active.then_some(9) }
    }

    fn ap_config() -> AccessPointConfig {
        AccessPointConfig::new("IOBEWI-Setup", "setup-pass-1", 6).unwrap()
    }

    fn setup(outcomes: &[bool]) -> (WifiManager<Fake, MemBackend>, Fake, MemBackend) {
        let backend = MemBackend::default();
        let fake = Fake::default();
        fake.0.borrow_mut().outcomes = outcomes.iter().copied().collect();
        let space = ConfigManager::new(backend.clone()).claim("wifi", CONFIG_BUDGET).unwrap();
        (WifiManager::new(fake.clone(), space), fake, backend)
    }

    fn stored(b: &MemBackend) -> Option<std::vec::Vec<u8>> {
        b.0.borrow().values.get("wifi").map(|s| s.data.clone())
    }

    fn seed(b: &MemBackend, ssid: &str, pw: &str) {
        let raw = WifiConfig { ssid: ssid.to_string(), password: pw.to_string() }.encode().unwrap();
        b.0.borrow_mut().values.insert("wifi".to_string(), Snapshot { generation: 1, data: raw });
    }

    #[derive(Default)]
    struct Sleeps(RefCell<std::vec::Vec<u32>>);
    impl Sleep for Sleeps {
        async fn sleep_ms(&self, ms: u32) { self.0.borrow_mut().push(ms); }
    }

    /// Never wakes: models a backoff still in progress.
    struct StuckSleep;
    impl Sleep for StuckSleep {
        async fn sleep_ms(&self, _ms: u32) { core::future::pending::<()>().await }
    }

    #[derive(Default)]
    struct Events(std::vec::Vec<&'static str>);
    impl LinkObserver<u8> for Events {
        fn link_down(&mut self) { self.0.push("down"); }
        fn ready(&mut self, _n: u8) { self.0.push("ready"); }
    }

    fn poll_once<F: Future>(f: &mut core::pin::Pin<&mut F>) -> Poll<F::Output> {
        f.as_mut().poll(&mut Context::from_waker(Waker::noop()))
    }

    #[test]
    fn backoff_is_bounded_and_progressive() {
        let seq: std::vec::Vec<u32> = (1..=7).map(backoff_ms).collect();
        assert_eq!(seq, [1000, 2000, 4000, 8000, 10000, 10000, 10000]);
        assert_eq!(backoff_ms(0), 1000);
        assert_eq!(backoff_ms(u32::MAX), 10000);
    }

    #[test]
    fn down_then_failures_then_success_returns_online_without_giving_up() {
        // boot connect ok, link drops, two failures, then success, then drops again.
        let (mut m, fake, b) = setup(&[true, false, false, true]);
        seed(&b, "lab", "pw");
        let sleeps = Sleeps::default();
        let mut ev = Events::default();
        {
            let mut fut = core::pin::pin!(m.maintain(&sleeps, &mut ev));
            assert!(poll_once(&mut fut).is_pending()); // online, waiting for a down event
            fake.0.borrow_mut().down_events = 1;
            assert!(poll_once(&mut fut).is_pending()); // down -> fail, fail -> ok -> waiting
        }
        assert_eq!(ev.0, ["ready", "down", "ready"]);
        assert_eq!(*sleeps.0.borrow(), [1000, 2000]);
        assert_eq!(fake.0.borrow().connects.len(), 4);
        assert!(fake.0.borrow().connects.iter().all(|c| c == &("lab".to_string(), "pw".to_string())));
    }

    #[test]
    fn ap_absent_at_boot_retries_with_backoff_and_resets_after_success() {
        let (mut m, fake, b) = setup(&[false, false, false, false, false, false, true, false, true]);
        seed(&b, "lab", "pw");
        let sleeps = Sleeps::default();
        let mut ev = Events::default();
        {
            let mut fut = core::pin::pin!(m.maintain(&sleeps, &mut ev));
            assert!(poll_once(&mut fut).is_pending());
            assert_eq!(*sleeps.0.borrow(), [1000, 2000, 4000, 8000, 10000, 10000]);
            fake.0.borrow_mut().down_events = 1;
            assert!(poll_once(&mut fut).is_pending());
        }
        // After the success the counter restarts at 1 s.
        assert_eq!(sleeps.0.borrow()[6..], [1000]);
        assert_eq!(ev.0, ["ready", "down", "ready"]);
    }

    #[test]
    fn missing_credentials_are_terminal_not_retried() {
        let (mut m, fake, _b) = setup(&[true]);
        let sleeps = Sleeps::default();
        let mut ev = Events::default();
        assert_eq!(block_on(m.maintain(&sleeps, &mut ev)), MaintainError::NotProvisioned);
        assert!(fake.0.borrow().connects.is_empty());
        assert!(sleeps.0.borrow().is_empty());
        assert!(ev.0.is_empty());
    }

    #[test]
    fn reprovision_interrupts_the_reconnect_and_rollback_still_restores_old_credentials() {
        // maintain: connect(old) fails -> stuck in backoff; dropped for reprovision.
        // provision(new) fails -> restore(old) succeeds.
        let (mut m, fake, b) = setup(&[false, false, true]);
        seed(&b, "old", "oldpw");
        let mut ev = Events::default();
        {
            let mut fut = core::pin::pin!(m.maintain(&StuckSleep, &mut ev));
            assert!(poll_once(&mut fut).is_pending());
            assert_eq!(fake.0.borrow().connects.len(), 1);
        } // dropped: no further connect attempt can run concurrently
        assert!(!block_on(m.provision("new", "newpw".to_string())));
        let calls = fake.0.borrow().connects.clone();
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[1].0, "new");
        assert_eq!(calls[2], ("old".to_string(), "oldpw".to_string()));
        assert_eq!(stored(&b).unwrap(), b"WFC1\x03\x05oldoldpw");
        assert!(m.is_online());
        assert!(ev.0.is_empty());
    }

    #[test]
    fn legacy_wfc1_bytes_round_trip_without_schema_change() {
        let raw = b"WFC1\x03\x04labpass";
        let config = WifiConfig::decode(raw).unwrap();
        assert_eq!(config.ssid, "lab");
        assert_eq!(config.password, "pass");
        assert_eq!(config.encode().unwrap(), raw);
        assert!(WifiConfig::decode(b"WFC1\x03\x04labpas").is_none());
    }

    #[test]
    fn credentials_absent_means_unprovisioned_and_no_connect_attempt() {
        let (mut m, fake, _b) = setup(&[true]);
        assert!(!block_on(m.reconnect_saved()));
        assert!(fake.0.borrow().connects.is_empty());
        assert!(!m.is_online());
    }

    #[test]
    fn corrupt_or_empty_ssid_credentials_are_not_provisioned() {
        let (mut m, fake, b) = setup(&[true]);
        b.0.borrow_mut().values.insert("wifi".into(), Snapshot { generation: 1, data: b"junk".to_vec() });
        assert!(!block_on(m.reconnect_saved()));
        assert!(fake.0.borrow().connects.is_empty());
        seed(&b, "", "x");
        let space = ConfigManager::new(b.clone()).claim("wifi", CONFIG_BUDGET).unwrap();
        assert!(!block_on(is_provisioned(&space)));
    }

    #[test]
    fn saved_credentials_reconnect_with_the_stored_values() {
        let (mut m, fake, b) = setup(&[true]);
        seed(&b, "lab", "secret");
        assert!(block_on(m.reconnect_saved()));
        assert_eq!(fake.0.borrow().connects, [("lab".to_string(), "secret".to_string())]);
        assert!(m.is_online());
        assert_eq!(m.ip(), Some(7));
        assert_eq!(m.network_handle(), Some(1));
        let space = ConfigManager::new(b).claim("wifi", CONFIG_BUDGET).unwrap();
        assert!(block_on(is_provisioned(&space)));
    }

    #[test]
    fn failed_reconnect_is_reported_and_retry_can_succeed() {
        let (mut m, fake, b) = setup(&[false, true]);
        seed(&b, "lab", "pw");
        assert!(!block_on(m.reconnect_saved()));
        assert!(!m.is_online());
        assert!(block_on(m.reconnect_saved()));
        assert_eq!(fake.0.borrow().connects.len(), 2);
    }

    #[test]
    fn provision_commits_only_after_a_successful_connection() {
        let (mut m, _fake, b) = setup(&[true]);
        assert!(block_on(m.provision("home", "pw".to_string())));
        assert_eq!(stored(&b).unwrap(), b"WFC1\x04\x02homepw");
    }

    #[test]
    fn reprovision_failure_restores_the_previous_network_and_keeps_old_credentials() {
        // connect(new) fails, then restore(previous) succeeds.
        let (mut m, fake, b) = setup(&[false, true]);
        seed(&b, "old", "oldpw");
        assert!(!block_on(m.provision("new", "newpw".to_string())));
        let calls = fake.0.borrow().connects.clone();
        assert_eq!(calls, [("new".to_string(), "newpw".to_string()), ("old".to_string(), "oldpw".to_string())]);
        assert_eq!(stored(&b).unwrap(), b"WFC1\x03\x05oldoldpw");
        assert!(m.is_online());
    }

    #[test]
    fn reprovision_success_replaces_the_credentials_in_order() {
        let (mut m, fake, b) = setup(&[true]);
        seed(&b, "old", "oldpw");
        assert!(block_on(m.provision("new", "newpw".to_string())));
        // One connect only: the transport itself owns disconnect-before-reconfigure.
        assert_eq!(fake.0.borrow().connects, [("new".to_string(), "newpw".to_string())]);
        assert_eq!(stored(&b).unwrap(), b"WFC1\x03\x05newnewpw");
    }

    #[test]
    fn commit_failure_after_connect_restores_previous_and_reports_failure() {
        let (mut m, fake, b) = setup(&[true, true]);
        seed(&b, "old", "oldpw");
        b.0.borrow_mut().fail_commit = true;
        assert!(!block_on(m.provision("new", "newpw".to_string())));
        assert_eq!(fake.0.borrow().connects.len(), 2);
        assert_eq!(fake.0.borrow().connects[1].0, "old");
        assert_eq!(stored(&b).unwrap(), b"WFC1\x03\x05oldoldpw");
    }

    #[test]
    fn oversized_credentials_are_rejected_without_a_commit() {
        let (mut m, _fake, b) = setup(&[true, true]);
        let long = "p".repeat(200);
        assert!(!block_on(m.provision("home", long)));
        assert!(stored(&b).is_none());
    }

    #[test]
    fn provisioning_capability_delegates_to_the_manager_policy() {
        let (mut m, _fake, b) = setup(&[true]);
        assert!(block_on(WifiProvisioning::provision(&mut m, "home", "pw".to_string())));
        assert!(stored(&b).is_some());
        assert_eq!(WifiProvisioning::address(&m), Some(7));
        assert_eq!(block_on(WifiProvisioning::scan(&mut m)).len(), 1);
    }

    #[test]
    fn access_point_start_and_stop_delegate_and_write_nothing_to_config_space() {
        let (mut m, fake, b) = setup(&[]);
        assert!(!m.is_access_point_active());
        assert_eq!(m.access_point_handle(), None);
        assert!(block_on(m.start_access_point(&ap_config())));
        assert!(m.is_access_point_active());
        assert_eq!(m.access_point_handle(), Some(9));
        assert_eq!(fake.0.borrow().ap_starts, [("IOBEWI-Setup".to_string(), "setup-pass-1".to_string(), 6)]);
        block_on(m.stop_access_point());
        assert!(!m.is_access_point_active());
        assert_eq!(m.access_point_handle(), None);
        assert_eq!(fake.0.borrow().ap_stops, 1);
        assert!(stored(&b).is_none(), "the access point is RAM-only");
        assert_eq!(b.0.borrow().generation, 0, "no config-space commit or clear happened");
    }

    #[test]
    fn a_failed_start_is_reported_and_leaves_the_access_point_inactive() {
        let (mut m, fake, _b) = setup(&[]);
        fake.0.borrow_mut().ap_fail = true;
        assert!(!block_on(m.start_access_point(&ap_config())));
        assert!(!m.is_access_point_active());
        assert_eq!(m.access_point_handle(), None);
    }

    #[test]
    fn stopping_an_inactive_access_point_is_harmless() {
        let (mut m, _fake, _b) = setup(&[]);
        block_on(m.stop_access_point());
        assert!(!m.is_access_point_active());
    }

    #[test]
    fn provisioning_through_the_access_point_commits_only_after_the_connection() {
        let (mut m, fake, b) = setup(&[false, true]);
        assert!(block_on(m.start_access_point(&ap_config())));
        // A wrong password from the provisioning page: nothing is saved, the access point stays up.
        assert!(!block_on(m.provision("home", "wrong".to_string())));
        assert!(stored(&b).is_none());
        assert!(m.is_access_point_active());
        // The correct one is saved, with the access point still up for the response to reach the phone.
        assert!(block_on(m.provision("home", "right".to_string())));
        assert!(stored(&b).is_some());
        assert!(m.is_access_point_active());
        block_on(m.stop_access_point());
        assert_eq!(fake.0.borrow().connects.len(), 2);
    }

    #[test]
    fn a_radio_restart_caused_by_the_access_point_is_recovered_by_maintain() {
        let (mut m, fake, b) = setup(&[true, true]);
        seed(&b, "lab", "pw");
        fake.0.borrow_mut().ap_restarts_station = true;
        let sleeps = Sleeps::default();
        let mut ev = Events::default();
        {
            let mut fut = core::pin::pin!(m.maintain(&sleeps, &mut ev));
            assert!(poll_once(&mut fut).is_pending()); // online
        } // reprovisioning/AP rule: drop maintain before touching the access point
        assert!(block_on(m.start_access_point(&ap_config())));
        assert!(!fake.0.borrow().online, "the access point start took the station down");
        {
            let mut fut = core::pin::pin!(m.maintain(&sleeps, &mut ev));
            assert!(poll_once(&mut fut).is_pending()); // reconnects from the saved credentials
        }
        assert_eq!(ev.0, ["ready", "ready"]);
        assert!(fake.0.borrow().online);
        assert!(m.is_access_point_active(), "the access point survives the station reconnect");
    }
}
