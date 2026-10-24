#![cfg_attr(not(test), no_std)]
#![doc = include_str!("../README.md")]

#[cfg(all(feature = "esp32c3", feature = "esp32s3"))]
compile_error!("select exactly one ESP32 target feature");

pub use smart_leds_trait::RGB8;
use smart_leds_trait::SmartLedsWrite;

/// Why a colour could not be shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteError {
    /// The transmission failed. The driver is now disabled.
    Transmit,
    /// An earlier write failed: the hardware is not touched any more.
    Disabled,
}

/// Disables the writer after its first error.
///
/// `esp-hal-smartled` 0.18 takes its RMT channel out of the driver for each blocking transmission
/// and cannot always put it back when the transmission fails to start; the next write would then
/// panic on a missing channel. Once any write has failed, no further write reaches the writer.
///
/// Only the chip features use it; the host build keeps it for its tests.
#[cfg_attr(not(any(feature = "esp32c3", feature = "esp32s3")), allow(dead_code))]
struct Guard<W> {
    writer: W,
    disabled: bool,
}

#[cfg_attr(not(any(feature = "esp32c3", feature = "esp32s3")), allow(dead_code))]
impl<W: SmartLedsWrite<Color = RGB8>> Guard<W> {
    const fn new(writer: W) -> Self {
        Self {
            writer,
            disabled: false,
        }
    }

    fn set(&mut self, color: RGB8) -> Result<(), WriteError> {
        if self.disabled {
            return Err(WriteError::Disabled);
        }
        self.writer.write(core::iter::once(color)).map_err(|_| {
            self.disabled = true;
            WriteError::Transmit
        })
    }
}

#[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
mod rmt {
    use esp_hal::{
        Blocking, gpio::interconnect::PeripheralOutput, rmt::TxChannelCreator, time::Rate,
    };
    use esp_hal_smartled::{RmtSmartLeds, WS2812_TIMING, buffer_size, color_order};

    use super::{Guard, RGB8, WriteError};

    /// RMT source clock the driver is built for: the caller must create its `Rmt` with this rate.
    ///
    /// The pulse widths are converted to ticks from it, so another rate gives wrong timings (the
    /// driver cannot read it back from the channel).
    pub const RMT_FREQ: Rate = Rate::from_mhz(80);

    /// Error returned when the driver cannot be created.
    pub type InitError = esp_hal_smartled::Error;

    const BUFFER: usize = buffer_size::<RGB8>(1);

    /// One WS2812 LED (GRB, `WS2812_TIMING`) driven by a blocking RMT transmission.
    ///
    /// The first failed write disables the driver; see [`WriteError`].
    pub struct Ws2812<'d> {
        guard: Guard<RmtSmartLeds<'d, BUFFER, Blocking, RGB8, color_order::Grb>>,
    }

    impl<'d> Ws2812<'d> {
        /// Takes a TX channel of an `Rmt` created at [`RMT_FREQ`] and the LED data pin.
        pub fn new<Ch, P>(channel: Ch, pin: P) -> Result<Self, InitError>
        where
            Ch: TxChannelCreator<'d, Blocking>,
            P: PeripheralOutput<'d>,
        {
            let led = RmtSmartLeds::new(WS2812_TIMING, channel, pin, RMT_FREQ)?;
            Ok(Self {
                guard: Guard::new(led),
            })
        }

        /// Shows `color`, as sent: no gamma, no brightness scaling.
        pub fn set(&mut self, color: RGB8) -> Result<(), WriteError> {
            self.guard.set(color)
        }

        /// Turns the LED off.
        pub fn off(&mut self) -> Result<(), WriteError> {
            self.set(RGB8::default())
        }

        /// True once a write has failed.
        pub fn is_disabled(&self) -> bool {
            self.guard.disabled
        }
    }
}

#[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
pub use rmt::{InitError, RMT_FREQ, Ws2812};

#[cfg(test)]
mod tests {
    use super::*;

    /// Records what reaches the writer and fails on demand.
    struct Fake {
        writes: usize,
        last: Option<RGB8>,
        fail: bool,
    }

    impl SmartLedsWrite for Fake {
        type Error = ();
        type Color = RGB8;

        fn write<T, I>(&mut self, iterator: T) -> Result<(), ()>
        where
            T: IntoIterator<Item = I>,
            I: Into<RGB8>,
        {
            self.writes += 1;
            self.last = iterator.into_iter().next().map(Into::into);
            if self.fail { Err(()) } else { Ok(()) }
        }
    }

    fn fake(fail: bool) -> Guard<Fake> {
        Guard::new(Fake {
            writes: 0,
            last: None,
            fail,
        })
    }

    const RED: RGB8 = RGB8 { r: 9, g: 0, b: 0 };

    #[test]
    fn passes_one_colour_through() {
        let mut guard = fake(false);
        assert_eq!(guard.set(RED), Ok(()));
        assert_eq!(guard.writer.last, Some(RED));
        assert_eq!(guard.writer.writes, 1);
        assert!(!guard.disabled);
    }

    #[test]
    fn keeps_writing_while_everything_works() {
        let mut guard = fake(false);
        for _ in 0..3 {
            assert_eq!(guard.set(RED), Ok(()));
        }
        assert_eq!(guard.writer.writes, 3);
    }

    #[test]
    fn first_error_is_reported_and_disables() {
        let mut guard = fake(true);
        assert_eq!(guard.set(RED), Err(WriteError::Transmit));
        assert!(guard.disabled);
    }

    #[test]
    fn disabled_guard_never_reaches_the_writer() {
        let mut guard = fake(true);
        assert_eq!(guard.set(RED), Err(WriteError::Transmit));
        // The writer would succeed now, but must not even be called.
        guard.writer.fail = false;
        for _ in 0..3 {
            assert_eq!(guard.set(RED), Err(WriteError::Disabled));
        }
        assert_eq!(guard.writer.writes, 1);
    }
}
