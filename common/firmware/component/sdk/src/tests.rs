use super::*;
use core::sync::atomic::{AtomicU32, AtomicU64};
use std::sync::Mutex;
use std::vec::Vec;

static LOGS: Mutex<Vec<(u32, Vec<u8>)>> = Mutex::new(Vec::new());
static NOW: AtomicU64 = AtomicU64::new(1_000);
static SLEPT: AtomicU32 = AtomicU32::new(0);
static STOP_ON_SLEEP: AtomicU32 = AtomicU32::new(0);

extern "C" fn log_write(level: u32, msg: *const u8, len: u32) -> i32 {
    if msg.is_null() {
        return status::INVALID;
    }
    // SAFETY: the SDK passes a valid slice.
    let bytes = unsafe { core::slice::from_raw_parts(msg, len as usize) }.to_vec();
    LOGS.lock().unwrap().push((level, bytes));
    status::OK
}
extern "C" fn mono(out: *mut u64) -> i32 {
    // SAFETY: the SDK passes a valid pointer.
    unsafe { *out = NOW.load(Ordering::Relaxed) };
    status::OK
}
extern "C" fn sleep(us: u32) -> i32 {
    SLEPT.fetch_add(us, Ordering::Relaxed);
    if STOP_ON_SLEEP.load(Ordering::Relaxed) != 0 {
        status::STOP_REQUESTED
    } else {
        status::OK
    }
}

static BLOCK: ControlBlockV1 = ControlBlockV1::new();

fn ctx_struct() -> WorkloadContextV1 {
    WorkloadContextV1 {
        size: core::mem::size_of::<WorkloadContextV1>() as u32,
        abi_version: 1,
        runtime_api_major: 1,
        runtime_api_minor: 0,
        flags: 0,
        control: &BLOCK,
        log: abi::LogServiceV1 {
            size: core::mem::size_of::<abi::LogServiceV1>() as u32,
            write: log_write,
        },
        time: abi::TimeServiceV1 {
            size: core::mem::size_of::<abi::TimeServiceV1>() as u32,
            monotonic_us: mono,
            sleep_us: sleep,
        },
    }
}

fn with_ctx(f: impl FnOnce(&Context)) {
    let raw = std::boxed::Box::leak(std::boxed::Box::new(ctx_struct()));
    // SAFETY: a valid, leaked context.
    let ctx = unsafe { Context::from_raw(raw) }.unwrap();
    f(&ctx);
}

#[test]
fn the_safe_api_reaches_the_services() {
    with_ctx(|ctx| {
        assert_eq!(ctx.runtime_api(), (1, 0));
        ctx.log().info("hello");
        ctx.log().warn("careful");
        let logs = LOGS.lock().unwrap();
        assert!(logs.contains(&(level::INFO, b"hello".to_vec())));
        assert!(logs.contains(&(level::WARN, b"careful".to_vec())));
        drop(logs);
        assert_eq!(ctx.time().now_us(), 1_000);
        assert!(ctx.time().sleep_ms(2));
        assert_eq!(SLEPT.load(Ordering::Relaxed), 2_000);
        STOP_ON_SLEEP.store(1, Ordering::Relaxed);
        assert!(
            !ctx.time().sleep_us(5),
            "a stop request ends the sleep early"
        );
        STOP_ON_SLEEP.store(0, Ordering::Relaxed);
    });
}

#[test]
fn control_counts_progress_and_sees_stop() {
    with_ctx(|ctx| {
        let c = ctx.control();
        let before = c.progress_value();
        c.progress();
        c.progress();
        assert_eq!(c.progress_value(), before + 2);
        assert!(!c.stop_requested());
        BLOCK.stop_requested.store(1, Ordering::Relaxed);
        assert!(c.stop_requested());
        BLOCK.stop_requested.store(0, Ordering::Relaxed);
    });
}

#[test]
fn a_bad_context_is_refused_not_trusted() {
    // SAFETY: null is accepted by contract.
    assert_eq!(
        unsafe { Context::from_raw(core::ptr::null()) }.err(),
        Some(ContextError::Null)
    );
    let mut c = ctx_struct();
    c.abi_version = 2;
    // SAFETY: `c` is valid for the call.
    assert_eq!(
        unsafe { Context::from_raw(&c) }.err(),
        Some(ContextError::AbiVersion(2))
    );
    let mut c = ctx_struct();
    c.size = 8;
    // SAFETY: as above.
    assert_eq!(
        unsafe { Context::from_raw(&c) }.err(),
        Some(ContextError::TooSmall(8))
    );
}

fn user_main(ctx: &Context) -> i32 {
    ctx.control().progress();
    7
}

#[test]
fn the_entry_announces_running_then_stopped_with_the_exit_code() {
    let raw = std::boxed::Box::leak(std::boxed::Box::new(ctx_struct()));
    // SAFETY: valid leaked context.
    let code = unsafe { rt::run(raw, user_main) };
    assert_eq!(code, 7);
    assert_eq!(BLOCK.state.load(Ordering::Relaxed), state::STOPPED);
    assert_eq!(BLOCK.exit_code.load(Ordering::Relaxed), 7);
    assert!(BLOCK.progress.load(Ordering::Relaxed) >= 1);
    // And an unusable context returns an error without touching the block.
    // SAFETY: null is accepted.
    assert_eq!(unsafe { rt::run(core::ptr::null(), user_main) }, -1);
}
