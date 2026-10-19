//! The qualification run: Wi-Fi, SNTP, then the scenarios, one `RESULT` line each.

use alloc::string::String;
use core::sync::atomic::{AtomicU8, Ordering};

use embassy_executor::Spawner;
use embassy_net::{Stack, StackResources};
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, signal::Signal};
use embassy_time::{Duration, Instant, Timer, with_timeout};
use embedded_io_async::Read;
use esp_hal::timer::timg::TimerGroup;
use example_https_client::{Expect, Failure, judge};
use iobewi_config_space::storage::{EkvBackend, SpaceSpec, Store};
use iobewi_config_space::{ConfigManager, ConfigSpace};
use iobewi_esp_config_space::EspConfigFlash;
use iobewi_esp_tls::embassy::{ClientConnectError, EspTlsDialer};
use iobewi_esp_tls::mbedtls_rs::SessionError;
use iobewi_http_client::{Target, get, read_head};
use iobewi_net_io::{Close, Connector};
use iobewi_tls_roots as roots;
use iobewi_tls_service::client::{ClientTlsError, StaticTrustConnector};
use iobewi_wifi_manager::{LinkObserver, Sleep, WifiManager};
use static_cell::StaticCell;

/// The heap of this run: 160 KiB, the smallest size measured to pass every scenario on the
/// board (96 KiB, StreamBeWI's today, does not hold the client; 128 KiB is unreliable; see
/// README). `IOBEWI_HEAP_KIB=<n>` at build time sets another size to measure.
const HEAP_BYTES: usize = match option_env!("IOBEWI_HEAP_KIB") {
    Some(kib) => parse_kib(kib) * 1024,
    None => 160 * 1024,
};

const fn parse_kib(text: &str) -> usize {
    let bytes = text.as_bytes();
    let mut value = 0;
    let mut i = 0;
    while i < bytes.len() {
        value = value * 10 + (bytes[i] - b'0') as usize;
        i += 1;
    }
    value
}
const SOCKETS: usize = 5; // DHCP, DNS, SNTP, the HTTPS connection, one spare.

/// The configuration manifest of the StreamBeWI board this is qualified on.
///
/// `Store::mount` checks the stored records against the manifest, so the region
/// of a board already in service mounts only with its complete manifest. The
/// example never formats: a blank or unmountable region stops it. The Wi-Fi
/// credentials are the ones that region holds (provisioned with Improv Serial);
/// none is built into the image.
const SPACES: &[SpaceSpec] = &[
    SpaceSpec::new("stations", 4096),
    SpaceSpec::new("wifi", iobewi_wifi_manager::CONFIG_BUDGET.max_bytes()),
    SpaceSpec::new("usb_boot", 5),
    SpaceSpec::new("log", 1024),
];

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const READ_TIMEOUT: Duration = Duration::from_secs(8);
/// The server the product streams from. Everything that needs a *valid* chain uses it, so that
/// the public test site below is hit only by the cases that need a faulty certificate.
const RADIO: &str = "https://icecast.radiofrance.fr/monpetitfranceinter-midfi.mp3";
const UA: &str = "iobewi-https-client/0.1";

/// 2026-01-01T00:00:00Z: an SNTP answer below it is not a clock.
const EPOCH_FLOOR: u64 = 1_767_225_600;
/// 2020-01-01: before every certificate under test was issued.
const EPOCH_PAST: u64 = 1_577_836_800;
/// 2099-01-01: after every certificate under test has expired.
const EPOCH_FUTURE: u64 = 4_070_908_800;

/// What the TLS stack believes the time is (see `now_unix`).
static CLOCK: AtomicU8 = AtomicU8::new(CLOCK_REAL);
const CLOCK_REAL: u8 = 0;
const CLOCK_PAST: u8 = 1;
const CLOCK_FUTURE: u8 = 2;

/// The clock the certificate checks read. The qualification moves it into the
/// past and the future to prove that out-of-validity certificates are refused;
/// `iobewi_time::is_set` (the connector's guard) is not affected.
fn now_unix() -> Option<u64> {
    match CLOCK.load(Ordering::Relaxed) {
        CLOCK_PAST => Some(EPOCH_PAST),
        CLOCK_FUTURE => Some(EPOCH_FUTURE),
        _ => iobewi_time::now(),
    }
}

struct Bufs {
    rx: [u8; 4096],
    tx: [u8; 2048],
    head: [u8; 2048],
}

struct Done {
    status: u16,
    connect_ms: u64,
    head_ms: u64,
    body: usize,
    body_ms: u64,
}

fn failure(error: &ClientTlsError<ClientConnectError>) -> Failure {
    match error {
        ClientTlsError::ClockUnsynced => Failure::ClockUnsynced,
        ClientTlsError::NoCa => Failure::NoCa,
        ClientTlsError::Connect(error) => match error {
            ClientConnectError::BadCa | ClientConnectError::InvalidHost => Failure::BadCa,
            ClientConnectError::Dns => Failure::Dns,
            ClientConnectError::Tcp(_) => Failure::Tcp,
            ClientConnectError::Handshake(SessionError::MbedTls(e)) => {
                Failure::from_handshake_code(e.code())
            }
            ClientConnectError::Handshake(SessionError::Io(_)) => Failure::Handshake(0),
        },
    }
}

type Connector_ = StaticTrustConnector<EspTlsDialer>;

/// Connect, send a GET, read the response head and up to `body_limit` bytes of body, close.
async fn fetch(
    connector: &Connector_,
    bufs: &mut Bufs,
    url: &str,
    body_limit: usize,
) -> Result<Done, Failure> {
    let target = Target::parse(url).map_err(|_| Failure::BadCa)?;
    let Bufs { rx, tx, head } = bufs;

    let t0 = Instant::now();
    let mut stream = match with_timeout(
        CONNECT_TIMEOUT,
        connector.connect(&target.host, target.port, rx, tx),
    )
    .await
    {
        Err(_) => return Err(Failure::Timeout),
        Ok(Err(e)) => return Err(failure(&e)),
        Ok(Ok(stream)) => stream,
    };
    let connect_ms = t0.elapsed().as_millis();

    get(
        &mut stream,
        &target.host,
        &target.path,
        &[("User-Agent", UA), ("Icy-MetaData", "0")],
    )
    .await
    .map_err(|_| Failure::Io)?;
    let response = match with_timeout(READ_TIMEOUT, read_head(&mut stream, head)).await {
        Err(_) => Err(Failure::Timeout),
        Ok(Err(_)) => Err(Failure::Io),
        Ok(Ok(h)) => Ok(h),
    };
    let head_ms = t0.elapsed().as_millis() - connect_ms;

    let mut done = Done {
        status: 0,
        connect_ms,
        head_ms,
        body: 0,
        body_ms: 0,
    };
    let result = match response {
        Err(f) => Err(f),
        Ok(h) => {
            done.status = h.status;
            done.body = h.filled - h.body_start;
            let t1 = Instant::now();
            let mut chunk = [0u8; 1460];
            let mut outcome = Ok(());
            while done.body < body_limit && h.status == 200 {
                match with_timeout(READ_TIMEOUT, stream.read(&mut chunk)).await {
                    Err(_) => outcome = Err(Failure::Timeout),
                    Ok(Err(_)) => outcome = Err(Failure::Io),
                    Ok(Ok(0)) => {}
                    Ok(Ok(n)) => {
                        done.body += n;
                        continue;
                    }
                }
                break;
            }
            done.body_ms = t1.elapsed().as_millis();
            outcome.map(|_| ())
        }
    };
    // Graceful close (close-notify, FIN); the figure of interest is that it returns.
    let _ = with_timeout(Duration::from_secs(5), stream.close()).await;
    result.map(|_| done)
}

struct Tally {
    pass: u32,
    fail: u32,
}

impl Tally {
    async fn scenario(
        &mut self,
        name: &str,
        expect: Expect,
        connector: &Connector_,
        bufs: &mut Bufs,
        url: &str,
        body_limit: usize,
    ) -> bool {
        let result = fetch(connector, bufs, url, body_limit).await;
        let observed = match &result {
            Ok(d) => Ok(d.status),
            Err(f) => Err(*f),
        };
        let ok = judge(expect, &observed);
        if ok {
            self.pass += 1
        } else {
            self.fail += 1
        }
        let verdict = if ok { "PASS" } else { "FAIL" };
        match &result {
            Ok(d) => log::info!(
                "RESULT {name} {verdict} expect={expect:?} status={} connect_ms={} head_ms={} body={} body_ms={} url={url}",
                d.status,
                d.connect_ms,
                d.head_ms,
                d.body,
                d.body_ms
            ),
            Err(f) => log::info!(
                "RESULT {name} {verdict} expect={expect:?} refused={} detail={f:?} url={url}",
                f.label()
            ),
        }
        crate::mem::report(name);
        ok
    }
}

async fn qualify(stack: Stack<'static>, spawner: Spawner) {
    static BUFS: StaticCell<Bufs> = StaticCell::new();
    let bufs = BUFS.init(Bufs {
        rx: [0; 4096],
        tx: [0; 2048],
        head: [0; 2048],
    });

    let tls = iobewi_esp_tls::init(now_unix);
    let dialer = EspTlsDialer { tls, stack };
    let full = StaticTrustConnector {
        dialer,
        ca_pem: roots::BUNDLE,
        clock_is_set: iobewi_time::is_set,
    };
    // Trust anchors that cannot validate the host under test.
    let wrong_roots = StaticTrustConnector {
        ca_pem: roots::DIGICERT_GLOBAL_ROOT_G2.pem,
        ..full
    };
    let mut t = Tally { pass: 0, fail: 0 };

    crate::mem::report("start");
    log::info!(
        "QUAL roots={} bundle_bytes={}",
        roots::ALL.len(),
        roots::BUNDLE.len()
    );

    // Before SNTP: the guard must refuse without touching the network.
    t.scenario(
        "clock-unsynced",
        Expect::ClockUnsynced,
        &full,
        bufs,
        "https://badssl.com/",
        0,
    )
    .await;

    let options = iobewi_ntp::SyncOptions {
        server: "pool.ntp.org",
        resync_period: Duration::from_secs(3600),
        retry_period: Duration::from_secs(10),
        exchange_timeout: Duration::from_secs(5),
        plausible_epoch_floor: EPOCH_FLOOR,
    };
    spawner.spawn(iobewi_ntp::sync_task(stack, options).unwrap());
    let t0 = Instant::now();
    if !iobewi_time::wait(Duration::from_secs(60)).await {
        log::error!("RESULT sntp FAIL no time after 60 s");
        t.fail += 1;
        return summary(&t);
    }
    log::info!(
        "RESULT sntp PASS epoch={:?} after_ms={}",
        iobewi_time::now(),
        t0.elapsed().as_millis()
    );
    t.pass += 1;

    let ok = Expect::Status(200);
    let refused = Expect::CertificateRejected;
    // Valid chains. The RSA one is the control for the refusals that follow.
    t.scenario("valid-rsa", ok, &full, bufs, "https://badssl.com/", 1024)
        .await;
    t.scenario(
        "valid-ecdsa-p384",
        ok,
        &full,
        bufs,
        "https://ecc384.badssl.com/",
        1024,
    )
    .await;
    t.scenario("valid-radio", ok, &full, bufs, RADIO, 128 * 1024)
        .await;

    // Refusals. Same server as the control, one fault each.
    t.scenario(
        "wrong-host",
        refused,
        &full,
        bufs,
        "https://wrong.host.badssl.com/",
        0,
    )
    .await;
    t.scenario(
        "unknown-ca",
        refused,
        &wrong_roots,
        bufs,
        "https://badssl.com/",
        0,
    )
    .await;
    t.scenario(
        "untrusted-root",
        refused,
        &full,
        bufs,
        "https://untrusted-root.badssl.com/",
        0,
    )
    .await;
    t.scenario(
        "self-signed",
        refused,
        &full,
        bufs,
        "https://self-signed.badssl.com/",
        0,
    )
    .await;
    CLOCK.store(CLOCK_FUTURE, Ordering::Relaxed);
    t.scenario("expired-clock-2099", refused, &full, bufs, RADIO, 0)
        .await;
    CLOCK.store(CLOCK_PAST, Ordering::Relaxed);
    t.scenario("not-yet-valid-clock-2020", refused, &full, bufs, RADIO, 0)
        .await;
    CLOCK.store(CLOCK_REAL, Ordering::Relaxed);
    t.scenario("valid-again", ok, &full, bufs, RADIO, 256).await;

    // Close and reconnect: the heap must come back to where it was.
    let before = esp_alloc::HEAP.stats().current_usage;
    for i in 0..10 {
        let mut name = String::from("reconnect-");
        name.push((b'0' + i) as char);
        t.scenario(&name, ok, &full, bufs, RADIO, 256).await;
    }
    let immediate = esp_alloc::HEAP.stats().current_usage;
    // Some buffers are released a little after the last close (an 8 520-byte block was seen
    // lingering after a connection): a leak is what is still held after a rest.
    Timer::after(Duration::from_secs(3)).await;
    let after = esp_alloc::HEAP.stats().current_usage;
    let drift = after as isize - before as isize;
    // Only growth is a leak: the first handshakes warm caches and the usage may come down.
    let leak_ok = drift <= 512;
    if leak_ok {
        t.pass += 1
    } else {
        t.fail += 1
    }
    log::info!(
        "RESULT reconnect-heap {} used_before={before} used_immediately={immediate} used_after_3s={after} drift={drift} (limit 512)",
        if leak_ok { "PASS" } else { "FAIL" }
    );
    summary(&t);
}

fn summary(t: &Tally) {
    crate::mem::report("end");
    crate::mem::report_fragmentation();
    log::info!(
        "SUMMARY {} pass={} fail={}",
        if t.fail == 0 { "PASS" } else { "FAIL" },
        t.pass,
        t.fail
    );
}

struct Clock;
impl Sleep for Clock {
    async fn sleep_ms(&self, ms: u32) {
        Timer::after(Duration::from_millis(ms.into())).await;
    }
}

type Ready = Signal<NoopRawMutex, Stack<'static>>;

struct Observer(&'static Ready);
impl LinkObserver<Stack<'static>> for Observer {
    fn ready(&mut self, network: Stack<'static>) {
        log::info!("QUAL Wi-Fi ready");
        self.0.signal(network);
    }
    fn link_down(&mut self) {
        log::warn!("QUAL Wi-Fi link lost; the manager reconnects without reboot");
    }
}

/// The Wi-Fi manager owns the transport for the whole run, in its own task: the
/// reconnection (bounded backoff) is its job, not the example's. Composing it
/// with the TLS handshakes in one task overflowed the S3 main stack (#72).
#[embassy_executor::task]
async fn wifi_task(
    transport: iobewi_esp_wifi::WifiManager<SOCKETS>,
    config: ConfigSpace<EkvBackend<'static, EspConfigFlash>>,
    ready: &'static Ready,
) {
    if !iobewi_wifi_manager::is_provisioned(&config).await {
        log::error!(
            "QUAL no Wi-Fi credentials in the config partition: provision with Improv Serial"
        );
        return;
    }
    let mut manager = WifiManager::new(transport, config);
    let result = manager.maintain(&Clock, &mut Observer(ready)).await;
    log::error!("QUAL Wi-Fi manager stopped: {result:?}");
}

/// Proof of life and of the heap, every 5 s, from a task of its own.
#[embassy_executor::task]
async fn heartbeat() {
    let mut ticks = 0u32;
    loop {
        let started = Instant::now();
        Timer::after(Duration::from_millis(10)).await;
        crate::mem::note_lateness_ms(started.elapsed().as_millis().saturating_sub(10) as u32);
        crate::mem::sample();
        ticks += 1;
        if ticks % 500 == 0 {
            crate::mem::report("alive");
        }
    }
}

#[embassy_executor::task]
async fn main_task(
    flash: esp_hal::peripherals::FLASH<'static>,
    wifi: esp_hal::peripherals::WIFI<'static>,
    spawner: Spawner,
) {
    crate::mem::report("task-start");
    #[cfg(all(feature = "dram2", not(feature = "no-memtest")))]
    {
        let (ok, detail) = crate::mem::dram2_integrity();
        log::info!(
            "RESULT mem-integrity {} {detail}",
            if ok { "PASS" } else { "FAIL" }
        );
        if !ok {
            log::error!("QUAL memory integrity failed: stopping before the radio starts");
            return;
        }
        crate::mem::report("after-integrity");
    }
    spawner.spawn(heartbeat().unwrap());
    let shared = iobewi_esp_flash::init(flash);
    let Ok(region) = EspConfigFlash::from_label(shared, "config").await else {
        log::error!("QUAL config partition unavailable");
        return;
    };
    match region.is_blank().await {
        Ok(false) => {}
        _ => {
            log::error!("QUAL config partition blank or unreadable: refusing to format it");
            return;
        }
    }
    static STORE: StaticCell<Store<EspConfigFlash>> = StaticCell::new();
    let Ok(store) = Store::new(region, esp_hal::rng::Rng::new().random(), SPACES) else {
        log::error!("QUAL config manifest refused");
        return;
    };
    let store = STORE.init(store);
    crate::mem::report("store-new");
    if let Err(error) = store.mount().await {
        log::error!("QUAL config mount failed: {error:?}; not formatting");
        return;
    }
    crate::mem::report("mounted");
    let mut manager = ConfigManager::new(store.backend());
    let Ok(wifi_space) = manager.claim("wifi", iobewi_wifi_manager::CONFIG_BUDGET) else {
        log::error!("QUAL Wi-Fi config claim failed");
        return;
    };

    static RESOURCES: StaticCell<StackResources<SOCKETS>> = StaticCell::new();
    let transport = iobewi_esp_wifi::WifiManager::<SOCKETS>::new(
        wifi,
        spawner,
        RESOURCES.init(StackResources::new()),
    );
    crate::mem::report("wifi-transport");
    static READY: StaticCell<Ready> = StaticCell::new();
    let ready = READY.init(Ready::new());
    spawner.spawn(wifi_task(transport, wifi_space, ready).unwrap());
    let Ok(stack) = with_timeout(Duration::from_secs(120), ready.wait()).await else {
        log::error!("QUAL Wi-Fi not ready after 120 s");
        return;
    };
    stack.wait_config_up().await;
    log::info!("QUAL net up heap={} KiB", HEAP_BYTES / 1024);
    qualify(stack, spawner).await;
    loop {
        Timer::after(Duration::from_secs(60)).await;
        crate::mem::report("idle");
    }
}

/// Size of the second region: all of `dram2_seg` (0x3FCDB700..0x3FCED710). The linker fails if
/// it does not fit; how much of it the allocator can hand out is measured, not assumed.
#[cfg(feature = "dram2")]
const DRAM2_BYTES: usize = 73_744;

/// Add the DRAM the 2nd stage bootloader frees as a second, separate heap region. The two regions
/// stay disjoint: the allocator tries the first region added, then the next.
#[cfg(feature = "dram2")]
fn add_dram2() {
    use core::mem::MaybeUninit;
    #[esp_hal::ram(reclaimed)]
    static mut DRAM2: MaybeUninit<[u8; DRAM2_BYTES]> = MaybeUninit::uninit();
    let start = core::ptr::addr_of_mut!(DRAM2) as *mut u8;
    crate::mem::dram2_boot_probe(start as usize, DRAM2_BYTES);
    // SAFETY: `DRAM2` is placed in `.dram2_uninit`, memory the bootloader no longer uses once the
    // application runs; it is used by nothing else and handed over once, before any allocation
    // from it.
    unsafe {
        esp_alloc::HEAP.add_region(esp_alloc::HeapRegion::new(
            start,
            DRAM2_BYTES,
            esp_alloc::MemoryCapability::Internal.into(),
        ));
    }
}

fn init_heap() {
    #[cfg(feature = "dram2-first")]
    add_dram2();
    esp_alloc::heap_allocator!(size: HEAP_BYTES);
    #[cfg(all(feature = "dram2", not(feature = "dram2-first")))]
    add_dram2();
}

pub fn boot() -> ! {
    #[cfg(feature = "cpu-max")]
    let config = esp_hal::Config::default().with_cpu_clock(esp_hal::clock::CpuClock::max());
    #[cfg(not(feature = "cpu-max"))]
    let config = esp_hal::Config::default();
    let peripherals = esp_hal::init(config);
    // The application target must be the binary's module path: other targets only log warnings.
    iobewi_log::install(iobewi_esp_console::console_print, "https_client_esp32");
    init_heap();
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);

    static EXECUTOR: StaticCell<esp_rtos::embassy::Executor> = StaticCell::new();
    let executor = EXECUTOR.init(esp_rtos::embassy::Executor::new());
    executor.run(|spawner: Spawner| {
        spawner.spawn(main_task(peripherals.FLASH, peripherals.WIFI, spawner).unwrap());
    })
}
