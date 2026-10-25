#![cfg_attr(not(test), no_std)]
#![doc = include_str!("../README.md")]

pub use smart_leds_trait::RGB8;

/// Milliseconds on the caller's clock. The counter wraps; see the crate documentation for the
/// exact limits.
pub type Millis = u32;

/// Steps in one period of a [`Pattern::Pulse`]: half going down, half coming back up.
const PULSE_STEPS: u32 = 32;

const BLACK: RGB8 = RGB8::new(0, 0, 0);

/// What the LED should do. Colours are given at full value; [`Indicator::set_intensity`] dims
/// them all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pattern {
    /// Dark.
    Off,
    /// A steady colour.
    Solid(RGB8),
    /// `color` for `on_ms`, then dark for `off_ms`, starting lit. `on_ms == 0` is [`Pattern::Off`],
    /// `off_ms == 0` is [`Pattern::Solid`].
    Blink {
        color: RGB8,
        on_ms: u16,
        off_ms: u16,
    },
    /// `color` fading from full to dark and back over `period_ms`, starting at full, in 32 steps
    /// per period. `period_ms == 0` is [`Pattern::Solid`].
    Pulse { color: RGB8, period_ms: u16 },
}

/// The result of [`Indicator::poll`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    /// The colour to transmit, only if it differs from the last colour confirmed as transmitted.
    pub color: Option<RGB8>,
    /// Milliseconds, always at least 1, until the pattern can next change the colour. `None` for a
    /// steady pattern: nothing changes until the next [`Indicator::set`] or
    /// [`Indicator::set_intensity`].
    pub wake_in: Option<Millis>,
}

/// Computes what one LED shows, from a time given by the caller.
///
/// It owns no clock, no timer, no hardware and no allocation; it is about 20 bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Indicator {
    pattern: Pattern,
    /// Time at which the current pattern began. Never moved while the pattern lasts.
    since: Millis,
    intensity: u8,
    /// Last colour confirmed as transmitted, `None` before the first one.
    transmitted: Option<RGB8>,
}

impl Indicator {
    /// Starts showing `pattern` at time `now`, at full intensity, with nothing transmitted yet.
    pub const fn new(pattern: Pattern, now: Millis) -> Self {
        Self {
            pattern,
            since: now,
            intensity: 255,
            transmitted: None,
        }
    }

    /// The pattern being shown.
    pub const fn pattern(&self) -> Pattern {
        self.pattern
    }

    /// Shows `pattern` from `now`.
    ///
    /// A pattern different from the current one restarts at its beginning (a blink lit, a pulse at
    /// full). The same pattern changes nothing and does not restart: it is safe to ask again and
    /// again. Poll again after a change: the engine wakes nobody.
    pub fn set(&mut self, pattern: Pattern, now: Millis) {
        if pattern != self.pattern {
            self.pattern = pattern;
            self.since = now;
        }
    }

    /// Sets the master intensity applied to every colour: 255 is full, 0 is dark. The phase of the
    /// pattern is not touched. Poll again after a change.
    pub fn set_intensity(&mut self, level: u8) {
        self.intensity = level;
    }

    /// What to show at `now`. It changes nothing: asking twice gives the same answer until
    /// [`Indicator::confirm`] (or a successful [`Indicator::drive`]) records a transmission.
    ///
    /// `now` must not run backwards and must not be earlier than the `now` of the last change of
    /// pattern.
    pub fn poll(&self, now: Millis) -> Frame {
        let (color, level, wake_in) = self.sample(now);
        let color = scale(color, mul(level, self.intensity));
        Frame {
            color: (self.transmitted != Some(color)).then_some(color),
            wake_in,
        }
    }

    /// Records that `color` (from [`Frame::color`]) reached the LED. Call it only after the
    /// transmission succeeded; after a failure do not call it, and the same colour comes back from
    /// the next [`Indicator::poll`].
    pub fn confirm(&mut self, color: RGB8) {
        self.transmitted = Some(color);
    }

    /// [`Indicator::poll`], then `write` if the colour changed, then [`Indicator::confirm`] if
    /// `write` succeeded. Returns the time until the next change, as in [`Frame::wake_in`].
    ///
    /// When `write` fails its error is returned, the colour stays untransmitted and the next call
    /// offers it again. It is up to the caller to retry, wait or give up: a device that keeps
    /// failing is not hammered by the engine, which never loops.
    pub fn drive<E>(
        &mut self,
        now: Millis,
        mut write: impl FnMut(RGB8) -> Result<(), E>,
    ) -> Result<Option<Millis>, E> {
        let frame = self.poll(now);
        if let Some(color) = frame.color {
            write(color)?;
            self.confirm(color);
        }
        Ok(frame.wake_in)
    }

    /// Colour at full value, pulse level (255 outside a pulse) and the time to the next change.
    fn sample(&self, now: Millis) -> (RGB8, u8, Option<Millis>) {
        // Exact modulo 2^32: see the crate documentation for what that means past 2^32 ms.
        let elapsed = now.wrapping_sub(self.since);
        match self.pattern {
            Pattern::Off | Pattern::Blink { on_ms: 0, .. } => (BLACK, 255, None),
            Pattern::Solid(color)
            | Pattern::Blink {
                color, off_ms: 0, ..
            }
            | Pattern::Pulse {
                color,
                period_ms: 0,
            } => (color, 255, None),
            Pattern::Blink {
                color,
                on_ms,
                off_ms,
            } => {
                let on = u32::from(on_ms);
                let cycle = on + u32::from(off_ms);
                let phase = elapsed % cycle;
                if phase < on {
                    (color, 255, Some(on - phase))
                } else {
                    (BLACK, 255, Some(cycle - phase))
                }
            }
            Pattern::Pulse { color, period_ms } => {
                let period = u32::from(period_ms);
                let phase = elapsed % period;
                let step = phase * PULSE_STEPS / period;
                // Smallest time at which the step index reaches step + 1: strictly after `phase`.
                let next = ((step + 1) * period).div_ceil(PULSE_STEPS);
                (color, pulse_level(step), Some(next - phase))
            }
        }
    }
}

/// Luminance of one step of the pulse: 255 at step 0, 0 at the middle step, back up after.
fn pulse_level(step: u32) -> u8 {
    let half = PULSE_STEPS / 2;
    let distance = if step < half {
        step
    } else {
        PULSE_STEPS - step
    };
    (255 - distance * 255 / half) as u8
}

/// `a` and `b` as fractions of 255, multiplied.
fn mul(a: u8, b: u8) -> u8 {
    (u32::from(a) * u32::from(b) / 255) as u8
}

/// Scales every channel by `level` / 255: unchanged at 255, dark at 0 (the arithmetic of
/// `smart_leds::brightness`).
fn scale(color: RGB8, level: u8) -> RGB8 {
    let k = u32::from(level) + 1;
    RGB8::new(
        (u32::from(color.r) * k / 256) as u8,
        (u32::from(color.g) * k / 256) as u8,
        (u32::from(color.b) * k / 256) as u8,
    )
}

/// Level for a single-channel LED (PWM duty, on/off threshold): the brightest channel of `color`.
/// Use white patterns on such a LED.
pub const fn level(color: RGB8) -> u8 {
    let rg = if color.r > color.g { color.r } else { color.g };
    if rg > color.b { rg } else { color.b }
}

#[cfg(test)]
mod tests;
