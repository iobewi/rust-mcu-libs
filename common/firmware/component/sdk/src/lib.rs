//! `iobewi-workload`: the ergonomic, safe API a native Workload is written against.
//!
//! Two layers, deliberately separate:
//!
//! ```text
//! your Workload (Rust, no_std)
//!        |  safe API: Context, Logger, Time, Control      <- this crate
//!        v
//! iobewi-workload-abi: WorkloadContextV1, repr(C), extern "C" tables   <- stable binary contract
//!        v
//! the Agent's services (log ring, monotonic clock, control block)
//! ```
//!
//! The application never sees a raw pointer or a function table: [`Context::from_raw`] is
//! the only `unsafe` entry (called by the [`workload_main!`] macro), and everything after is
//! safe. The crate is `no_std`, allocation-free and knows nothing about the platform (no
//! esp-hal, no partitions, no Agent types).
#![no_std]

use core::sync::atomic::{AtomicPtr, Ordering};
use iobewi_workload_abi as abi;
use iobewi_workload_abi::{ControlBlockV1, WorkloadContextV1};

pub use abi::{ABI_VERSION, level, state, status};

/// Handle to what the Agent provides. `Copy`, valid for the whole execution.
#[derive(Clone, Copy)]
pub struct Context {
    raw: &'static WorkloadContextV1,
}

/// Why a context was refused (wrong ABI, too small).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextError {
    Null,
    /// The Agent speaks an ABI version this SDK does not.
    AbiVersion(u32),
    /// The Agent's context is smaller than what this SDK needs (older Agent).
    TooSmall(u32),
}

impl Context {
    /// # Safety
    /// `ptr` must be the context the Agent passed to the entry point (valid, aligned and
    /// immutable for the whole execution), or null.
    pub unsafe fn from_raw(ptr: *const WorkloadContextV1) -> Result<Self, ContextError> {
        if ptr.is_null() {
            return Err(ContextError::Null);
        }
        // SAFETY: the caller guarantees `ptr` is the Agent's context.
        let raw: &'static WorkloadContextV1 = unsafe { &*ptr };
        if raw.abi_version != u32::from(ABI_VERSION) {
            return Err(ContextError::AbiVersion(raw.abi_version));
        }
        if (raw.size as usize) < core::mem::size_of::<WorkloadContextV1>() {
            return Err(ContextError::TooSmall(raw.size));
        }
        Ok(Self { raw })
    }

    /// The `RuntimeApi` (major, minor) the Agent provides.
    pub fn runtime_api(&self) -> (u16, u16) {
        (self.raw.runtime_api_major, self.raw.runtime_api_minor)
    }

    pub fn log(&self) -> Logger {
        Logger { svc: self.raw.log }
    }

    pub fn time(&self) -> Time {
        Time { svc: self.raw.time }
    }

    pub fn control(&self) -> Control {
        // SAFETY: the Agent guarantees the control block outlives the execution.
        Control {
            block: unsafe { &*self.raw.control },
        }
    }
}

#[derive(Clone, Copy)]
pub struct Logger {
    svc: abi::LogServiceV1,
}

impl Logger {
    /// Send raw bytes at `level`. Returns the service status (`status::OK`...).
    pub fn write(&self, level: u32, msg: &[u8]) -> i32 {
        let len = if msg.len() > u32::MAX as usize {
            u32::MAX
        } else {
            msg.len() as u32
        };
        (self.svc.write)(level, msg.as_ptr(), len)
    }
    pub fn info(&self, msg: &str) {
        self.write(level::INFO, msg.as_bytes());
    }
    pub fn warn(&self, msg: &str) {
        self.write(level::WARN, msg.as_bytes());
    }
    pub fn error(&self, msg: &str) {
        self.write(level::ERROR, msg.as_bytes());
    }
    pub fn debug(&self, msg: &str) {
        self.write(level::DEBUG, msg.as_bytes());
    }
}

#[derive(Clone, Copy)]
pub struct Time {
    svc: abi::TimeServiceV1,
}

impl Time {
    /// Monotonic microseconds (not wall-clock; unrelated to NTP).
    pub fn now_us(&self) -> u64 {
        let mut out = 0u64;
        (self.svc.monotonic_us)(&mut out);
        out
    }

    /// Wait at least `us` microseconds. Returns `false` if a stop was requested first (the
    /// caller should then return from `main`).
    pub fn sleep_us(&self, us: u32) -> bool {
        (self.svc.sleep_us)(us) != status::STOP_REQUESTED
    }

    pub fn sleep_ms(&self, ms: u32) -> bool {
        self.sleep_us(ms.saturating_mul(1000))
    }
}

#[derive(Clone, Copy)]
pub struct Control {
    block: &'static ControlBlockV1,
}

impl Control {
    /// Has the Agent asked this Workload to stop? Poll it in the main loop.
    pub fn stop_requested(&self) -> bool {
        self.block.stop_requested.load(Ordering::Acquire) != 0
    }

    /// Proof of life: the Supervisor's health is derived from this counter advancing.
    pub fn progress(&self) {
        self.block.progress.fetch_add(1, Ordering::Release);
    }

    pub fn progress_value(&self) -> u32 {
        self.block.progress.load(Ordering::Acquire)
    }
}

// ---- entry / panic support (used by `workload_main!`) ---------------------------------

static CONTROL: AtomicPtr<ControlBlockV1> = AtomicPtr::new(core::ptr::null_mut());

#[doc(hidden)]
pub mod rt {
    use super::*;

    /// Called by the generated entry: validates the context, announces `RUNNING`, runs
    /// `main`, announces `STOPPED` with its exit code.
    ///
    /// # Safety
    /// `ctx` must be the Agent-provided context (see [`Context::from_raw`]).
    pub unsafe fn run(ctx: *const WorkloadContextV1, main: fn(&Context) -> i32) -> i32 {
        // SAFETY: forwarded.
        let ctx = match unsafe { Context::from_raw(ctx) } {
            Ok(c) => c,
            Err(_) => return -1,
        };
        let block = ctx.raw.control as *mut ControlBlockV1;
        CONTROL.store(block, Ordering::Release);
        let control = ctx.control();
        control.block.state.store(state::RUNNING, Ordering::Release);
        let code = main(&ctx);
        control
            .block
            .exit_code
            .store(code as u32, Ordering::Release);
        control.block.state.store(state::STOPPED, Ordering::Release);
        code
    }

    /// What the panic handler does: mark the Workload failed and report it. Never returns.
    pub fn fail(code: u32) -> ! {
        let block = CONTROL.load(Ordering::Acquire);
        if !block.is_null() {
            // SAFETY: stored from a live Agent-owned block.
            let block = unsafe { &*block };
            block.fault.store(code, Ordering::Release);
            block.state.store(state::FAILED, Ordering::Release);
        }
        loop {
            core::hint::spin_loop();
        }
    }
}

/// Declares the Workload entry point:
///
/// ```ignore
/// iobewi_workload::workload_main!(main);
/// fn main(ctx: &iobewi_workload::Context) -> i32 { ctx.log().info("hello"); 0 }
/// ```
#[macro_export]
macro_rules! workload_main {
    ($main:path) => {
        /// The Workload's native entry point (ABI v1): `extern "C" fn(ctx) -> i32`.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn workload_entry(
            ctx: *const $crate::__abi::WorkloadContextV1,
        ) -> i32 {
            // SAFETY: the Agent passes its context.
            unsafe { $crate::rt::run(ctx, $main) }
        }
    };
}

#[doc(hidden)]
pub use iobewi_workload_abi as __abi;

#[cfg(feature = "panic-handler")]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    rt::fail(1)
}

#[cfg(test)]
extern crate std;
#[cfg(test)]
mod tests;
