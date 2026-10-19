//! Heap and main-stack figures for the qualification report.
//!
//! The heap comes from `esp-alloc`'s statistics. The stack high-water mark is
//! measured by painting: `paint` fills the unused part of the cpu0 main stack
//! with a pattern at boot and `report` finds how far it was overwritten. The
//! Embassy executor and its tasks run on that stack. Stacks of the radio tasks
//! live in the heap and are not covered.

use core::sync::atomic::{AtomicUsize, Ordering};

/// Where the second heap region lives (0 when there is none), for the per-region figures.
pub static DRAM2_START: AtomicUsize = AtomicUsize::new(0);
pub static DRAM2_LEN: AtomicUsize = AtomicUsize::new(0);

/// Peaks seen by `sample` since it started: the heap's own `max_used` also counts the integrity
/// test, which fills it on purpose.
static SAMPLED_TOTAL: AtomicUsize = AtomicUsize::new(0);
static SAMPLED_SECOND: AtomicUsize = AtomicUsize::new(0);
/// Longest time, since the last report, the heartbeat task was late for its 10 ms tick: the time
/// another task kept the executor without yielding (a TLS handshake, for one).
static MAX_LATENESS_MS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

pub fn note_lateness_ms(ms: u32) {
    MAX_LATENESS_MS.fetch_max(ms, Ordering::Relaxed);
}

/// Record the current total and second-region usage if they are new peaks.
pub fn sample() {
    let (main, second) = region_used();
    SAMPLED_TOTAL.fetch_max(main + second, Ordering::Relaxed);
    SAMPLED_SECOND.fetch_max(second, Ordering::Relaxed);
}

fn in_dram2(address: usize) -> bool {
    let start = DRAM2_START.load(Ordering::Relaxed);
    start != 0 && address >= start && address < start + DRAM2_LEN.load(Ordering::Relaxed)
}

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
    for region in h.region_stats.iter().flatten() {
        // Told apart by size, as in `region_used`.
        let name = if DRAM2_LEN.load(Ordering::Relaxed) != 0 && region.size < 76 * 1024 {
            "dram2"
        } else {
            "main"
        };
        log::info!(
            "MEM {label}: region {name} size={} used={} free={}",
            region.size,
            region.used,
            region.free
        );
    }
    log::info!(
        "MEM {label}: sampled_peak_total={} sampled_peak_dram2={} executor_max_lateness_ms={}",
        SAMPLED_TOTAL.load(Ordering::Relaxed),
        SAMPLED_SECOND.load(Ordering::Relaxed),
        MAX_LATENESS_MS.swap(0, Ordering::Relaxed)
    );
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

/// 32-bit FNV-1a over the words of a region, to tell whether it was left as found.
fn checksum(start: usize, len: usize) -> (u32, usize) {
    let (mut hash, mut zeros) = (0x811c_9dc5u32, 0usize);
    let mut p = start;
    while p + 4 <= start + len {
        // SAFETY: the caller passes a region of mapped, otherwise unused internal RAM.
        let word = unsafe { core::ptr::read_volatile(p as *const u32) };
        zeros += usize::from(word == 0);
        hash = (hash ^ word).wrapping_mul(0x0100_0193);
        p += 4;
    }
    (hash, zeros)
}

/// Log what the second region holds before the allocator takes it: the bootloader's leftovers and
/// whatever the previous run left (the RAM is not cleared by a reset), so nothing can assume zeros.
pub fn dram2_boot_probe(start: usize, len: usize) {
    DRAM2_START.store(start, Ordering::Relaxed);
    DRAM2_LEN.store(len, Ordering::Relaxed);
    let (hash, zeros) = checksum(start, len);
    // SAFETY: as in `checksum`.
    let first = unsafe { core::ptr::read_volatile(start as *const u32) };
    log::info!(
        "DRAM2 boot: start={start:#x} len={len} end={:#x} align4={} fnv={hash:#010x} zero_words={zeros}/{} first_word={first:#010x}",
        start + len,
        start % 4 == 0,
        len / 4
    );
}

struct Block {
    data: alloc::vec::Vec<u8>,
    tag: u8,
}

impl Block {
    fn pattern(&self, i: usize) -> u8 {
        self.tag ^ (i as u8).wrapping_mul(31)
    }
    fn fill(&mut self) {
        for i in 0..self.data.len() {
            let v = self.pattern(i);
            self.data[i] = v;
        }
    }
    fn intact(&self) -> bool {
        self.data
            .iter()
            .enumerate()
            .all(|(i, b)| *b == self.pattern(i))
    }
    fn address(&self) -> usize {
        self.data.as_ptr() as usize
    }
}

/// `(main, second)` bytes in use. The statistics carry no address: the second region is the one
/// smaller than the 76 KiB a main heap of 80 KiB or more has, which is the case of this experiment.
fn region_used() -> (usize, usize) {
    let h = esp_alloc::HEAP.stats();
    let (mut main, mut second) = (0, 0);
    for region in h.region_stats.iter().flatten() {
        if DRAM2_LEN.load(Ordering::Relaxed) != 0 && region.size < 76 * 1024 {
            second += region.used;
        } else {
            main += region.used;
        }
    }
    (main, second)
}

const BLOCK: usize = 4096;

fn take(blocks: &mut alloc::vec::Vec<Block>) -> bool {
    let mut data = alloc::vec::Vec::new();
    if data.try_reserve_exact(BLOCK).is_err() {
        return false;
    }
    data.resize(BLOCK, 0);
    let mut block = Block {
        data,
        tag: (blocks.len() as u8) ^ 0x5a,
    };
    block.fill();
    blocks.push(block);
    true
}

/// Allocation and free with integrity checks across both regions, and exhaustion of each one on
/// its own. Run before the radio starts: exhausting the heap under a live driver would test the
/// driver. Returns `(ok, summary)`.
pub fn dram2_integrity() -> (bool, alloc::string::String) {
    use alloc::format;
    use alloc::vec::Vec;
    let base = esp_alloc::HEAP.stats().current_usage;
    let mut blocks: Vec<Block> = Vec::new();
    if blocks.try_reserve_exact(256).is_err() {
        return (false, format!("cannot reserve the block table"));
    }

    // Everything the checks themselves need is allocated before the heap is exhausted.
    let mut spans: Vec<usize> = Vec::new();
    if spans.try_reserve_exact(256).is_err() {
        return (false, format!("cannot reserve the span table"));
    }

    // A. Fill until the allocator refuses (cleanly): both regions are used, nothing overlaps.
    while take(&mut blocks) {}
    let total = blocks.len();
    let in2 = blocks.iter().filter(|b| in_dram2(b.address())).count();
    let aligned = blocks.iter().all(|b| b.address() % 4 == 0);
    let (main_used, second_used) = region_used();
    let intact_a = blocks.iter().all(Block::intact);
    spans.extend(blocks.iter().map(Block::address));
    spans.sort_unstable();
    let disjoint = spans.windows(2).all(|w| w[0] + BLOCK <= w[1]);

    // B. Only the main region has room: free its blocks; every new block must land there.
    let before = blocks.len();
    blocks.retain(|b| in_dram2(b.address()));
    let freed_main = before - blocks.len();
    let kept_b = blocks.len();
    while take(&mut blocks) {}
    let new_b = blocks.len() - kept_b;
    let landed_b = blocks[kept_b..].iter().all(|b| !in_dram2(b.address()));
    let intact_b = blocks.iter().all(Block::intact);

    // C. Only the second region has room: free its blocks; every new block must land there.
    let before = blocks.len();
    blocks.retain(|b| !in_dram2(b.address()));
    let freed_second = before - blocks.len();
    let kept_c = blocks.len();
    while take(&mut blocks) {}
    let new_c = blocks.len() - kept_c;
    let landed_c = blocks[kept_c..].iter().all(|b| in_dram2(b.address()));
    let intact_c = blocks.iter().all(Block::intact);

    // D. Free everything: usage returns to where it was and a large block fits again.
    drop(blocks);
    drop(spans);
    let back = esp_alloc::HEAP.stats().current_usage;
    let largest = largest_free_kib();

    let ok = total > 8
        && in2 > 0
        && aligned
        && disjoint
        && intact_a
        && freed_main > 0
        && new_b == freed_main
        && landed_b
        && intact_b
        && freed_second > 0
        && new_c == freed_second
        && landed_c
        && intact_c
        && back == base;
    (
        ok,
        format!(
            "blocks={total} in_dram2={in2} aligned={aligned} disjoint={disjoint} intact_full={intact_a} main_used={main_used} dram2_used={second_used} | main_only: freed={freed_main} refilled={new_b} landed_in_main={landed_b} intact={intact_b} | dram2_only: freed={freed_second} refilled={new_c} landed_in_dram2={landed_c} intact={intact_c} | usage_base={base} usage_back={back} largest_free_kib={largest}"
        ),
    )
}
