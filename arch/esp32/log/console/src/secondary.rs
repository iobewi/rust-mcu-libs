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

impl Console for SecondaryUart<'_> {
    fn write_bytes(&self, mut bytes: &[u8]) {
        critical_section::with(|cs| {
            let mut tx = self.0.borrow_ref_mut(cs);
            while !bytes.is_empty() {
                match tx.write(bytes) {
                    Ok(n) => bytes = &bytes[n..],
                    // Best effort: a broken secondary console must not stop the application.
                    Err(_) => return,
                }
            }
        });
    }

    fn flush(&self) {
        critical_section::with(|cs| {
            let _ = self.0.borrow_ref_mut(cs).flush();
        });
    }
}
