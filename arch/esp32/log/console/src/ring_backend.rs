//! The application's primary console on UART0 with a transmit ring (feature `ring`).
//!
//! `console_print` deposits a whole line in a 2 KiB ring and returns: no producer ever waits for
//! the UART, and the UART is never fed from inside a long critical section. The ring is emptied
//! by the producer that deposited (one non-blocking FIFO fill) and then by the UART's `TxDone`
//! interrupt, so a message accepted by the ring leaves even if nothing else is ever logged.
//!
//! Three guarantees, three different things:
//!
//! * **deposit**: `write_bytes` returned: the message is in the ring, whole, or it was dropped
//!   whole and counted ([`lost_messages`]); a `WARN - console: N message(s) dropped` line follows
//!   the first deposit that fits again.
//! * **transfer to the FIFO**: happens without any further call, in order, as fast as the UART
//!   takes it, provided the UART interrupt is not masked for long.
//! * **physical transmission**: [`flush`] returns `true` once the ring is empty and the UART said
//!   `TxDone`; it waits (bounded) and is for thread context.
//!
//! The panic path ([`emergency`]) never takes the critical section: see `iobewi_console::ring`.

use core::{
    cell::RefCell,
    fmt::Write as _,
    sync::atomic::{AtomicBool, Ordering::*},
};
use critical_section::Mutex;
use esp_hal::{
    Blocking,
    gpio::AnyPin,
    handler,
    peripherals::{Interrupt, UART0},
    system::Cpu,
    time::Instant,
    uart::{Config, ConfigError, Uart, UartInterrupt},
};
use iobewi_console::{
    LineBuf,
    ring::{Claim, Deposit, FreezeError, Platform, Pump, Ring},
};

/// Size of the transmit ring.
pub const RING_BYTES: usize = 2048;
/// Longest fatal line.
const LINE: usize = 256;
/// How long the panic path waits for a confirmed freeze.
const FREEZE_US: u64 = 2_000;
/// How long two simultaneous panics wait for each other's line.
const SECOND_WAIT_US: u64 = 25_000;
/// Safety net when the timer does not run (a panic before `esp_hal::init`): not a time bound.
const SPIN_CAP: u32 = 2_000_000;

struct Section;

impl Platform for Section {
    fn with<R>(&self, f: impl FnOnce() -> R) -> R {
        critical_section::with(|_| {
            #[cfg(feature = "ring-stats")]
            {
                let t0 = stats::ticks();
                let r = f();
                stats::note(&stats::HOLD, stats::ticks().wrapping_sub(t0));
                r
            }
            #[cfg(not(feature = "ring-stats"))]
            f()
        })
    }

    fn core(&self) -> u32 {
        Cpu::current() as u32
    }
}

static RING: Ring<RING_BYTES, Section> = Ring::new(Section);
static UART: Mutex<RefCell<Option<Uart<'static, Blocking>>>> = Mutex::new(RefCell::new(None));
static INSTALLED: AtomicBool = AtomicBool::new(false);
/// Nothing left in the FIFO: the last byte written has been sent (set by `TxDone`).
static IDLE: AtomicBool = AtomicBool::new(true);

/// Takes UART0 and its TX pin, configures them and starts the transmit ring.
///
/// Call it once, at start-up, on the core that must service the UART interrupt (the handler is
/// bound to the current core), before other tasks or the other core log. Until it is called,
/// and in the bootloader, the console is the direct synchronous one. `config` must use the
/// baud rate of the platform's console (the ROM console is 115200 baud).
pub fn install(
    uart: UART0<'static>,
    tx: impl Into<AnyPin<'static>>,
    config: Config,
) -> Result<(), ConfigError> {
    // Let the ROM console finish what it is sending before the driver resets the FIFO.
    esp_println::Printer::write_bytes(&[]);
    let mut uart = Uart::new(uart, config)?.with_tx(tx.into());
    uart.set_interrupt_handler(uart0_isr);
    uart.clear_interrupts(UartInterrupt::TxDone.into());
    uart.unlisten(UartInterrupt::TxDone);
    critical_section::with(|cs| UART.borrow_ref_mut(cs).replace(uart));
    IDLE.store(true, Release);
    INSTALLED.store(true, Release);
    Ok(())
}

/// Whether [`install`] ran.
pub fn installed() -> bool {
    INSTALLED.load(Acquire)
}

/// Messages dropped because the ring was full.
pub fn lost_messages() -> u32 {
    RING.lost_total()
}

/// Bytes deposited and not yet in the FIFO (advisory).
pub fn pending_bytes() -> usize {
    RING.pending()
}

/// Deposits one message. Called by `Primary::write_bytes`.
pub(crate) fn write(bytes: &[u8]) {
    if RING.deposit(bytes) == Deposit::Accepted {
        service(false);
    }
}

/// Waits (at most `timeout_us`) until everything deposited has been transmitted: the ring is
/// empty and the UART reported `TxDone`. `true` if so. For thread context; it helps the
/// interrupt, so it also works while the UART interrupt is masked. Nothing to wait for before
/// [`install`], where writes are synchronous.
pub fn flush(timeout_us: u32) -> bool {
    if !installed() {
        return true;
    }
    let start = Instant::now();
    loop {
        service(false);
        if RING.pending() == 0 && IDLE.load(Acquire) {
            return true;
        }
        let late = Instant::now()
            .duration_since_epoch()
            .as_micros()
            .wrapping_sub(start.duration_since_epoch().as_micros())
            >= u64::from(timeout_us);
        if RING.emergency() || late {
            return false;
        }
        core::hint::spin_loop();
    }
}

/// One drain step: acknowledge `TxDone`, move what the FIFO accepts, rearm or disarm `TxDone`.
/// `from_isr`: a spurious call (no `TxDone`) does nothing.
fn service(from_isr: bool) {
    critical_section::with(|cs| {
        #[cfg(feature = "ring-stats")]
        let t0 = stats::ticks();
        let mut slot = UART.borrow_ref_mut(cs);
        let Some(uart) = slot.as_mut() else { return };
        let raised = uart.interrupts().contains(UartInterrupt::TxDone);
        if from_isr && !raised {
            return;
        }
        if raised {
            uart.clear_interrupts(UartInterrupt::TxDone.into());
        }
        match RING.pump(|chunk| fill(uart, chunk)) {
            // An emergency owns the wire now: leave the hardware as it is.
            Pump::Frozen => {}
            Pump::Done { moved, remaining } => {
                if moved > 0 {
                    IDLE.store(false, Release);
                    uart.listen(UartInterrupt::TxDone);
                } else if remaining == 0 && raised {
                    IDLE.store(true, Release);
                    uart.unlisten(UartInterrupt::TxDone);
                }
            }
        }
        #[cfg(feature = "ring-stats")]
        stats::note(&stats::SERVICE, stats::ticks().wrapping_sub(t0));
    });
}

/// Puts into the FIFO what it takes without waiting.
fn fill(uart: &mut Uart<'static, Blocking>, chunk: &[u8]) -> usize {
    let mut done = 0;
    while done < chunk.len() && uart.write_ready() {
        match uart.write(&chunk[done..]) {
            Ok(n) => done += n,
            Err(_) => break,
        }
    }
    #[cfg(feature = "fault-injection")]
    if done > 0 && fault::ARMED.load(Acquire) {
        fault::ARMED.store(false, Release);
        // The FIFO has the bytes, the ring has not forgotten them yet: the unsafe state.
        panic!("injected fault inside the ring");
    }
    done
}

/// Test instrument (feature `fault-injection`): the next pump that moves bytes panics inside the ring.
#[cfg(feature = "fault-injection")]
mod fault {
    pub(super) static ARMED: core::sync::atomic::AtomicBool =
        core::sync::atomic::AtomicBool::new(false);
}

/// Arms the fault: the next pump that gives bytes to the FIFO panics before the ring advances.
#[cfg(feature = "fault-injection")]
pub fn arm_fault_in_pump() {
    fault::ARMED.store(true, Release);
}

#[handler]
fn uart0_isr() {
    if RING.emergency() {
        // A panic owns the wire and cannot acknowledge for us: silence this core's routing of the
        // UART interrupt (only this core's: the handler is bound to the core that installed it).
        esp_hal::interrupt::disable(Cpu::current(), Interrupt::UART0);
        return;
    }
    service(true);
}

// ---- emergency path ----

fn now_us() -> u64 {
    Instant::now().duration_since_epoch().as_micros()
}

/// A bound in time, with an iteration cap for when the timer does not run.
fn bounded(us: u64) -> impl FnMut() -> bool {
    let start = now_us();
    let mut spins = 0u32;
    move || {
        spins += 1;
        spins > SPIN_CAP || now_us().wrapping_sub(start) >= us
    }
}

fn wait_for(mut done: impl FnMut() -> bool, us: u64) {
    let mut late = bounded(us);
    while !done() && !late() {
        core::hint::spin_loop();
    }
}

/// Direct, synchronous output of the ROM routine: no lock, no ring.
pub(crate) fn direct(bytes: &[u8]) {
    esp_println::Printer::write_bytes(bytes);
}

fn direct_line(args: core::fmt::Arguments<'_>) {
    let mut line = LineBuf::<LINE>::new();
    let _ = line.write_fmt(args);
    line.finish();
    direct(line.as_bytes());
}

/// The fatal diagnostic of a panic: the panic line first, then what the ring still held.
///
/// Winner of the race: panic line, then (once the freeze is confirmed) the untransferred bytes,
/// announced and unchanged. Loser: waits for the winner's line (at most 25 ms), then writes its
/// own. Nothing here waits without a bound, takes the critical section or touches `iobewi-log`.
pub(crate) fn emergency(info: &core::panic::PanicInfo<'_>) {
    let core = Cpu::current() as u32;
    let mut fatal = LineBuf::<LINE>::new();
    let _ = write!(fatal, "\r\nPANIC[core {core}]: {info}");
    fatal.finish();
    match RING.claim() {
        Claim::Loser => {
            RING.announce_second();
            wait_for(|| RING.fatal_done(), SECOND_WAIT_US);
            direct(fatal.as_bytes());
            RING.mark_second_done();
        }
        Claim::Winner => {
            direct(fatal.as_bytes());
            RING.mark_fatal_done();
            if RING.second_announced() {
                wait_for(|| RING.second_done(), SECOND_WAIT_US);
            }
            replay();
        }
    }
}

fn replay() {
    match RING.freeze(bounded(FREEZE_US)) {
        Ok(frozen) => {
            let lost = RING.lost_total();
            if frozen.is_empty() && lost == 0 {
                return;
            }
            direct_line(format_args!(
                "-- console ring: {} byte(s) not transmitted before the panic, {lost} message(s) \
                 dropped since start; the first fragment may continue an interrupted line --",
                frozen.len()
            ));
            frozen.for_each(32, direct);
            direct_line(format_args!("-- end of console ring --"));
        }
        Err(why) => {
            let why = match why {
                FreezeError::Own => "the panic happened inside the ring",
                FreezeError::Timeout => "the freeze was not confirmed in time",
                FreezeError::Doubtful => "the ring state is doubtful",
            };
            direct_line(format_args!("-- console ring not replayed: {why} --"));
        }
    }
}

#[cfg(feature = "ring-stats")]
pub use stats::{RingStats, ring_stats};

/// Measurements of the ring's critical sections (feature `ring-stats`, for qualification).
#[cfg(feature = "ring-stats")]
mod stats {
    use super::*;
    use core::sync::atomic::AtomicU32;
    use esp_hal::timer::systimer::{SystemTimer, Unit};

    pub(super) static HOLD: AtomicU32 = AtomicU32::new(0);
    pub(super) static SERVICE: AtomicU32 = AtomicU32::new(0);

    pub(super) fn ticks() -> u32 {
        SystemTimer::unit_value(Unit::Unit0) as u32
    }

    /// Call inside the critical section.
    pub(super) fn note(max: &AtomicU32, dt: u32) {
        if dt > max.load(Relaxed) {
            max.store(dt, Relaxed);
        }
    }

    /// Longest durations seen, in system-timer ticks (`ticks_per_second`).
    pub struct RingStats {
        /// Longest time the ring's exclusive section was held by one actor (deposit or pump).
        pub hold_max: u32,
        /// Longest whole drain step, including the UART calls.
        pub service_max: u32,
        pub ticks_per_second: u64,
    }

    pub fn ring_stats() -> RingStats {
        RingStats {
            hold_max: HOLD.load(Relaxed),
            service_max: SERVICE.load(Relaxed),
            ticks_per_second: SystemTimer::ticks_per_second(),
        }
    }
}
