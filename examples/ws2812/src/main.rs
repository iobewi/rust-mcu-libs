#![no_std]
#![no_main]

//! Shows red, green, blue, off, then a succession of colours on the board's RGB LED.
//!
//! Board wiring, chosen by feature (the driver holds no pin number):
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

use esp_hal::{delay::Delay, rmt::Rmt};
use iobewi_esp_ws2812::{RGB8, RMT_FREQ, Ws2812};

esp_bootloader_esp_idf::esp_app_desc!();

/// Low values: the on-board LEDs are bright.
const RED: RGB8 = RGB8::new(32, 0, 0);
const GREEN: RGB8 = RGB8::new(0, 32, 0);
const BLUE: RGB8 = RGB8::new(0, 0, 32);
const OFF: RGB8 = RGB8::new(0, 0, 0);

/// The colours of the second part of the show, then the loop restarts.
const SUCCESSION: [RGB8; 5] = [
    RGB8::new(32, 32, 0),  // yellow
    RGB8::new(0, 32, 32),  // cyan
    RGB8::new(32, 0, 32),  // magenta
    RGB8::new(32, 12, 0),  // orange
    RGB8::new(16, 16, 16), // white
];

const STEP_MS: u32 = 1000;

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

    // The caller owns the RMT and picks the channel; the clock must be the driver's.
    let Ok(rmt) = Rmt::new(p.RMT, RMT_FREQ) else {
        idle(&delay)
    };
    let Ok(mut led) = Ws2812::new(rmt.channel0, led_pin) else {
        idle(&delay)
    };

    loop {
        // A failed write disables the driver; the example then stops, as a firmware would go on
        // without its LED.
        for color in [RED, GREEN, BLUE, OFF] {
            show(&mut led, &delay, color);
        }
        for color in SUCCESSION {
            show(&mut led, &delay, color);
        }
        show(&mut led, &delay, OFF);
    }
}

fn show(led: &mut Ws2812<'_>, delay: &Delay, color: RGB8) {
    if led.set(color).is_err() {
        idle(delay);
    }
    delay.delay_millis(STEP_MS);
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
