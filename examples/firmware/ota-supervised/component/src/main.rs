//! Native Workload example. Compiled for the target, packed into an IWNI image by
//! `iobewi-workload-pack`, uploaded through the Workload OTA, loaded and run by the Agent.
//! Nothing here refers to the Agent, esp-hal, flash, partitions or OTA: only the safe
//! `iobewi-workload` API.
#![no_std]
#![no_main]

use iobewi_workload::{Context, workload_main};

#[cfg(not(feature = "variant-b"))]
const IDENT: &str = "native-workload-a";
#[cfg(feature = "variant-b")]
const IDENT: &str = "native-workload-b";

#[cfg(not(feature = "variant-b"))]
const PERIOD_MS: u32 = 250;
#[cfg(feature = "variant-b")]
const PERIOD_MS: u32 = 100;

workload_main!(main);

/// Decimal text of `n` into `buf` (no core::fmt: keeps the image small).
fn dec(mut n: u64, buf: &mut [u8; 20]) -> &[u8] {
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    &buf[i..]
}

fn main(ctx: &Context) -> i32 {
    let log = ctx.log();
    let time = ctx.time();
    let control = ctx.control();

    log.info(IDENT);
    let t0 = time.now_us();
    let mut tick = 0u32;
    loop {
        #[cfg(not(any(feature = "fault-ignore-stop", feature = "fault-log-flood")))]
        if control.stop_requested() {
            break;
        }
        #[cfg(feature = "fault-log-flood")]
        {
            log.info("flood");
            control.progress();
            continue;
        }
        control.progress();
        tick += 1;

        if tick % 20 == 0 {
            let mut line = [0u8; 64];
            let mut n = 0;
            for part in [IDENT.as_bytes(), b" alive, elapsed_ms=".as_slice()] {
                line[n..n + part.len()].copy_from_slice(part);
                n += part.len();
            }
            let mut digits = [0u8; 20];
            let d = dec((time.now_us() - t0) / 1000, &mut digits);
            line[n..n + d.len()].copy_from_slice(d);
            n += d.len();
            log.write(iobewi_workload::level::INFO, &line[..n]);
        }

        #[cfg(feature = "fault-panic")]
        if tick == 8 {
            panic!("native-workload fault: panic");
        }
        #[cfg(feature = "fault-reset-chip")]
        if tick == 24 {
            // A trusted native Workload can do anything: here, a system reset through the
            // RTC control block (OPTIONS0.SW_SYS_RST, bit 31) -- not through any iobewi
            // service. It exists to prove the Agent's crash-loop guard.
            const OPTIONS0: *mut u32 = 0x6000_8000 as *mut u32;
            // SAFETY (deliberately violated): raw peripheral access from the Workload.
            unsafe { OPTIONS0.write_volatile(OPTIONS0.read_volatile() | (1 << 31)) };
        }
        #[cfg(feature = "fault-null-jump")]
        if tick == 24 {
            // SAFETY (deliberately violated): a jump to address 0 raises an
            // instruction-fetch exception, to observe how the Agent copes.
            unsafe {
                core::mem::transmute::<usize, extern "C" fn()>(core::hint::black_box(0usize))()
            };
        }

        // A Workload that ignores stop keeps its own pace (the sleep service would return at
        // once after a stop request).
        #[cfg(feature = "fault-ignore-stop")]
        {
            let until = time.now_us() + u64::from(PERIOD_MS) * 1000;
            while time.now_us() < until {}
        }
        #[cfg(not(feature = "fault-ignore-stop"))]
        if !time.sleep_ms(PERIOD_MS) {
            break;
        }
    }
    log.info("stopping");
    0
}
