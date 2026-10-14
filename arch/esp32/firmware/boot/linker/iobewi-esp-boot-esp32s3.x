/*
 * iobewi-esp-bootloader memory layout (ESP32-S3).
 *
 * Mirrors iobewi-esp-boot-esp32c3.x's approach exactly; only the addresses
 * change, because ESP32-S3's usable internal SRAM is larger. The ROM loads
 * the image found at flash offset 0 -- RAM segments only -- and jumps to its
 * entry point.
 *
 * SRAM is visible through two aliases: instruction bus 0x40370000.. and data
 * bus 0x3fc88000.., offset 0x6f0000 (esp-idf's `SOC_I_D_OFFSET` for
 * ESP32-S3 -- NOT the same constant as ESP32-C3's 0x700000). Code goes in
 * the IRAM alias, data and the stack in the DRAM alias, and the two regions
 * below are chosen so they do NOT cover the same physical bytes:
 *
 *   vectors_seg 0x403cb000..0x403cb400, IRAM 0x403cb400..0x403d4000
 *     <-> DRAM alias 0x3fcdb000..0x3fce4000
 *   DRAM 0x3fce4000..0x3fcec000  (data, bss, stack growing down from the end)
 *
 * The DRAM window must end at or below 0x3fced710, the start of the ROM's own
 * data (the same rule as ESP32-C3's "ROM keeps its own data above 0x3fcdc710",
 * and the stock ESP-IDF bootloader's segments -- 0x3fcd8700..0x3fcde654 and
 * 0x3fce2820..0x3fce3cf0 -- stay inside it too). Above it:
 *
 *   0x3fced710..0x3fcf0000  ROM data (rom_spiflash_legacy_data 0x3fceffe4, ...)
 *   0x3fcf0000..0x3fd00000  the data cache takes the HIGH end of this range
 *                           (esp-hal's `esp32_init`, which runs before any
 *                           Rust code, configures 32 KiB: 0x3fcf8000..0x3fd00000;
 *                           up to 64 KiB would start at 0x3fcf0000)
 *
 * The first ESP32-S3 layout put DRAM at 0x3fcf4000..0x3fcfc000: the whole
 * stack (it starts at the top, 0x3fcfc000) sat inside the D-cache window, so
 * `esp32_init` pulled the memory out from under the running stack and the very
 * next return landed in a DoubleException before anything was printed. Hence
 * DRAM now sits directly above the IRAM alias window, below the ROM data.
 *
 * The application image is loaded at the bottom of SRAM (0x40370000 /
 * 0x3fc88000 upwards); `main.rs` refuses any segment reaching this window.
 */
MEMORY
{
  /* Xtensa's vector table (xtensa-lx-rt's `exception.x`) is a fixed 0x400-byte,
   * hardware-aligned block that must land in a region literally named
   * `vectors_seg`; carved off the front of the same IRAM window so the vector
   * table and the naked exception handlers it jumps to (call0, short range)
   * stay adjacent. */
  vectors_seg (RX)  : ORIGIN = 0x403cb000, LENGTH = 0x400
  IRAM        (RWX) : ORIGIN = 0x403cb400, LENGTH = 0x9000 - 0x400
  DRAM        (RW)  : ORIGIN = 0x3fce4000, LENGTH = 0x8000
  RTC_FAST    (RWX) : ORIGIN = 0x600fe000, LENGTH = 0x2000
  /* ESP32-S3's second, larger RTC bank (unlike ESP32-C3, which has only the
   * one above). Declared -- `rtc_slow.x`'s sections need a real region of
   * this exact name to place their (here, empty) output into -- but not
   * used for anything: nothing in this bootloader targets `#[ram(rtc_slow)]`. */
  rtc_slow_seg (RWX) : ORIGIN = 0x50000000, LENGTH = 0x2000
}

/* esp-rom-sys 0.1.4/0.1.5's ESP32-S3 `additional.ld` provides this ROM
 * function only as `rom_Cache_Suspend_ICache`, not the plain name
 * `iobewi_esp_boot::esp32s3::hw` (and the ESP32-C3 module, and every other ROM
 * function on both chips) calls directly. Address confirmed against a newer
 * esp-hal revision's `esp32s3.rom.ld`, which does provide the plain name at
 * the same address -- this is a naming gap in the pinned crate version, not
 * an uncertain address. */
PROVIDE(Cache_Suspend_ICache = 0x4000189c);

/* esp-hal's section files place things by these logical names; point all of
 * them at RAM (rodata goes to the *data* alias: byte loads from the
 * instruction alias are not safe). */
REGION_ALIAS("ROTEXT", IRAM);
REGION_ALIAS("RODATA", DRAM);
REGION_ALIAS("RWDATA", DRAM);
REGION_ALIAS("RWTEXT", IRAM);
REGION_ALIAS("RTC_FAST_RWTEXT", RTC_FAST);
REGION_ALIAS("RTC_FAST_RWDATA", RTC_FAST);
REGION_ALIAS("RTC_SLOW_RWTEXT", rtc_slow_seg);
REGION_ALIAS("RTC_SLOW_RWDATA", rtc_slow_seg);

/* Xtensa needs its own vector table; RISC-V's script has no equivalent of
 * this include. Provided by xtensa-lx-rt, placed like esp-hal's own
 * esp32s3.x does it. */
INCLUDE exception.x

/* No flash-mapped sections, hence no `.rotext_dummy`/`.rwdata_dummy` tricks
 * (esp32s3.x): with disjoint IRAM/DRAM windows nothing overlaps. */
SECTIONS {
  INCLUDE "rwtext.x"
  INCLUDE "rwdata.x"
}
INCLUDE "rodata.x"
INCLUDE "text.x"
INCLUDE "rtc_fast.x"
/* xtensa-lx-rt's Reset handler unconditionally zeroes/initializes rtc_slow's
 * bss/persistent regions regardless of whether anything is placed there;
 * include it (onto the RTC_SLOW_* alias above) purely so those symbols
 * exist, not because this bootloader uses rtc_slow for anything. */
INCLUDE "rtc_slow.x"
INCLUDE "stack.x"
INCLUDE "metadata.x"
INCLUDE "eh_frame.x"

INCLUDE "hal-defaults.x"

/* A bad layout must fail the link, not the hardware: a stack that the data
 * cache takes over, or a window that overlaps the ROM's data, crashes before
 * the first line of output with nothing to diagnose. */
ROM_DATA_START = 0x3fced710;
IRAM_ALIAS_OFFSET = 0x6f0000;
ASSERT(ORIGIN(DRAM) + LENGTH(DRAM) <= ROM_DATA_START,
  "bootloader DRAM (data + stack) must end below the ROM data at 0x3fced710 (and so below the D-cache window)");
ASSERT(ORIGIN(DRAM) >= ORIGIN(IRAM) + LENGTH(IRAM) - IRAM_ALIAS_OFFSET,
  "bootloader DRAM overlaps the physical SRAM behind the IRAM window");
ASSERT(_stack_start <= ROM_DATA_START, "initial SP is above the ROM data / inside the D-cache window");
ASSERT(_stack_start % 16 == 0, "initial SP must be 16-byte aligned");
ASSERT(_stack_start - _stack_end >= 0x2000, "bootloader stack smaller than 8 KiB");
ASSERT(Reset >= ORIGIN(IRAM) && Reset < ORIGIN(IRAM) + LENGTH(IRAM), "entry point is not in IRAM");
ASSERT(_bss_end <= _stack_end, "bss runs into the stack");
