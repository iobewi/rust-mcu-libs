# Indicator

`iobewi-indicator` computes what one LED shows: a steady colour, a blink, a pulse or nothing. It
is a small engine, not a driver. It has no clock, no timer, no task, no hardware and no
allocation, and it knows nothing of statuses or events. The caller says which pattern and when it
is, and gets back the colour to show and how long it stays valid.

Colours are `RGB8` from `smart-leds-trait`, the type the `iobewi-esp-ws2812` driver already uses.

```rust,ignore
use iobewi_indicator::{Indicator, Pattern, RGB8};

let mut ind = Indicator::new(Pattern::Off, now_ms());
ind.set_intensity(32);                       // master level, 255 by default
ind.set(Pattern::Blink { color: RGB8::new(0, 255, 0), on_ms: 150, off_ms: 150 }, now_ms());

loop {
    // Writes only when the colour changed; returns the time to the next change.
    let wake = ind.drive(now_ms(), |c| led.set(c))?;
    // Wait `wake` ms, or for the next `set` if it is `None`, then loop.
}
```

## Patterns

| Pattern | Shows |
| --- | --- |
| `Off` | dark |
| `Solid(color)` | the colour, steady |
| `Blink { color, on_ms, off_ms }` | lit for `on_ms`, dark for `off_ms`, starting lit. `on_ms == 0` is `Off`, `off_ms == 0` is `Solid` |
| `Pulse { color, period_ms }` | fades from full to dark and back over `period_ms`, starting at full, in 32 steps per period. `period_ms == 0` is `Solid` |

Colours are given at full value. `set_intensity(level)` dims every colour (255 full, 0 dark) and
combines with the pulse level; it does not touch the phase.

## Time

The caller passes `now` in milliseconds (`u32`). The engine never reads a clock.

- **Stable origin.** A pattern keeps the time at which it began for as long as it lasts. Nothing
  moves it.
- **A different pattern restarts.** `set` with another pattern begins it at `now` (a blink lit, a
  pulse at full). `set` with the same pattern changes nothing and does not restart it, so a
  consumer may repeat its request freely.
- **`poll(now)`** is read-only. It returns the colour to transmit, only if it differs from the last
  colour confirmed as transmitted, and `wake_in`: the milliseconds, never less than 1, until the
  colour can next change. `wake_in` is `None` for a steady pattern: wait for the next `set` or
  `set_intensity`, there is nothing to poll. The engine wakes nobody; after `set` or
  `set_intensity` the caller polls again.
- **Not every wake changes the colour.** A pulse wakes at each of its 32 steps; under a low
  intensity several steps give the same colour, and `poll` then returns no colour to write.

### Limits of the `u32` millisecond counter

The elapsed time of a pattern is `now.wrapping_sub(origin)`, exact modulo 2^32 ms.

- `now` may wrap at any time; a pattern that spans the wrap runs unbroken.
- `now` must not run backwards, and must not be earlier than the `now` given to the last `set`
  that changed the pattern.
- A periodic pattern keeps its phase for 2^32 ms = 49 days 17 hours 2 minutes 47.296 seconds after
  its origin. After that the elapsed time wraps to zero and the phase restarts once, unless the
  period divides 2^32. This is a visual glitch of at most one cycle, every 49.7 days, with no
  other effect. To avoid it, change to another pattern and back before then; asking for the same
  pattern again does not move the origin.
- Steady patterns do not depend on time.

Periods are `u16` milliseconds (at most 65.535 s per half-period for a blink, per period for a
pulse); the sums and products involved fit in `u32`.

## Writing to a device

`drive(now, write)` polls, calls `write(color)` if there is a colour to transmit, and records it
only if `write` returned `Ok`. If `write` fails, the error is returned, the colour stays
untransmitted, and the next `drive` offers it again. The engine never loops, retries or panics:
the caller decides whether to wait, retry or give up (the `iobewi-esp-ws2812` driver disables
itself on its first error, and then every write returns an error).

To write with something else than a closure, for instance an async driver, use `poll` and call
`confirm(color)` after the transmission succeeded.

Recipes, not code of this crate:

- `iobewi-esp-ws2812`: `ind.drive(now, |c| led.set(c))`.
- A `SmartLedsWrite`: `ind.drive(now, |c| w.write(core::iter::once(c)))`.
- A single-channel LED (PWM duty, on/off): `ind.drive(now, |c| pwm.set_duty(level(c)))`. `level`
  is the brightest channel of the colour; use white patterns. Blink and pulse need no change.

## Memory

About 20 bytes (`Indicator`), 8 bytes (`Pattern`). No allocation; the host tests print the exact
sizes.

## Not in this crate

Statuses, priorities between states, an event source, a task or a timer, the hardware, gamma
correction, sequences or fades between colours.

## Validation

Host unit tests (`cargo test -p iobewi-indicator`): steady colour and off, exact blink and pulse
transitions, every pulse period from 1 to 300 ms, change of pattern during an animation,
unnecessary writes, intensity, counter wrap, and failing devices.
[`examples/indicator`](../../examples/indicator) shows the patterns on the WS2812 LED.
