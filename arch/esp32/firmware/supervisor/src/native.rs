//! ESP32-S3 backend of the native Workload runtime.
//!
//! * **Where the code lives.** One fixed RAM region, the tail of the reclaimed `dram2` area
//!   (`0x3FCE3700..0x3FCEB700`, 32 KiB), reserved by the Agent's linker script. SRAM1 is
//!   mapped twice: the *instruction bus* (`+0x6F0000`) is where code is linked and executed,
//!   the *data bus* is where this module writes it. No cache is involved (internal SRAM), no
//!   MMU, no relocation: an image linked for another address is refused by the gate.
//! * **Who runs it.** The second core (APP_CPU). The Workload entry point runs there with its
//!   own stack, in parallel with the Agent's executor, which is never blocked. Interrupts are
//!   masked on that core and the Workload owns no hardware.
//! * **How it stops.** Cooperatively through the control block; if the Workload ignores the
//!   request, [`halt`](NativeBackend::halt) parks (stalls) the core, so no Workload instruction
//!   runs afterwards. That is a real stop, not a flag.
//! * **Flash interplay.** The shared flash is built with `multicore_auto_park`: while the
//!   Agent writes or erases flash, the Workload core is paused and resumed (a few hundred ms
//!   for a large erase), so a Workload can never execute from a flash cache that is off.
//! * **What is NOT isolated.** The Workload is *trusted native code*: nothing prevents it from
//!   reading or writing any Agent memory or peripheral. The digest guarantees integrity, not
//!   harmlessness. See `docs/workload-runtime-api.md`.

use core::cell::{RefCell, UnsafeCell};
use core::sync::atomic::{AtomicU32, Ordering, fence};

use esp_hal::system::{Cpu, CpuControl, Stack};
use iobewi_firmware_update::compat::otm2::RuntimeApi;
use iobewi_workload_abi::{
    ABI_VERSION, ControlBlockV1, LogServiceV1, TimeServiceV1, WorkloadContextV1, WorkloadEntryFn,
    status,
};
use iobewi_workload_image::{
    ESP32S3_CODE_CAPACITY, ESP32S3_DATA_ADDR, ESP32S3_DATA_CAPACITY, ESP32S3_REGION_DBUS,
    ESP32S3_REGION_SIZE, TargetLayout, esp32s3_layout,
};
use iobewi_workload_native::{LaunchError, NativeBackend};

/// Data-bus address where the Agent must place the Workload region.
pub const REGION_DBUS: u32 = ESP32S3_REGION_DBUS;
pub use iobewi_workload_native::{NativeConfig, NativeRuntime, StopOutcome, boot_guard};

/// Stack of the Workload core (Agent-owned memory; this is the Workload's whole stack budget).
pub const WORKLOAD_STACK_SIZE: usize = 8 * 1024;

struct SyncCell<T>(UnsafeCell<T>);
// SAFETY: access is serialised by the launch/halt protocol described on each use.
unsafe impl<T> Sync for SyncCell<T> {}

static CONTROL: ControlBlockV1 = ControlBlockV1::new();
static CONTEXT: SyncCell<WorkloadContextV1> = SyncCell(UnsafeCell::new(WorkloadContextV1 {
    size: core::mem::size_of::<WorkloadContextV1>() as u32,
    abi_version: ABI_VERSION as u32,
    runtime_api_major: 0,
    runtime_api_minor: 0,
    flags: 0,
    control: core::ptr::null(),
    log: LogServiceV1 {
        size: core::mem::size_of::<LogServiceV1>() as u32,
        write: svc_log,
    },
    time: TimeServiceV1 {
        size: core::mem::size_of::<TimeServiceV1>() as u32,
        monotonic_us: svc_monotonic_us,
        sleep_us: svc_sleep_us,
    },
}));
static STACK: SyncCell<Stack<WORKLOAD_STACK_SIZE>> = SyncCell(UnsafeCell::new(Stack::new()));

// ---- log ring (single producer = Workload core, single consumer = Agent core) ----------

const RING_CAP: usize = 2048;
const MAX_MSG: usize = 120;
static RING: SyncCell<[u8; RING_CAP]> = SyncCell(UnsafeCell::new([0; RING_CAP]));
static RING_HEAD: AtomicU32 = AtomicU32::new(0); // bytes ever written (producer)
static RING_TAIL: AtomicU32 = AtomicU32::new(0); // bytes ever consumed (consumer)
static RING_DROPPED: AtomicU32 = AtomicU32::new(0);

extern "C" fn svc_log(level: u32, msg: *const u8, len: u32) -> i32 {
    if msg.is_null() || !(1..=4).contains(&level) {
        return status::INVALID;
    }
    let len = (len as usize).min(MAX_MSG);
    let head = RING_HEAD.load(Ordering::Relaxed);
    let tail = RING_TAIL.load(Ordering::Acquire);
    let used = head.wrapping_sub(tail) as usize;
    if RING_CAP - used < len + 2 {
        RING_DROPPED.fetch_add(1, Ordering::Relaxed);
        return status::OK;
    }
    let ring = RING.0.get().cast::<u8>();
    let put = |i: usize, b: u8| {
        // SAFETY: index is reduced modulo the capacity; only this core writes free space.
        unsafe { ring.add((head as usize + i) % RING_CAP).write(b) };
    };
    put(0, level as u8);
    put(1, len as u8);
    for i in 0..len {
        // SAFETY: the caller promised `len` readable bytes (trusted Workload).
        put(2 + i, unsafe { msg.add(i).read() });
    }
    RING_HEAD.store(head.wrapping_add(len as u32 + 2), Ordering::Release);
    status::OK
}

/// Drain at most `max_lines` of the Workload's log lines into `sink(level, text)`. Call it
/// periodically from the Agent (a task every ~100 ms). It is **bounded**: a Workload that logs
/// in a tight loop can neither starve the Agent's executor (the ring is finite and the drain
/// stops after `max_lines`) nor flood the Agent's log (excess lines are dropped by the ring and
/// reported once as a count).
pub fn drain_logs(max_lines: usize, mut sink: impl FnMut(u32, &[u8])) {
    let ring = RING.0.get().cast::<u8>();
    for _ in 0..max_lines {
        let tail = RING_TAIL.load(Ordering::Relaxed);
        let head = RING_HEAD.load(Ordering::Acquire);
        if head == tail {
            break;
        }
        let get = |i: usize| -> u8 {
            // SAFETY: bytes between tail and head were published by the producer.
            unsafe { ring.add((tail as usize + i) % RING_CAP).read() }
        };
        let level = u32::from(get(0));
        let len = get(1) as usize;
        let mut line = [0u8; MAX_MSG];
        for (i, slot) in line.iter_mut().enumerate().take(len) {
            *slot = get(2 + i);
        }
        RING_TAIL.store(tail.wrapping_add(len as u32 + 2), Ordering::Release);
        sink(level, &line[..len]);
    }
}

/// Number of log lines the ring had to drop (ring full) since the last call.
pub fn take_dropped_logs() -> u32 {
    RING_DROPPED.swap(0, Ordering::Relaxed)
}

// ---- time ------------------------------------------------------------------------------

fn now_us() -> u64 {
    esp_hal::time::Instant::now()
        .duration_since_epoch()
        .as_micros()
}

extern "C" fn svc_monotonic_us(out: *mut u64) -> i32 {
    if out.is_null() {
        return status::INVALID;
    }
    // SAFETY: non-null, provided by the Workload for this call.
    unsafe { out.write(now_us()) };
    status::OK
}

extern "C" fn svc_sleep_us(us: u32) -> i32 {
    let start = now_us();
    loop {
        if CONTROL.stop_requested.load(Ordering::Acquire) != 0 {
            return status::STOP_REQUESTED;
        }
        if now_us().wrapping_sub(start) >= u64::from(us) {
            return status::OK;
        }
        core::hint::spin_loop();
    }
}

// ---- the backend -----------------------------------------------------------------------

pub struct EspNativeBackend {
    cpu: RefCell<CpuControl<'static>>,
    provides: RuntimeApi,
    /// The Agent confirmed that the region really is at the fixed address. When it is not,
    /// the layout advertises zero capacity, so the gate refuses every image and nothing is
    /// ever written there.
    region_ok: bool,
}

/// What runs on the Workload core. Never returns: when the Workload's entry point returns
/// the core spins here until the Agent parks it.
fn workload_core_main(entry: usize, ctx: usize) {
    // SAFETY: `entry` was validated by the image gate to be an aligned address inside the
    // freshly loaded code area; `ctx` is the static context built before the launch.
    let f: WorkloadEntryFn = unsafe { core::mem::transmute::<usize, WorkloadEntryFn>(entry) };
    // SAFETY: calling trusted native code with the documented ABI.
    let code = unsafe { f(ctx as *const WorkloadContextV1) };
    CONTROL.exit_code.store(code as u32, Ordering::Release);
    loop {
        core::hint::spin_loop();
    }
}

impl EspNativeBackend {
    /// # Safety
    /// The fixed IWNI RAM region must be reserved exclusively for this backend
    /// for the entire program. The caller must use trusted native artifacts and
    /// the validated NativeRuntime loading protocol; there is no memory sandbox.
    pub unsafe fn new(cpu: CpuControl<'static>, provides: RuntimeApi, region_ok: bool) -> Self {
        Self {
            cpu: RefCell::new(cpu),
            provides,
            region_ok,
        }
    }
}

impl NativeBackend for EspNativeBackend {
    fn layout(&self) -> TargetLayout {
        let mut layout = esp32s3_layout(self.provides, ABI_VERSION);
        if !self.region_ok {
            layout.code_capacity = 0;
            layout.data_capacity = 0;
        }
        layout
    }

    fn control(&self) -> &ControlBlockV1 {
        &CONTROL
    }

    fn clear_region(&self) {
        // SAFETY: the Agent's linker script reserves exactly this region; nothing runs on the
        // Workload core (the runtime halts it before reusing the region).
        unsafe {
            core::ptr::write_bytes(
                ESP32S3_REGION_DBUS as *mut u8,
                0,
                ESP32S3_REGION_SIZE as usize,
            )
        };
        fence(Ordering::SeqCst);
    }

    fn write_code(&self, offset: u32, bytes: &[u8]) {
        assert!(offset as usize + bytes.len() <= ESP32S3_CODE_CAPACITY as usize);
        // SAFETY: bounds checked above; region reserved (see `clear_region`).
        unsafe {
            core::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                (ESP32S3_REGION_DBUS as *mut u8).add(offset as usize),
                bytes.len(),
            )
        };
    }

    fn write_data(&self, offset: u32, bytes: &[u8]) {
        assert!(offset as usize + bytes.len() <= ESP32S3_DATA_CAPACITY as usize);
        // SAFETY: as above, on the data area.
        unsafe {
            core::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                (ESP32S3_DATA_ADDR as *mut u8).add(offset as usize),
                bytes.len(),
            )
        };
    }

    fn launch(&self, entry: u32) -> Result<(), LaunchError> {
        // SAFETY: the Workload core is halted (the runtime stops before every launch), so
        // nothing else reads the context or the stack.
        unsafe {
            let ctx = &mut *CONTEXT.0.get();
            ctx.control = &CONTROL;
            ctx.runtime_api_major = self.provides.major;
            ctx.runtime_api_minor = self.provides.minor;
        }
        fence(Ordering::SeqCst);
        let ctx_addr = CONTEXT.0.get() as usize;
        let entry = entry as usize;
        // SAFETY: the stack is only ever used by the Workload core, which is halted.
        let stack: &'static mut Stack<WORKLOAD_STACK_SIZE> = unsafe { &mut *STACK.0.get() };
        let guard = self
            .cpu
            .borrow_mut()
            .start_app_core(stack, move || workload_core_main(entry, ctx_addr))
            .map_err(|_| LaunchError("core already running"))?;
        // Dropping the guard would park the core.
        core::mem::forget(guard);
        Ok(())
    }

    fn halt(&self) {
        // SAFETY: parking the second core is always allowed; the Workload gets no more cycles.
        unsafe { self.cpu.borrow_mut().park_core(Cpu::AppCpu) };
    }

    fn now_ms(&self) -> u64 {
        now_us() / 1000
    }

    async fn delay_ms(&self, ms: u32) {
        embassy_time::Timer::after_millis(u64::from(ms)).await;
    }
}
