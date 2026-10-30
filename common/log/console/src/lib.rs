#![no_std]

//! Portable console contract. A console is a byte sink: this crate says how to select one
//! ([`ConsoleBackend`]), which one the platform's images share ([`PRIMARY`], [`STAMP`]) and how
//! to write text to it ([`Console`], [`Writer`]). It owns no hardware, no buffer, no policy and
//! no logger: `iobewi-log` consumes a console, a bootloader writes to one directly.

use core::fmt;

mod backend;
pub use backend::ConsoleBackend;

include!(concat!(env!("OUT_DIR"), "/console_config.rs"));

/// A byte sink. Writes are best effort and cannot fail: a console that is not there must not
/// turn a diagnostic into a second error. `&self`, so a panic handler can use it without owning
/// anything; an implementation must work before any runtime or allocator is up.
pub trait Console {
    fn write_bytes(&self, bytes: &[u8]);
    fn flush(&self);
}

/// [`core::fmt::Write`] over a [`Console`].
pub struct Writer<'a, C: Console + ?Sized>(pub &'a C);

impl<C: Console + ?Sized> fmt::Write for Writer<'_, C> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.0.write_bytes(s.as_bytes());
        Ok(())
    }
}

/// A fixed line buffer: a message is formatted first and written in one go, so concurrent
/// writers interleave whole lines (or whole chunks of one) instead of formatting fragments.
/// A line longer than `N` is cut and ends with `...`; it never fails and never allocates.
pub struct LineBuf<const N: usize> {
    buf: [u8; N],
    len: usize,
    cut: bool,
}

impl<const N: usize> LineBuf<N> {
    pub const fn new() -> Self {
        Self {
            buf: [0; N],
            len: 0,
            cut: false,
        }
    }

    /// The line so far; a cut line ends with `...`.
    pub fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }

    /// Ends the line: `\n`, or `...\n` over the tail when the line was cut. Needs `N >= 4`.
    pub fn finish(&mut self) {
        let keep = if self.cut {
            N.saturating_sub(4)
        } else {
            N.saturating_sub(1)
        };
        if self.len > keep {
            self.len = keep;
        }
        if self.cut {
            self.buf[self.len..self.len + 3].copy_from_slice(b"...");
            self.len += 3;
        }
        self.buf[self.len] = b'\n';
        self.len += 1;
    }
}

impl<const N: usize> Default for LineBuf<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> fmt::Write for LineBuf<N> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        // Keep room for the `...` and the line end.
        let room = N.saturating_sub(4).saturating_sub(self.len);
        let take = s.len().min(room);
        self.buf[self.len..self.len + take].copy_from_slice(&s.as_bytes()[..take]);
        self.len += take;
        if take < s.len() {
            self.cut = true;
        }
        Ok(())
    }
}

/// Writes `LEVEL - message` (no line end) for a `log` record.
#[cfg(feature = "log")]
pub fn write_record(out: &mut impl fmt::Write, record: &log::Record<'_>) -> fmt::Result {
    write!(out, "{} - {}", record.level(), record.args())
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::{cell::RefCell, fmt::Write as _};

    struct Capture(RefCell<[u8; 64]>, RefCell<usize>);

    impl Console for Capture {
        fn write_bytes(&self, bytes: &[u8]) {
            let mut n = self.1.borrow_mut();
            self.0.borrow_mut()[*n..*n + bytes.len()].copy_from_slice(bytes);
            *n += bytes.len();
        }
        fn flush(&self) {}
    }

    #[test]
    fn line_buf_ends_a_line() {
        let mut l = LineBuf::<16>::new();
        write!(l, "A {}", 1).unwrap();
        l.finish();
        assert_eq!(l.as_bytes(), b"A 1\n");
    }

    #[test]
    fn line_buf_cuts_a_long_line_and_says_so() {
        let mut l = LineBuf::<16>::new();
        write!(l, "{}", "x".repeat(40)).unwrap();
        l.finish();
        assert_eq!(l.as_bytes(), b"xxxxxxxxxxxx...\n");
        assert_eq!(l.as_bytes().len(), 16);
        // an exact fit is not a cut
        let mut l = LineBuf::<16>::new();
        write!(l, "{}", "y".repeat(12)).unwrap();
        l.finish();
        assert_eq!(l.as_bytes(), b"yyyyyyyyyyyy\n");
    }

    #[test]
    fn names_round_trip() {
        assert_eq!(
            ConsoleBackend::from_name("none"),
            Some(ConsoleBackend::None)
        );
        assert_eq!(
            ConsoleBackend::from_name("usb-serial-jtag"),
            Some(ConsoleBackend::UsbSerialJtag)
        );
        assert_eq!(
            ConsoleBackend::from_name("uart0"),
            Some(ConsoleBackend::Uart(0))
        );
        assert_eq!(
            ConsoleBackend::from_name("uart12"),
            Some(ConsoleBackend::Uart(12))
        );
    }

    #[test]
    fn one_spelling_per_console() {
        for bad in [
            "", "uart", "uart01", "uart+1", "uart-1", "UART0", "uart256", "jtag", "None", " none",
        ] {
            assert_eq!(ConsoleBackend::from_name(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn same_is_exact() {
        assert!(ConsoleBackend::Uart(1).same(ConsoleBackend::Uart(1)));
        assert!(!ConsoleBackend::Uart(0).same(ConsoleBackend::Uart(1)));
        assert!(!ConsoleBackend::None.same(ConsoleBackend::UsbSerialJtag));
    }

    #[test]
    fn default_primary_is_uart0_and_stamped() {
        // Holds unless the test run itself sets IOBEWI_CONSOLE_PRIMARY.
        if option_env!("IOBEWI_CONSOLE_PRIMARY").is_none() {
            assert!(PRIMARY.same(ConsoleBackend::Uart(0)));
            assert_eq!(&STAMP[..], b"IOBEWI-CONSOLE primary=uart0\0");
        }
        assert_eq!(STAMP[STAMP.len() - 1], 0);
    }

    #[test]
    fn writer_forwards_formatted_text() {
        let console = Capture(RefCell::new([0; 64]), RefCell::new(0));
        write!(Writer(&console), "boot {:#x}", 255).unwrap();
        let n = *console.1.borrow();
        assert_eq!(&console.0.borrow()[..n], b"boot 0xff");
    }
}
