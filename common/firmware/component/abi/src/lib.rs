//! The **binary contract** (ABI v1) between the Agent (which loads and supervises) and a
//! native Workload (a separately compiled binary).
//!
//! Nothing Rust-specific crosses this boundary: no trait objects, references, `Box`,
//! `String`, `Vec`, `Future`, Rust enums or Rust-ABI functions. Only
//!
//! * `#[repr(C)]` structs of fixed-size integers and `AtomicU32`,
//! * explicit pointers (32-bit on every target this ABI is defined for),
//! * `extern "C"` function pointers with integer/pointer arguments,
//! * `i32` status codes (`0` = ok, negative = error, see [`status`]).
//!
//! The Workload receives one pointer, `*const WorkloadContextV1`, valid for the whole
//! execution. It discovers services through it; there are no global Agent symbols and no
//! magic addresses. The safe Rust API over it is the `iobewi-workload` crate.
//!
//! Layout rules: all offsets below are for 32-bit pointers (Xtensa LX7, RISC-V RV32); they
//! are asserted at compile time on 32-bit targets and by host tests on every target.
#![no_std]

use core::mem::{align_of, size_of};
use core::sync::atomic::AtomicU32;

/// Version of the `WorkloadContext` layout. Bumped only on an incompatible layout change;
/// additive growth uses the `size` fields of the context and of each service table.
pub const ABI_VERSION: u16 = 1;

/// Status codes returned by services (`i32`, stable).
pub mod status {
    pub const OK: i32 = 0;
    /// A null pointer, an out-of-range length or an unknown level.
    pub const INVALID: i32 = -1;
    /// A stop was requested: the call returned early (e.g. `sleep_us`) and the Workload
    /// should return from its entry point.
    pub const STOP_REQUESTED: i32 = -2;
    /// The Agent does not provide this service in this build.
    pub const UNSUPPORTED: i32 = -3;
}

/// Log levels (`u32`).
pub mod level {
    pub const ERROR: u32 = 1;
    pub const WARN: u32 = 2;
    pub const INFO: u32 = 3;
    pub const DEBUG: u32 = 4;
}

/// Lifecycle states stored in [`ControlBlockV1::state`].
pub mod state {
    /// Agent has prepared the control block, the Workload has not started.
    pub const STARTING: u32 = 1;
    /// The Workload entry point is executing.
    pub const RUNNING: u32 = 2;
    /// The entry point returned.
    pub const STOPPED: u32 = 3;
    /// The Workload reported a failure (panic) and will not make progress.
    pub const FAILED: u32 = 4;
}

/// Shared control block (Agent-owned memory, Workload gets a pointer). Every field is an
/// atomic `u32`: a Workload and the Agent may run on different cores.
///
/// Direction: Agent writes `stop_requested`; the Workload writes `state`,
/// `progress`, `exit_code`, `fault`. `progress` is the proof of life the Supervisor reads.
#[repr(C)]
pub struct ControlBlockV1 {
    /// `size_of::<ControlBlockV1>()`, written by the Agent.
    pub size: AtomicU32,
    pub abi_version: AtomicU32,
    pub state: AtomicU32,
    pub stop_requested: AtomicU32,
    pub progress: AtomicU32,
    /// Value returned by the entry point (as `u32` bit pattern of the `i32`).
    pub exit_code: AtomicU32,
    /// `0` = none; otherwise a fault code set by the Workload runtime support (panic = 1).
    pub fault: AtomicU32,
    pub reserved: AtomicU32,
}

impl ControlBlockV1 {
    pub const fn new() -> Self {
        Self {
            size: AtomicU32::new(size_of::<ControlBlockV1>() as u32),
            abi_version: AtomicU32::new(ABI_VERSION as u32),
            state: AtomicU32::new(0),
            stop_requested: AtomicU32::new(0),
            progress: AtomicU32::new(0),
            exit_code: AtomicU32::new(0),
            fault: AtomicU32::new(0),
            reserved: AtomicU32::new(0),
        }
    }
}

impl Default for ControlBlockV1 {
    fn default() -> Self {
        Self::new()
    }
}

/// `log(level, ptr, len)`: `len` bytes of UTF-8 text at `ptr` (not NUL-terminated, copied
/// by the Agent before the call returns). Returns a [`status`] code.
pub type LogWriteFn = extern "C" fn(level: u32, msg: *const u8, len: u32) -> i32;

/// `monotonic_us(out)`: writes a monotonic microsecond counter (not wall-clock, no NTP) to
/// `*out`. Returns a [`status`] code.
pub type MonotonicUsFn = extern "C" fn(out: *mut u64) -> i32;

/// `sleep_us(us)`: waits at least `us` microseconds, or returns [`status::STOP_REQUESTED`]
/// as soon as a stop is requested. Cooperative; it does not yield to an Agent scheduler on
/// the calling core.
pub type SleepUsFn = extern "C" fn(us: u32) -> i32;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct LogServiceV1 {
    /// `size_of::<LogServiceV1>()`
    pub size: u32,
    pub write: LogWriteFn,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TimeServiceV1 {
    /// `size_of::<TimeServiceV1>()`
    pub size: u32,
    pub monotonic_us: MonotonicUsFn,
    pub sleep_us: SleepUsFn,
}

/// The one argument of the Workload entry point.
#[repr(C)]
pub struct WorkloadContextV1 {
    /// `size_of::<WorkloadContextV1>()`: lets a newer Workload detect an older Agent.
    pub size: u32,
    /// [`ABI_VERSION`].
    pub abi_version: u32,
    /// The `RuntimeApi` this Agent provides (major, minor).
    pub runtime_api_major: u16,
    pub runtime_api_minor: u16,
    /// Reserved, zero.
    pub flags: u32,
    /// Agent-owned, valid for the whole execution; never freed or moved.
    pub control: *const ControlBlockV1,
    pub log: LogServiceV1,
    pub time: TimeServiceV1,
}

/// Entry point signature of a Workload: `extern "C" fn(ctx) -> i32`. Returning ends the
/// Workload (`0` = clean stop).
pub type WorkloadEntryFn = unsafe extern "C" fn(ctx: *const WorkloadContextV1) -> i32;

/// Name of the symbol the linker script exports as the entry point.
pub const ENTRY_SYMBOL: &str = "workload_entry";

// ---- layout assertions (32-bit pointers: Xtensa LX7, RISC-V RV32) ----------------------
#[cfg(target_pointer_width = "32")]
const _: () = {
    assert!(size_of::<ControlBlockV1>() == 32);
    assert!(align_of::<ControlBlockV1>() == 4);
    assert!(size_of::<LogServiceV1>() == 8);
    assert!(size_of::<TimeServiceV1>() == 12);
    assert!(size_of::<WorkloadContextV1>() == 40);
    assert!(align_of::<WorkloadContextV1>() == 4);
    assert!(core::mem::offset_of!(WorkloadContextV1, control) == 16);
    assert!(core::mem::offset_of!(WorkloadContextV1, log) == 20);
    assert!(core::mem::offset_of!(WorkloadContextV1, time) == 28);
};
// Layout-independent assertions (also hold on a 64-bit host).
const _: () = {
    assert!(size_of::<AtomicU32>() == 4);
    assert!(align_of::<ControlBlockV1>() == 4);
    assert!(core::mem::offset_of!(WorkloadContextV1, size) == 0);
    assert!(core::mem::offset_of!(WorkloadContextV1, abi_version) == 4);
    assert!(core::mem::offset_of!(WorkloadContextV1, runtime_api_major) == 8);
    assert!(core::mem::offset_of!(WorkloadContextV1, runtime_api_minor) == 10);
    assert!(core::mem::offset_of!(WorkloadContextV1, flags) == 12);
    assert!(core::mem::offset_of!(ControlBlockV1, state) == 8);
    assert!(core::mem::offset_of!(ControlBlockV1, stop_requested) == 12);
    assert!(core::mem::offset_of!(ControlBlockV1, progress) == 16);
    assert!(core::mem::offset_of!(ControlBlockV1, exit_code) == 20);
    assert!(core::mem::offset_of!(ControlBlockV1, fault) == 24);
};

#[cfg(test)]
extern crate std;
#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::offset_of;

    /// Expected sizes/offsets for a 32-bit pointer target, computed from the pointer size so
    /// the test also checks the 32-bit numbers documented in `workload-runtime-api.md` when
    /// run on a 64-bit host.
    fn p() -> usize {
        size_of::<*const u8>()
    }

    #[test]
    fn control_block_is_eight_u32s_with_documented_offsets() {
        assert_eq!(size_of::<ControlBlockV1>(), 32);
        assert_eq!(offset_of!(ControlBlockV1, size), 0);
        assert_eq!(offset_of!(ControlBlockV1, abi_version), 4);
        assert_eq!(offset_of!(ControlBlockV1, state), 8);
        assert_eq!(offset_of!(ControlBlockV1, stop_requested), 12);
        assert_eq!(offset_of!(ControlBlockV1, progress), 16);
        assert_eq!(offset_of!(ControlBlockV1, exit_code), 20);
        assert_eq!(offset_of!(ControlBlockV1, fault), 24);
        assert_eq!(offset_of!(ControlBlockV1, reserved), 28);
    }

    #[test]
    fn context_layout_follows_the_documented_table() {
        // 32-bit pointers: 4 + 4 + 2 + 2 + 4 + 4 (control) + 8 (log) + 12 (time) = 40.
        if p() == 4 {
            assert_eq!(size_of::<WorkloadContextV1>(), 40);
            assert_eq!(offset_of!(WorkloadContextV1, control), 16);
            assert_eq!(offset_of!(WorkloadContextV1, log), 20);
            assert_eq!(offset_of!(WorkloadContextV1, time), 28);
            assert_eq!(size_of::<LogServiceV1>(), 8);
            assert_eq!(size_of::<TimeServiceV1>(), 12);
        } else {
            // On a 64-bit host the fixed-size prefix is still the documented one.
            assert_eq!(offset_of!(WorkloadContextV1, control), 16);
            assert_eq!(size_of::<LogServiceV1>(), 16);
        }
        assert_eq!(offset_of!(WorkloadContextV1, flags), 12);
    }

    #[test]
    fn new_control_block_describes_itself() {
        use core::sync::atomic::Ordering::Relaxed;
        let c = ControlBlockV1::new();
        assert_eq!(c.size.load(Relaxed), 32);
        assert_eq!(c.abi_version.load(Relaxed), u32::from(ABI_VERSION));
        assert_eq!(c.state.load(Relaxed), 0);
    }

    #[test]
    fn status_levels_and_states_are_distinct_stable_values() {
        assert_eq!(
            (
                status::OK,
                status::INVALID,
                status::STOP_REQUESTED,
                status::UNSUPPORTED
            ),
            (0, -1, -2, -3)
        );
        assert_eq!(
            (level::ERROR, level::WARN, level::INFO, level::DEBUG),
            (1, 2, 3, 4)
        );
        assert_eq!(
            (
                state::STARTING,
                state::RUNNING,
                state::STOPPED,
                state::FAILED
            ),
            (1, 2, 3, 4)
        );
    }
}
