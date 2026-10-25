use super::*;

const RED: RGB8 = RGB8::new(200, 100, 50);

/// A device that records what it is given and fails on demand.
struct Led {
    writes: std::vec::Vec<RGB8>,
    fail: bool,
}

impl Led {
    fn new() -> Self {
        Self {
            writes: std::vec::Vec::new(),
            fail: false,
        }
    }

    fn write(&mut self, c: RGB8) -> Result<(), &'static str> {
        if self.fail {
            return Err("device error");
        }
        self.writes.push(c);
        Ok(())
    }
}

fn blink(on_ms: u16, off_ms: u16) -> Pattern {
    Pattern::Blink {
        color: RED,
        on_ms,
        off_ms,
    }
}

fn pulse(period_ms: u16) -> Pattern {
    Pattern::Pulse {
        color: RGB8::new(255, 255, 255),
        period_ms,
    }
}

/// The colour the pattern calls for at `now`, whatever was transmitted before.
fn color_at(ind: &Indicator, now: Millis) -> RGB8 {
    let mut fresh = ind.clone();
    fresh.transmitted = None;
    fresh.poll(now).color.expect("nothing transmitted yet")
}

// 1. steady colour and off ---------------------------------------------------------------------

#[test]
fn solid_is_offered_once_then_nothing() {
    let mut ind = Indicator::new(Pattern::Solid(RED), 0);
    let mut led = Led::new();
    assert_eq!(ind.drive(0, |c| led.write(c)), Ok(None));
    assert_eq!(ind.drive(1, |c| led.write(c)), Ok(None));
    assert_eq!(ind.drive(100_000, |c| led.write(c)), Ok(None));
    assert_eq!(led.writes, [RED]);
    assert_eq!(ind.poll(5).wake_in, None);
}

#[test]
fn off_is_dark_and_still() {
    let mut ind = Indicator::new(Pattern::Solid(RED), 0);
    let mut led = Led::new();
    ind.drive(0, |c| led.write(c)).unwrap();
    ind.set(Pattern::Off, 10);
    assert_eq!(ind.drive(10, |c| led.write(c)), Ok(None));
    assert_eq!(led.writes, [RED, BLACK]);
}

#[test]
fn a_dark_pattern_at_start_is_still_transmitted_once() {
    // Nothing was transmitted yet: the LED may be lit by anything.
    let mut ind = Indicator::new(Pattern::Off, 0);
    let mut led = Led::new();
    ind.drive(0, |c| led.write(c)).unwrap();
    ind.drive(1, |c| led.write(c)).unwrap();
    assert_eq!(led.writes, [BLACK]);
}

// 2. blink ---------------------------------------------------------------------------------------

#[test]
fn blink_switches_exactly_on_the_boundaries() {
    let ind = Indicator::new(blink(100, 300), 1_000);
    let at = |t: u32| ind.poll(1_000 + t);
    assert_eq!(at(0).color, Some(RED));
    assert_eq!(at(0).wake_in, Some(100));
    assert_eq!(at(99).wake_in, Some(1));
    assert_eq!(color_at(&ind, 1_099), RED);
    assert_eq!(color_at(&ind, 1_100), BLACK);
    assert_eq!(at(100).wake_in, Some(300));
    assert_eq!(at(399).wake_in, Some(1));
    assert_eq!(color_at(&ind, 1_399), BLACK);
    assert_eq!(color_at(&ind, 1_400), RED);
    assert_eq!(at(400).wake_in, Some(100));
    assert_eq!(color_at(&ind, 1_000 + 400 * 1_000), RED);
}

#[test]
fn blink_without_a_half_period_is_a_steady_pattern() {
    assert_eq!(Indicator::new(blink(0, 500), 0).poll(7).color, Some(BLACK));
    assert_eq!(Indicator::new(blink(0, 500), 0).poll(7).wake_in, None);
    assert_eq!(Indicator::new(blink(0, 0), 0).poll(7).wake_in, None);
    assert_eq!(Indicator::new(blink(500, 0), 0).poll(7).color, Some(RED));
    assert_eq!(Indicator::new(blink(500, 0), 0).poll(7).wake_in, None);
}

#[test]
fn blink_wake_is_strictly_positive_and_lands_on_the_boundary() {
    for (on, off) in [
        (1, 1),
        (1, 7),
        (150, 150),
        (500, 1),
        (3, 1000),
        (65_535, 65_535),
    ] {
        let ind = Indicator::new(blink(on, off), 11);
        let cycle = u32::from(on) + u32::from(off);
        let mut now = 11u32;
        for _ in 0..8 {
            let wake = ind.poll(now).wake_in.expect("a blink always wakes");
            assert!(wake >= 1, "{on}/{off} at {now}");
            let before = color_at(&ind, now);
            // Nothing changes before the wake, and the colour flips exactly at it.
            assert_eq!(
                color_at(&ind, now + wake - 1),
                before,
                "{on}/{off} at {now}"
            );
            assert_ne!(color_at(&ind, now + wake), before, "{on}/{off} at {now}");
            now += wake;
        }
        assert!(now - 11 >= 4 * cycle - 1);
    }
}

#[test]
fn blink_longest_cycle_does_not_overflow() {
    let ind = Indicator::new(blink(u16::MAX, u16::MAX), 0);
    assert_eq!(color_at(&ind, 65_534), RED);
    assert_eq!(color_at(&ind, 65_535), BLACK);
    assert_eq!(ind.poll(131_069).wake_in, Some(1));
    assert_eq!(color_at(&ind, 131_070), RED);
}

// 3. pulse ---------------------------------------------------------------------------------------

#[test]
fn pulse_starts_at_full_goes_to_zero_and_comes_back() {
    let ind = Indicator::new(pulse(3_200), 0);
    // 100 ms per step.
    let level_at_step = |s: u32| color_at(&ind, s * 100).r;
    assert_eq!(level_at_step(0), 255);
    assert_eq!(level_at_step(16), 0);
    assert_eq!(level_at_step(32), 255);
    for s in 0..16 {
        assert!(level_at_step(s) > level_at_step(s + 1), "down at {s}");
    }
    for s in 16..32 {
        assert!(level_at_step(s) < level_at_step(s + 1), "up at {s}");
    }
    for s in 0..=16 {
        assert_eq!(level_at_step(s), level_at_step(32 - s), "symmetry at {s}");
    }
}

#[test]
fn pulse_is_constant_inside_a_step_and_wakes_at_the_next_one() {
    let ind = Indicator::new(pulse(3_200), 50);
    let first = ind.poll(50);
    assert_eq!(first.wake_in, Some(100));
    for t in 0..100 {
        assert_eq!(color_at(&ind, 50 + t), color_at(&ind, 50), "t={t}");
    }
    assert_ne!(color_at(&ind, 150), color_at(&ind, 50));
    assert_eq!(ind.poll(50 + 99).wake_in, Some(1));
}

#[test]
fn pulse_wake_is_strictly_positive_and_advances_a_step_for_every_period() {
    for period in 1..=300u16 {
        let p = u64::from(period);
        let ind = Indicator::new(pulse(period), 7);
        let end = 7 + 3 * u32::from(period);
        let mut now = 7u32;
        let mut wakes = 0;
        // Steps since the start of the pattern, counted over whole cycles.
        let mut seen = None;
        while now < end {
            let wake = ind.poll(now).wake_in.expect("a pulse always wakes");
            assert!(wake >= 1, "period {period} at {now}");
            let absolute = u64::from(now - 7) * 32 / p;
            assert!(
                seen.is_none_or(|s| absolute > s),
                "stuck at {now}, period {period}"
            );
            seen = Some(absolute);
            now += wake;
            wakes += 1;
        }
        // 32 steps per period at most, fewer when several fall in the same millisecond.
        assert!(wakes <= 3 * 32 + 1, "period {period}: {wakes} wakes");
    }
}

#[test]
fn pulse_cycles_exactly() {
    let ind = Indicator::new(pulse(2_000), 0);
    for now in [0, 1, 123, 999, 1_000, 1_500, 1_999] {
        assert_eq!(ind.poll(now), ind.poll(now + 2_000 * 7));
    }
}

#[test]
fn pulse_without_a_period_is_steady() {
    let ind = Indicator::new(pulse(0), 0);
    assert_eq!(ind.poll(9).color, Some(RGB8::new(255, 255, 255)));
    assert_eq!(ind.poll(9).wake_in, None);
}

#[test]
fn pulse_longest_period_does_not_overflow() {
    let ind = Indicator::new(pulse(u16::MAX), 0);
    assert_eq!(color_at(&ind, 0).r, 255);
    assert!(ind.poll(u32::from(u16::MAX) - 1).wake_in.unwrap() >= 1);
}

// 4. change of pattern ---------------------------------------------------------------------------

#[test]
fn a_different_pattern_restarts_the_phase() {
    let mut ind = Indicator::new(blink(100, 300), 0);
    assert_eq!(color_at(&ind, 250), BLACK);
    ind.set(blink(100, 200), 250);
    assert_eq!(color_at(&ind, 250), RED);
    assert_eq!(ind.poll(250).wake_in, Some(100));
    assert_eq!(color_at(&ind, 350), BLACK);
    assert_eq!(color_at(&ind, 550), RED);
}

#[test]
fn the_same_pattern_does_not_restart() {
    let mut ind = Indicator::new(blink(100, 300), 0);
    let before = ind.clone();
    ind.set(blink(100, 300), 250);
    assert_eq!(ind, before);
    assert_eq!(color_at(&ind, 250), BLACK);
    assert_eq!(ind.poll(250).wake_in, Some(150));
}

#[test]
fn changing_the_pattern_in_a_pulse_restarts_at_full() {
    let mut ind = Indicator::new(pulse(3_200), 0);
    assert!(color_at(&ind, 1_000).r < 255);
    ind.set(pulse(1_600), 1_000);
    assert_eq!(color_at(&ind, 1_000).r, 255);
}

#[test]
fn the_origin_is_stable_while_a_pattern_lasts() {
    let mut ind = Indicator::new(Pattern::Off, 0);
    ind.set(blink(100, 300), 40);
    let mut led = Led::new();
    // Many calls, at irregular times, drive the engine; the phase must not drift.
    for now in [40, 41, 90, 139, 140, 300, 439, 440, 441, 700, 7_001] {
        ind.drive(now, |c| led.write(c)).unwrap();
    }
    assert_eq!(color_at(&ind, 40 + 400 * 1_000 + 99), RED);
    assert_eq!(color_at(&ind, 40 + 400 * 1_000 + 100), BLACK);
}

#[test]
fn the_same_pattern_asked_again_and_again_keeps_the_animation_going() {
    let mut ind = Indicator::new(pulse(3_200), 0);
    let mut led = Led::new();
    for now in (0..=1_600).step_by(100) {
        ind.set(pulse(3_200), now);
        ind.drive(now, |c| led.write(c)).unwrap();
    }
    // Down to the dark middle of the pulse, not stuck at full.
    assert_eq!(led.writes.first().map(|c| c.r), Some(255));
    assert_eq!(led.writes.last().map(|c| c.r), Some(0));
}

// 5. unnecessary writes --------------------------------------------------------------------------

#[test]
fn poll_changes_nothing() {
    let ind = Indicator::new(blink(100, 300), 0);
    let before = ind.clone();
    let first = ind.poll(50);
    assert_eq!(ind.poll(50), first);
    assert_eq!(ind.poll(50), first);
    assert_eq!(ind, before);
}

#[test]
fn blink_writes_only_when_the_colour_changes() {
    let mut ind = Indicator::new(blink(100, 300), 0);
    let mut led = Led::new();
    for now in 0..2_000 {
        ind.drive(now, |c| led.write(c)).unwrap();
    }
    // lit at 0, dark at 100, lit at 400, ... five cycles in 2000 ms.
    assert_eq!(led.writes.len(), 1 + 2 * 4 + 1);
    assert!(led.writes.windows(2).all(|w| w[0] != w[1]));
}

#[test]
fn a_pulse_under_low_intensity_skips_the_steps_that_look_the_same() {
    let mut ind = Indicator::new(pulse(3_200), 0);
    ind.set_intensity(8);
    let mut led = Led::new();
    let mut calls = 0;
    let mut now = 0;
    while now < 3_200 {
        let wake = ind.drive(now, |c| led.write(c)).unwrap().unwrap();
        calls += 1;
        now += wake;
    }
    assert_eq!(calls, 32);
    assert!(led.writes.len() < 32);
    assert!(led.writes.windows(2).all(|w| w[0] != w[1]));
}

// 6. time limits ---------------------------------------------------------------------------------

#[test]
fn the_clock_may_wrap_during_a_pattern() {
    let since = u32::MAX - 150;
    let ind = Indicator::new(blink(100, 300), since);
    assert_eq!(color_at(&ind, since), RED);
    assert_eq!(color_at(&ind, u32::MAX), BLACK); // 150 ms in
    assert_eq!(ind.poll(u32::MAX).wake_in, Some(250));
    assert_eq!(color_at(&ind, 0), BLACK); // 151 ms in
    // 399 ms in is dark, 400 ms in is lit again, across the wrap.
    assert_eq!(color_at(&ind, 248), BLACK);
    assert_eq!(color_at(&ind, 249), RED);
}

#[test]
fn the_phase_is_exact_up_to_2_pow_32_ms_then_restarts() {
    // A period that does not divide 2^32: past 2^32 ms the elapsed time wraps, so the phase
    // restarts once. This is the documented limit.
    let ind = Indicator::new(blink(100, 900), 0);
    let last = u32::MAX; // 2^32 - 1 ms in: phase 295 (dark)
    assert_eq!(u64::from(last) % 1_000, 295);
    assert_eq!(color_at(&ind, last), BLACK);
    // 2^32 ms in is read as 0 ms in: lit.
    assert_eq!(color_at(&ind, last.wrapping_add(1)), RED);
}

// 7. device errors -------------------------------------------------------------------------------

#[test]
fn a_failed_write_leaves_the_colour_untransmitted_and_it_comes_back() {
    let mut ind = Indicator::new(Pattern::Solid(RED), 0);
    let mut led = Led::new();
    led.fail = true;
    assert_eq!(ind.drive(0, |c| led.write(c)), Err("device error"));
    assert_eq!(ind.poll(0).color, Some(RED));
    assert_eq!(ind.drive(5, |c| led.write(c)), Err("device error"));
    led.fail = false;
    assert_eq!(ind.drive(9, |c| led.write(c)), Ok(None));
    assert_eq!(led.writes, [RED]);
    assert_eq!(ind.poll(10).color, None);
}

#[test]
fn a_failed_write_does_not_forget_the_previous_colour() {
    let mut ind = Indicator::new(blink(100, 100), 0);
    let mut led = Led::new();
    ind.drive(0, |c| led.write(c)).unwrap(); // lit, transmitted
    led.fail = true;
    assert!(ind.drive(100, |c| led.write(c)).is_err()); // dark, not transmitted
    // Back to lit before the device recovers: the LED really shows lit, so nothing to send.
    assert_eq!(ind.poll(200).color, None);
    assert_eq!(led.writes, [RED]);
}

#[test]
fn a_device_that_always_fails_is_asked_once_per_call_and_the_engine_does_not_panic() {
    let mut ind = Indicator::new(pulse(1_000), 0);
    let mut asked = 0;
    for now in 0..50 {
        let r: Result<_, ()> = ind.drive(now * 10, |_| {
            asked += 1;
            Err(())
        });
        assert!(r.is_err());
    }
    assert_eq!(asked, 50);
}

#[test]
fn confirm_records_a_transmission_made_by_the_caller() {
    let mut ind = Indicator::new(Pattern::Solid(RED), 0);
    let color = ind.poll(0).color.unwrap();
    ind.confirm(color);
    assert_eq!(ind.poll(0).color, None);
}

// 8. intensity -----------------------------------------------------------------------------------

#[test]
fn scale_is_the_smart_leds_brightness_arithmetic() {
    for c in 0..=255u32 {
        for b in 0..=255u8 {
            let expected = (c * (u32::from(b) + 1) / 256) as u8;
            assert_eq!(scale(RGB8::new(c as u8, 0, 0), b).r, expected);
        }
    }
    assert_eq!(scale(RED, 255), RED);
    assert_eq!(scale(RED, 0), BLACK);
}

#[test]
fn intensity_dims_every_channel() {
    let mut ind = Indicator::new(Pattern::Solid(RGB8::new(255, 128, 4)), 0);
    assert_eq!(ind.poll(0).color, Some(RGB8::new(255, 128, 4)));
    ind.set_intensity(0);
    assert_eq!(ind.poll(0).color, Some(BLACK));
    ind.set_intensity(127);
    assert_eq!(ind.poll(0).color, Some(RGB8::new(127, 64, 2)));
}

#[test]
fn changing_the_intensity_sends_one_new_colour_and_keeps_the_phase() {
    let mut ind = Indicator::new(blink(100, 300), 0);
    let mut led = Led::new();
    ind.drive(0, |c| led.write(c)).unwrap();
    ind.set_intensity(51);
    ind.drive(10, |c| led.write(c)).unwrap();
    ind.drive(20, |c| led.write(c)).unwrap();
    assert_eq!(led.writes, [RED, scale(RED, 51)]);
    assert_eq!(ind.poll(20).wake_in, Some(80));
}

#[test]
fn intensity_combines_with_the_pulse_level() {
    let mut ind = Indicator::new(pulse(3_200), 0);
    ind.set_intensity(128);
    assert_eq!(color_at(&ind, 0).r, 128);
    assert_eq!(color_at(&ind, 1_600).r, 0);
}

// level and size ---------------------------------------------------------------------------------

#[test]
fn level_is_the_brightest_channel() {
    assert_eq!(level(BLACK), 0);
    assert_eq!(level(RGB8::new(10, 20, 30)), 30);
    assert_eq!(level(RGB8::new(40, 20, 30)), 40);
    assert_eq!(level(RGB8::new(10, 50, 30)), 50);
    assert_eq!(level(RGB8::new(255, 255, 255)), 255);
}

#[test]
fn the_engine_stays_small() {
    println!(
        "size_of Indicator={} Pattern={} Frame={}",
        size_of::<Indicator>(),
        size_of::<Pattern>(),
        size_of::<Frame>()
    );
    assert!(size_of::<Indicator>() <= 24);
}
