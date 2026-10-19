# Experiment: a second heap in DRAM2 (ESP32-S3, no PSRAM)

Branch `exp/21-dram2`. Nothing here changes a product. Question: can the 73 744 bytes of DRAM that the 2nd stage bootloader frees (`dram2_seg`) serve as a **second, separate** heap region, so that a firmware keeps its main heap and its 96 KiB audio ring and still has room for the TLS client (peak about 120 KiB for the client alone, see the README)?

## Phase 1: the memory plan, checked

Sources: `esp-hal` 1.2.2 `ld/esp32s3/memory.x` and `ld/sections/{dram2,stack}.x`, `esp-bootloader-esp-idf` 0.6.0, `esp-hal-procmacros` 0.23.0, `esp-alloc` 0.11.0; the ELFs of the example (built with `dram2`) and of the StreamBeWI image of the 2026-10-16 campaign; the image segments written to flash.

| Zone | Address range | Size | Use |
| --- | --- | --- | --- |
| `dram_seg` | 0x3FC88000 .. 0x3FCDB700 | 341 760 B | `.data`, `.bss`, `.noinit`, then the main stack; its top is 0x3FCDB700 |
| `dram2_seg` | 0x3FCDB700 .. 0x3FCED710 | 73 744 B (0x12010) | `.dram2_uninit` only (NOLOAD) |
| ROM-reserved | 0x3FCED710 .. 0x3FCF0000 | 10 480 B | not in any segment: left alone |
| `dcache_reclaimed_seg` | 0x3FCF0000 .. 0x3FD00000 - data cache size | depends on the cache size | another candidate, **not tried** |

Findings:

1. **No overlap.** The stack grows down from 0x3FCDB700, the first byte of `dram2_seg`; it cannot reach it. In the built image `.stack` ends at 0x3FCDB700 and `.dram2_uninit` spans exactly 0x3FCDB700 .. 0x3FCED710. The flash image has 5 segments and none loads anything between those addresses (the `NOLOAD` section produces no data); the ELF's extra `LOAD` header there is memory only (`NOBITS`: the image has no segment for it).
2. **Lifecycle.** The 2nd stage bootloader runs from this region (the ROM loads it at 0x3FCE2820 and following) and is finished when the application runs; esp-hal documents the region as "available after the 2nd stage bootloader is finished" and `esp-bootloader-esp-idf` as usable for dynamic allocation, **uninitialized**. The attribute `#[esp_hal::ram(reclaimed)]` refuses to compile without the ESP-IDF bootloader. The board's bootloader is the ESP-IDF one ("ESP-IDF v6.1-beta1 2nd stage bootloader" in the boot log). A firmware started by another bootloader (the repository has its own, `iobewi-esp-bootloader`) would need this checked again.
3. **Other users.** No `.dram2_uninit` section exists in the example's or StreamBeWI's ELFs; no radio, RTOS or PHY crate of the build refers to `dram2`.
4. **Allocator.** `esp-alloc` takes up to 3 regions. An allocation tries the regions in the order they were added and falls to the next when one cannot satisfy it; a free finds its region by address. Regions are never merged: the largest block is bounded by the biggest region. All regions added here are `Internal`.
5. **Comparison of the two memory plans.**

| | HTTPS example (this branch) | StreamBeWI (image of 2026-10-16) |
| --- | --- | --- |
| Main heap | 80 or 96 KiB | 96 KiB (80 KiB on `origin/main`) |
| Audio ring | none | 96 KiB static (`STREAM`, 98 376 B) |
| Static `.data` + `.bss` | about 188 KiB with a 96 KiB heap | about 247 KiB |
| Main stack reserved | 142 208 B (96 KiB heap) | 44 424 B |
| `dram2_seg` used | 73 744 B in the experiment | 0 |

The second region is above the stack, so adding it takes nothing from the stack or from the ring.

**Not all of the 73 744 bytes can be assumed usable**: the allocator's own bookkeeping and the block granularity take some. Measured, not assumed: see below.

## Phase 2: the prototype

`add_dram2()` in `src/platform.rs`: a `static mut DRAM2: MaybeUninit<[u8; 73_744]>` in `#[esp_hal::ram(reclaimed)]`, handed to `esp_alloc::HEAP.add_region` as its own region. The main heap is untouched; no single interval is made from the two. Features: `dram2` (main region first), `dram2-first` (DRAM2 first, so that the radio, the network stack and the crypto allocate there first), `no-memtest`, `cpu-max`.

Checks added to the example:

- `DRAM2 boot:` logs the address, the length, the alignment and a checksum of the region **before** the allocator takes it: the RAM is not cleared by a reset, so the contents are the bootloader's and the previous run's. Nothing may assume zeros.
- `mem-integrity` (before the radio starts, since exhausting the heap under a live driver would test the driver): fills the heap with 4 KiB blocks carrying a pattern until the allocator refuses, checks alignment, disjointness and the patterns; frees the blocks of the main region and refills (each new block must land in the main region, the others must stay intact); the same with the second region; frees everything and requires the usage to return to exactly its initial value.
- Per-region figures (`MEM ...: region main|dram2`), a 10 ms sampler of the peak usage per region, and the longest time the executor was kept from running (`executor_max_lateness_ms`).

## Phase 3: results on the board (Metronic 477144, ESP32-S3)

Traces: `traces/dram2/`. Every run: the 23 scenarios of the README, plus `mem-integrity`. The board's `factory` and `config` partitions were backed up beforehand and read back identical afterwards.

| Configuration | Total heap | Runs | Result |
| --- | --- | --- | --- |
| main 80 KiB, DRAM2 after | 155 664 B | 3 | 23/23, integrity PASS, each |
| main 96 KiB, DRAM2 after | 172 048 B | 2 (+2 earlier) | 23/23, integrity PASS, each |
| main 96 KiB, DRAM2 **first** | 172 048 B | 2 | 23/23, integrity PASS, each |
| main 80 KiB, DRAM2 **first** | 155 664 B | 1 | 23/23, integrity PASS |
| main 80 KiB, DRAM2 after, no integrity test (exact peak) | 155 664 B | 1 | 23/23 |
| same, CPU at maximum clock | 155 664 B | 1 | 23/23 |

- **Usable size.** The allocator takes the whole 73 744 bytes as a region. With 4 KiB blocks 18 fit (73 728 B) when the region is filled by itself, 16 (65 536 B) when it is the first region and also holds the radio, the stack and the crypto at that moment. Largest contiguous block of the whole heap after a run: 72 KiB (main 80 or 96 KiB with DRAM2 after: the main region is occupied by the 48 KiB baseline and the second region is the largest free area), 80 or 96 KiB when the main region is the free one (DRAM2 first).
- **Integrity.** In all runs: blocks aligned and disjoint, patterns intact after each phase, refills land in the region that has room, usage back to exactly the initial value. Exhaustion is clean (`try_reserve` returns an error, no panic, no corruption); one region exhausted, the other keeps serving.
- **Peak.** Exact (allocator, no integrity test): **119 940 B** at the default clock, 115 248 B at maximum clock (timing of allocations differs), the same as the single 160 KiB region. With main = 80 KiB the second region therefore holds at least 119 940 - 81 920 = 38 020 B at the peak (the 10 ms sampler saw 30 168 B: it misses short peaks and is only a lower bound). With DRAM2 first it fills to 73 616 .. 73 688 of 73 744 B, and the rest of the peak goes to the main region.
- **DMA and hardware.** With DRAM2 first (3 runs), at the end of every run the 48 016 B in use are all in DRAM2 and none in the main region: the radio driver, the network stack, the config store and the crypto state were allocated there, and Wi-Fi association, DHCP, DNS, TCP, SNTP and the 131 KB TLS download all worked. USB is not in this build and is not tested.
- **Restart.** Each run starts after a reset (the bench resets when the Wi-Fi join fails, so some runs have two boots): the checksum of the region at boot differs from boot to boot and between 187 and 192 of its 18 436 words are zero: it holds leftovers, not zeros.
- **No drift.** `reconnect-heap` after 10 close/reconnect cycles and a 3 s rest: back to 48 016 B in every run, or lower (the first handshakes of a run leave a block that is released later; the drift was 0 or negative in the 8 runs of the table above; the check waits 3 s because an 8 520-byte block can still be held right after the loop).
- **Stack margin** (the example's stack is much larger than the product's, so only the high-water mark transfers): 16 112 .. 16 912 B in every run.

### The finding that matters for the product: the executor is blocked during a handshake

`executor_max_lateness_ms` is the longest the 10 ms heartbeat task was late, that is the time some task kept the (single) executor without yielding. A TLS handshake is CPU work done inside one poll:

| Handshake (80 MHz, default clock) | Executor blocked |
| --- | --- |
| first RSA (TLS 1.2) | 1 054 ms |
| Radio France (TLS 1.3, RSA-PSS), each of the 11 | about 760 ms |
| certificate refused | 240 .. 750 ms |
| ECDSA P-384 | 3 508 ms |

At the maximum CPU clock, as StreamBeWI runs: first RSA 358 ms, Radio France about 260 .. 270 ms, refusals 83 .. 248 ms (the P-384 figure of that run was not captured). In StreamBeWI the USB MSC task shares an executor with the network and the stream. A 260 ms (or longer: P-384, a slower CPU) stall at every connection and reconnection is what the simultaneous qualification must measure on the MSC read path, with the product's read budget and soft-disconnect policy, before any claim of stability. If it is not acceptable the handshake must run elsewhere (the second core, or an executor of its own), which is a design change this experiment does not make.

## Conclusion of the experiment

Demonstrated on the board: the second region is safe (no overlap, disjoint from the main heap, correct in alignment, exhaustion and reuse, usable by the radio and the DMA-using stack, stable over repeated runs and restarts) and gives the client what it needs: 80 KiB + DRAM2 passes the 23 scenarios like the 160 KiB single region, with the ring and the stack untouched.

Not demonstrated: anything about StreamBeWI itself (not modified, not measured with this region); USB MSC during a handshake (and the stall above makes it the first thing to test); the `dcache_reclaimed` region; another bootloader.
