#![no_std]

//! Low-level ESP TIMG0 watchdog access for an application boot window.
//!
//! The caller decides whether to arm, feed or disable it and owns the
//! timeout. This crate has no knowledge of OTA or self-checks.
//! `esp_hal::init()` disables watchdogs. On the current runtime, TIMG0 must
//! be initialized for `esp_rtos` before calling `arm_ms`: constructing that
//! timer group later resets its peripheral block and clears the watchdog.

use esp_hal::peripherals::TIMG0;
use esp_hal::time::Duration;
use esp_hal::timer::timg::{MwdtStage, Wdt};

fn watchdog() -> Wdt<TIMG0<'static>> {
    Wdt::new()
}

/// Arm the first hardware watchdog stage with an application-supplied limit.
pub fn arm_ms(timeout_ms: u64) {
    let mut wdt = watchdog();
    wdt.set_timeout(MwdtStage::Stage0, Duration::from_millis(timeout_ms));
    wdt.enable();
}

/// Reset the counter while the application boot check is still pending.
pub fn feed() {
    watchdog().feed();
}

/// Stop the watchdog once the application no longer needs boot protection.
pub fn disable() {
    watchdog().disable();
}
