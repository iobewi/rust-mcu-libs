//! A secondary console on UART1. It never replaces the primary console: the application owns
//! both and decides what goes where.

use core::cell::RefCell;
use critical_section::Mutex;
use esp_hal::{
    Blocking,
    gpio::AnyPin,
    peripherals::UART1,
    uart::{Config, ConfigError, Uart},
};
use iobewi_console::Console;

/// Console on UART1. It takes the peripheral and the pin by value: the type system
/// guarantees nobody else holds them, and UART0 (the primary console) cannot be passed.
pub struct SecondaryUart<'d>(Mutex<RefCell<Uart<'d, Blocking>>>);

impl<'d> SecondaryUart<'d> {
    pub fn new(
        uart: UART1<'d>,
        tx: impl Into<AnyPin<'d>>,
        config: Config,
    ) -> Result<Self, ConfigError> {
        Ok(Self(Mutex::new(RefCell::new(
            Uart::new(uart, config)?.with_tx(tx.into()),
        ))))
    }
}

/// Polls without progress before a write gives up (a stuck UART must not hang the application).
const PATIENCE: u32 = 2_000_000;

impl Console for SecondaryUart<'_> {
    /// The critical section covers one non-blocking FIFO fill at a time; waiting for FIFO space
    /// happens outside it, so interrupts are masked for microseconds, not for a whole line.
    /// Bytes of two concurrent writers can interleave at FIFO-fill boundaries.
    fn write_bytes(&self, mut bytes: &[u8]) {
        let mut idle = 0;
        while !bytes.is_empty() {
            let wrote = critical_section::with(|cs| {
                let mut tx = self.0.borrow_ref_mut(cs);
                if tx.write_ready() {
                    // Ready: this write does not block.
                    tx.write(bytes).ok()
                } else {
                    Some(0)
                }
            });
            match wrote {
                // Best effort: a broken secondary console must not stop the application.
                None => return,
                Some(0) => {
                    idle += 1;
                    if idle > PATIENCE {
                        return;
                    }
                    core::hint::spin_loop();
                }
                Some(n) => {
                    idle = 0;
                    bytes = &bytes[n..];
                }
            }
        }
    }

    /// Nothing to do: `write_bytes` returns once the bytes are in the TX FIFO, and waiting for
    /// the FIFO to drain would hold interrupts off.
    fn flush(&self) {}
}
