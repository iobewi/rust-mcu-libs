#![no_std]
#![no_main]

//! `state` → `indicator` → WS2812 on the board's RGB LED, with two simulated sources and two
//! independent consumers.
//!
//! ```text
//! scenario task ─ Wi-Fi source ─┐
//!                 USB source  ──┤                ┌─ Observer ─ LED task ─ Indicator ─ WS2812
//!                               └─ Registry ─────┤
//!                                                └─ Observer ─ diagnostic task (per source)
//! ```
//!
//! The scenario plays eight steps in a loop and prints each one. The LED task maps the
//! **aggregate** of the flags to a pattern; the diagnostic task prints what **each source** did.
//! Steps 4 and 5 are the point: when the Wi-Fi drops `Connected` while USB still holds it, the
//! aggregate does not change, the diagnostic task sees the Wi-Fi change, and the LED is neither
//! rewritten nor restarted.
//!
//! The mapping from flags to patterns ([`look`]) belongs to this example. `iobewi-state` and
//! `iobewi-indicator` know nothing of each other.
//!
//! Board wiring, chosen by feature (no library holds a pin number):
//!
//! | Feature | Board | LED pin |
//! | --- | --- | --- |
//! | `esp32s3` | ESP32-S3-DevKitC-1 v1.0 | GPIO48 |
//! | `esp32s3` + `devkitc-1-v1-1` | ESP32-S3-DevKitC-1 v1.1 | GPIO38 |
//! | `esp32c3` | ESP32-C3-DevKitM-1 | GPIO8 |

#[cfg(all(feature = "esp32c3", feature = "esp32s3"))]
compile_error!("Choose exactly one MCU feature");
#[cfg(not(any(feature = "esp32c3", feature = "esp32s3")))]
compile_error!("Choose --features esp32c3 or esp32s3");
#[cfg(all(feature = "devkitc-1-v1-1", not(feature = "esp32s3")))]
compile_error!("devkitc-1-v1-1 selects the LED pin of the ESP32-S3-DevKitC-1");

use embassy_executor::Spawner;
use embassy_futures::select::{Either, select};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_time::{Instant, Timer};
use esp_hal::{rmt::Rmt, timer::timg::TimerGroup};
use iobewi_esp_console::println;
use iobewi_esp_ws2812::{RMT_FREQ, WriteError, Ws2812};
use iobewi_indicator::{Indicator, Millis, Pattern, RGB8};
use iobewi_state::{Flags, Observer, Registry, Snapshot, Source, SourceId, State};
use static_cell::StaticCell;

esp_bootloader_esp_idf::esp_app_desc!();

const SOURCES: usize = 4;
const OBSERVERS: usize = 2;

type Reg = Registry<CriticalSectionRawMutex, SOURCES, OBSERVERS>;
type Obs = Observer<'static, CriticalSectionRawMutex, SOURCES, OBSERVERS>;
type Src = Source<'static, CriticalSectionRawMutex, SOURCES, OBSERVERS>;
type Snap = Snapshot<SOURCES>;

/// The registry of the example. Nothing is allocated.
static STATE: Reg = Registry::new();

/// The on-board LED is bright.
const INTENSITY: u8 = 32;

const WHITE: RGB8 = RGB8::new(255, 255, 255);
const BLUE: RGB8 = RGB8::new(0, 0, 255);
const GREEN: RGB8 = RGB8::new(0, 255, 0);
const RED: RGB8 = RGB8::new(255, 0, 0);

/// The policy of this example: which pattern the aggregate gets. An error wins over everything,
/// then a link, then a link being made, then the initialisation, otherwise the LED is dark.
fn look(flags: Flags) -> (Pattern, &'static str) {
    if flags.contains(State::Error) {
        let pattern = Pattern::Blink {
            color: RED,
            on_ms: 250,
            off_ms: 250,
        };
        (pattern, "red blinking")
    } else if flags.contains(State::Connected) {
        (Pattern::Solid(GREEN), "green steady")
    } else if flags.contains(State::Connecting) {
        let pattern = Pattern::Blink {
            color: BLUE,
            on_ms: 300,
            off_ms: 300,
        };
        (pattern, "blue blinking")
    } else if flags.contains(State::Initializing) {
        (Pattern::Solid(WHITE), "white steady")
    } else {
        (Pattern::Off, "off")
    }
}

fn now_ms() -> Millis {
    // The low 32 bits are enough: the indicator works modulo 2^32 ms.
    Instant::now().as_millis() as Millis
}

/// Sends the colour to the LED if it changed. The blocking write of one LED takes a few tens of
/// microseconds: it does not hold the executor.
fn show(
    ind: &mut Indicator,
    led: &mut Ws2812<'_>,
    writes: &mut u32,
    now: Millis,
) -> Result<Option<Millis>, WriteError> {
    ind.drive(now, |color| {
        *writes += 1;
        led.set(color)
    })
}

/// Consumer 1: the aggregate decides the pattern. It wakes on every change of the registry, and
/// between changes when the indicator says the colour can next change (blink, pulse).
#[embassy_executor::task]
async fn led_task(mut obs: Obs, mut led: Ws2812<'static>) {
    let mut ind = Indicator::new(Pattern::Off, now_ms());
    ind.set_intensity(INTENSITY);
    let mut writes = 0u32;

    let mut snap: Snap = obs.changed().await;
    loop {
        let (pattern, name) = look(snap.aggregate());
        let same = pattern == ind.pattern();
        let now = now_ms();
        ind.set(pattern, now);
        let before = writes;
        let mut wake = match show(&mut ind, &mut led, &mut writes, now) {
            Ok(wake) => wake,
            Err(_) => {
                println!("[LED ] write failed: the driver is disabled, the LED task stops");
                return;
            }
        };
        println!(
            "[LED ] rev {} aggregate {:?} -> {} ({}), LED writes +{}",
            snap.revision(),
            snap.aggregate(),
            name,
            if same { "same pattern" } else { "new pattern" },
            writes - before,
        );

        snap = loop {
            let event = match wake {
                Some(ms) => select(obs.changed(), Timer::after_millis(u64::from(ms))).await,
                None => Either::First(obs.changed().await),
            };
            match event {
                Either::First(next) => break next,
                // The colour can change although no flag did: blink and pulse.
                Either::Second(()) => match show(&mut ind, &mut led, &mut writes, now_ms()) {
                    Ok(next) => wake = next,
                    Err(_) => {
                        println!("[LED ] write failed: the driver is disabled, the LED task stops");
                        return;
                    }
                },
            }
        };
    }
}

fn who(id: SourceId, wifi: SourceId, usb: SourceId) -> &'static str {
    if id == wifi {
        "wifi"
    } else if id == usb {
        "usb "
    } else {
        "?   "
    }
}

/// Consumer 2: what each source did, whether or not the aggregate moved. It knows nothing of the
/// LED.
#[embassy_executor::task]
async fn diag_task(mut obs: Obs, wifi: SourceId, usb: SourceId) {
    let mut previous = Snap::EMPTY;
    loop {
        let snap = obs.changed().await;
        println!(
            "[DIAG] rev {} aggregate {:?}",
            snap.revision(),
            snap.aggregate()
        );
        for change in snap.changes_since(&previous) {
            println!(
                "[DIAG]   {} {:?} +{:?} -{:?}",
                who(change.source, wifi, usb),
                change.kind,
                change.raised,
                change.cleared
            );
        }
        previous = snap;
    }
}

/// The producers: two simulated sources playing the scenario in a loop.
#[embassy_executor::task]
async fn scenario_task(wifi: Src, usb: Src) {
    // Let both consumers print their first state.
    Timer::after_millis(1_500).await;
    let mut cycle = 0u32;
    loop {
        cycle += 1;
        println!("=== cycle {cycle} ===");

        step(1, "Wi-Fi raises Initializing", "white steady");
        wifi.raise(State::Initializing);
        Timer::after_millis(3_000).await;

        step(2, "Wi-Fi replaces its set by {Connecting}", "blue blinking");
        wifi.replace(Flags::of(&[State::Connecting]));
        Timer::after_millis(5_000).await;

        step(3, "Wi-Fi replaces its set by {Connected}", "green steady");
        wifi.replace(Flags::of(&[State::Connected]));
        Timer::after_millis(3_000).await;

        step(4, "USB raises Connected", "green steady, unchanged");
        usb.raise(State::Connected);
        Timer::after_millis(3_000).await;

        step(
            5,
            "Wi-Fi clears Connected",
            "green steady, unchanged: USB still holds Connected",
        );
        wifi.clear(State::Connected);
        Timer::after_millis(4_000).await;

        step(6, "USB clears Connected", "off");
        usb.clear(State::Connected);
        Timer::after_millis(3_000).await;

        step(7, "USB raises Error", "red blinking");
        usb.raise(State::Error);
        Timer::after_millis(5_000).await;

        step(8, "USB clears Error", "off");
        usb.clear(State::Error);
        Timer::after_millis(4_000).await;
    }
}

fn step(n: u32, action: &str, expected: &str) {
    println!(
        "STEP {n}: {action} | registry rev {} | expected LED: {expected}",
        STATE.snapshot().revision()
    );
}

#[esp_hal::main]
fn main() -> ! {
    let p = esp_hal::init(esp_hal::Config::default());

    #[cfg(all(feature = "esp32s3", not(feature = "devkitc-1-v1-1")))]
    let led_pin = p.GPIO48;
    #[cfg(all(feature = "esp32s3", feature = "devkitc-1-v1-1"))]
    let led_pin = p.GPIO38;
    #[cfg(feature = "esp32c3")]
    let led_pin = p.GPIO8;

    let timg0 = TimerGroup::new(p.TIMG0);
    esp_rtos::start(timg0.timer0, p.FROM_CPU_INTR0);

    let Ok(rmt) = Rmt::new(p.RMT, RMT_FREQ) else {
        halt()
    };
    let Ok(led) = Ws2812::new(rmt.channel0, led_pin) else {
        halt()
    };
    let (Ok(wifi), Ok(usb)) = (STATE.register(), STATE.register()) else {
        halt()
    };
    let (Some(led_obs), Some(diag_obs)) = (STATE.observer(), STATE.observer()) else {
        halt()
    };
    let (wifi_id, usb_id) = (wifi.id(), usb.id());

    static EXECUTOR: StaticCell<esp_rtos::embassy::Executor> = StaticCell::new();
    let executor = EXECUTOR.init(esp_rtos::embassy::Executor::new());
    executor.run(|spawner: Spawner| {
        // The tasks only fail to spawn if they were already running.
        spawner.spawn(led_task(led_obs, led).unwrap());
        spawner.spawn(diag_task(diag_obs, wifi_id, usb_id).unwrap());
        spawner.spawn(scenario_task(wifi, usb).unwrap());
    })
}

fn halt() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    iobewi_esp_console::panic_print(info);
    halt()
}
