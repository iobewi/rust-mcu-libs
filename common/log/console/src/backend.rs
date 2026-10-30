#![allow(dead_code)] // shared with build.rs, which uses a subset
//! Transport selection. Shared with `build.rs`, so the variable and the constant parse alike.

/// A physical console transport. What a given MCU supports is the architecture crate's concern.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsoleBackend {
    /// No console output through this connector.
    None,
    /// UART number `N`.
    Uart(u8),
    /// The chip's native USB Serial/JTAG device.
    UsbSerialJtag,
}

impl ConsoleBackend {
    /// `none`, `usb-serial-jtag` or `uart<N>`.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "none" => Some(Self::None),
            "usb-serial-jtag" => Some(Self::UsbSerialJtag),
            _ => {
                let digits = name.strip_prefix("uart")?;
                // No sign, no leading zero: one spelling per console.
                if digits.is_empty()
                    || !digits.bytes().all(|b| b.is_ascii_digit())
                    || (digits.len() > 1 && digits.starts_with('0'))
                {
                    return None;
                }
                digits.parse().ok().map(Self::Uart)
            }
        }
    }

    /// Equality usable in `const` assertions.
    pub const fn same(self, other: Self) -> bool {
        match (self, other) {
            (Self::None, Self::None) | (Self::UsbSerialJtag, Self::UsbSerialJtag) => true,
            (Self::Uart(a), Self::Uart(b)) => a == b,
            _ => false,
        }
    }
}
