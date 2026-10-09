#![cfg_attr(not(test), no_std)]

//! Portable Unix epoch clock state, independent of any network stack.
//!
//! The clock is absent until a time source (for example `iobewi-ntp`'s SNTP
//! task) records a first valid epoch with [`set_synced`]; afterwards [`now`]
//! extrapolates from Embassy's monotonic time. A later network failure in the
//! time source retains the last synchronized value.

use core::cell::RefCell;

use critical_section::Mutex;
use embassy_time::{with_timeout, Duration, Instant, Timer};

/// Keep 64-bit clock state valid also on targets without 64-bit atomics.
#[derive(Clone, Copy)]
struct Sync {
    epoch_at_sync_s: u64,
    mono_at_sync_us: u64,
}

static SYNC: Mutex<RefCell<Option<Sync>>> = Mutex::new(RefCell::new(None));

/// Whether a time source has recorded a valid epoch at least once since boot.
pub fn is_set() -> bool {
    critical_section::with(|cs| SYNC.borrow(cs).borrow().is_some())
}

/// Current Unix epoch seconds UTC, or `None` before the first valid sync.
pub fn now() -> Option<u64> {
    let sync = critical_section::with(|cs| *SYNC.borrow(cs).borrow())?;
    let elapsed_us = Instant::now().as_micros().saturating_sub(sync.mono_at_sync_us);
    Some(sync.epoch_at_sync_s + elapsed_us / 1_000_000)
}

/// Blocks until the first sync completes or `timeout` elapses. Returns
/// `true` immediately if already synced from an earlier call.
pub async fn wait(timeout: Duration) -> bool {
    let observed = with_timeout(timeout, async {
        while !is_set() {
            Timer::after_millis(50).await;
        }
    }).await.is_ok();
    observed || is_set()
}


/// Records `epoch_s` (Unix seconds UTC) as the current time, anchored to the
/// monotonic clock at the instant of the call.
pub fn set_synced(epoch_s: u64) {
    let sync = Sync { epoch_at_sync_s: epoch_s, mono_at_sync_us: Instant::now().as_micros() };
    critical_section::with(|cs| *SYNC.borrow(cs).borrow_mut() = Some(sync));
}

/// Broken-down UTC calendar time with C `struct tm` field conventions
/// (`mon` is 0-11, `year` counts from 1900, `wday` 0 = Sunday, `yday` 0-365),
/// so platform adapters can map it onto their own `tm` without re-deriving
/// the calendar. Purely temporal: no dependency on any TLS/crypto type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UtcTm {
    pub sec: i32,
    pub min: i32,
    pub hour: i32,
    pub mday: i32,
    pub mon: i32,
    pub year: i32,
    pub wday: i32,
    pub yday: i32,
}

/// Unix epoch seconds -> broken-down UTC time. `None` outside years 1970-9999.
pub fn epoch_to_utc_tm(epoch: u64) -> Option<UtcTm> {
    let days = i64::try_from(epoch / 86_400).ok()?;
    let secs = (epoch % 86_400) as i32;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as i32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as i32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    if !(1970..=9999).contains(&year) {
        return None;
    }
    let is_leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    const CUMULATIVE: [i32; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    let yday = CUMULATIVE[(month - 1) as usize] + day - 1 + i32::from(is_leap && month > 2);

    Some(UtcTm {
        sec: secs % 60,
        min: secs / 60 % 60,
        hour: secs / 3_600,
        mday: day,
        mon: month - 1,
        year: (year - 1900) as i32,
        wday: ((days + 4).rem_euclid(7)) as i32,
        yday,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_conversion_known_dates() {
        let t = epoch_to_utc_tm(0).unwrap();
        assert_eq!((t.year, t.mon, t.mday, t.wday, t.yday), (70, 0, 1, 4, 0));
        assert_eq!((t.hour, t.min, t.sec), (0, 0, 0));

        let t = epoch_to_utc_tm(1_709_164_800).unwrap(); // 2024-02-29 00:00:00 UTC
        assert_eq!((t.year, t.mon, t.mday, t.yday), (124, 1, 29, 59));

        let t = epoch_to_utc_tm(1_709_164_800 + 86_399).unwrap(); // 23:59:59 same day
        assert_eq!((t.hour, t.min, t.sec, t.mday), (23, 59, 59, 29));
    }

    #[test]
    fn leap_year_day_of_year_and_weekday() {
        let t = epoch_to_utc_tm(1_709_251_200).unwrap(); // 2024-03-01
        assert_eq!((t.mon, t.mday, t.yday, t.wday), (2, 1, 60, 5)); // Friday
        let t = epoch_to_utc_tm(978_220_800).unwrap(); // 2000-12-31 (leap year, yday 365)
        assert_eq!((t.year, t.mon, t.mday, t.yday), (100, 11, 31, 365));
    }

    #[test]
    fn upper_bound_is_year_9999() {
        let last = 253_402_300_799; // 9999-12-31 23:59:59
        let t = epoch_to_utc_tm(last).unwrap();
        assert_eq!((t.year, t.mon, t.mday), (9999 - 1900, 11, 31));
        assert!(epoch_to_utc_tm(last + 1).is_none());
        assert!(epoch_to_utc_tm(u64::MAX).is_none());
    }
}
