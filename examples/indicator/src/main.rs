#![no_std]
#![no_main]

//! Shows the patterns of `iobewi-indicator` on the board's WS2812 LED, one after the other, in a
//! loop. It uses no executor, no `common/state` and no logging: a blocking loop that sleeps until
//! the engine says the colour can next change.
//!
//! Board wiring, chosen by feature (neither crate holds a pin number):
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

use esp_hal::{delay::Delay, rmt::Rmt, time::Instant};
use iobewi_esp_ws2812::{RMT_FREQ, Ws2812};
use iobewi_indicator::{Indicator, Millis, Pattern, RGB8};

esp_bootloader_esp_idf::esp_app_desc!();

const RED: RGB8 = RGB8::new(255, 0, 0);
const GREEN: RGB8 = RGB8::new(0, 255, 0);
const BLUE: RGB8 = RGB8::new(0, 0, 255);
const ORANGE: RGB8 = RGB8::new(255, 110, 0);
const WHITE: RGB8 = RGB8::new(255, 255, 255);

/// One scene of the show: what to display, how dim, and for how long.
struct Scene {
    pattern: Pattern,
    intensity: u8,
    ms: Millis,
}

const fn scene(pattern: Pattern, intensity: u8, ms: Millis) -> Scene {
    Scene {
        pattern,
        intensity,
        ms,
    }
}

/// The on-board LEDs are bright: intensities stay low.
const SHOW: [Scene; 9] = [
    scene(Pattern::Solid(RED), 32, 2_000),
    scene(
        Pattern::Blink {
            color: GREEN,
            on_ms: 150,
            off_ms: 150,
        },
        32,
        3_000,
    ),
    scene(
        Pattern::Blink {
            color: BLUE,
            on_ms: 500,
            off_ms: 500,
        },
        32,
        4_000,
    ),
    scene(
        Pattern::Pulse {
            color: BLUE,
            period_ms: 2_000,
        },
        48,
        5_000,
    ),
    // The pattern changes in the middle of a pulse (1.3 s of a 2 s period): it restarts at once.
    scene(
        Pattern::Pulse {
            color: ORANGE,
            period_ms: 2_000,
        },
        48,
        1_300,
    ),
    scene(
        Pattern::Blink {
            color: ORANGE,
            on_ms: 150,
            off_ms: 150,
        },
        48,
        3_000,
    ),
    scene(Pattern::Off, 255, 2_000),
    // Same pattern, three intensities: the intensity changes, the animation does not restart.
    scene(Pattern::Solid(WHITE), 8, 1_500),
    scene(Pattern::Solid(WHITE), 64, 1_500),
];

fn now_ms() -> Millis {
    // The low 32 bits are enough: the engine works modulo 2^32 ms.
    Instant::now().duration_since_epoch().as_millis() as Millis
}

#[esp_hal::main]
fn main() -> ! {
    let p = esp_hal::init(esp_hal::Config::default());
    let delay = Delay::new();

    #[cfg(all(feature = "esp32s3", not(feature = "devkitc-1-v1-1")))]
    let led_pin = p.GPIO48;
    #[cfg(all(feature = "esp32s3", feature = "devkitc-1-v1-1"))]
    let led_pin = p.GPIO38;
    #[cfg(feature = "esp32c3")]
    let led_pin = p.GPIO8;

    let Ok(rmt) = Rmt::new(p.RMT, RMT_FREQ) else {
        idle(&delay)
    };
    let Ok(mut led) = Ws2812::new(rmt.channel0, led_pin) else {
        idle(&delay)
    };

    let mut ind = Indicator::new(Pattern::Off, now_ms());
    let mut index = 0;
    let mut scene_start = now_ms();
    loop {
        let now = now_ms();
        if now.wrapping_sub(scene_start) >= SHOW[index].ms {
            index = (index + 1) % SHOW.len();
            scene_start = now;
        }
        let scene = &SHOW[index];
        // Asking for the pattern again and again is harmless: only a different one restarts.
        ind.set(scene.pattern, now);
        ind.set_intensity(scene.intensity);

        // A failed write means the LED driver disabled itself: stop, the firmware goes on without it.
        let Ok(wake) = ind.drive(now, |c| led.set(c)) else {
            idle(&delay)
        };

        // Sleep until the colour can change, or until the scene ends, whichever comes first. A
        // steady pattern has no wake-up of its own.
        let left = SHOW[index].ms.saturating_sub(now.wrapping_sub(scene_start));
        delay.delay_millis(wake.map_or(left, |w| w.min(left)).max(1));
    }
}

fn idle(delay: &Delay) -> ! {
    loop {
        delay.delay_millis(1000);
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
