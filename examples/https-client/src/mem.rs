//! Heap and main-stack figures for the qualification report.
//!
//! The heap comes from `esp-alloc`'s statistics. The stack high-water mark is
//! measured by painting: `paint` fills the unused part of the cpu0 main stack
//! with a pattern at boot and `report` finds how far it was overwritten. The
//! Embassy executor and its tasks run on that stack. Stacks of the radio tasks
//! live in the heap and are not covered.

use core::sync::atomic::{AtomicUsize, Ordering};

unsafe extern "C" {
    static _stack_end_cpu0: u32;
    static _stack_start_cpu0: u32;
}

const PATTERN: u32 = 0xA5A5_A5A5;
/// The bottom of the stack holds `__stack_chk_guard`: leave it alone.
const SKIP: usize = 256;
/// Margin under the painter's own frame.
const MARGIN: usize = 1024;
static PAINT_TOP: AtomicUsize = AtomicUsize::new(0);

fn bounds() -> (usize, usize) {
    (
        core::ptr::addr_of!(_stack_end_cpu0) as usize,
        core::ptr::addr_of!(_stack_start_cpu0) as usize,
    )
}

/// Paint the free part of the main stack. Call first in `main`.
#[inline(never)]
pub fn paint() {
    let marker = 0u32;
    let sp = core::ptr::addr_of!(marker) as usize;
    let (end, _) = bounds();
    let top = (sp - MARGIN) & !3;
    let mut p = end + SKIP;
    while p < top {
        // SAFETY: [end + SKIP, top) is the unused part of this core's stack,
        // below the current frame by MARGIN; nothing else lives there yet.
        unsafe { core::ptr::write_volatile(p as *mut u32, PATTERN) };
        p += 4;
    }
    PAINT_TOP.store(top, Ordering::Relaxed);
}

/// Largest block the heap can still hand out, in KiB (binary search on 1 KiB steps up to the
/// free total): a gap between this and `free` is fragmentation. It allocates transiently, so it
/// raises the heap's `max_used`: it is called once, after the peak has been read.
fn largest_free_kib() -> usize {
    let free_kib = esp_alloc::HEAP.free() / 1024;
    let fits = |kib: usize| {
        let mut block = alloc::vec::Vec::<u8>::new();
        block.try_reserve_exact(kib * 1024).is_ok()
    };
    let (mut lo, mut hi) = (0, free_kib);
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if fits(mid) { lo = mid } else { hi = mid - 1 }
    }
    lo
}

/// Log heap and stack figures at a named point.
pub fn report(label: &str) {
    let (end, start) = bounds();
    let top = PAINT_TOP.load(Ordering::Relaxed);
    let lo = end + SKIP;
    let mut p = lo;
    // SAFETY: reads inside the painted range, which is mapped stack memory.
    while p < top && unsafe { core::ptr::read_volatile(p as *const u32) } == PATTERN {
        p += 4;
    }
    let reserved = start - end;
    let used = start - p;
    let note = if p == lo {
        " (paint exhausted: at least)"
    } else {
        ""
    };
    let h = esp_alloc::HEAP.stats();
    log::info!(
        "MEM {label}: heap size={} used={} free={} max_used={}; stack reserved={reserved} max_used={used} margin={}{note}",
        h.size,
        h.current_usage,
        esp_alloc::HEAP.free(),
        h.max_usage,
        reserved - used,
    );
}

/// The end-of-run figure: the peak is already logged by `report`, this adds the largest free block.
pub fn report_fragmentation() {
    log::info!(
        "MEM fragmentation: free={} largest_free_kib={} (measured after the peak was logged)",
        esp_alloc::HEAP.free(),
        largest_free_kib()
    );
}
