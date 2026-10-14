//! ROM primitives: boot phase only, before application flash ownership.
//! Cache operations require execution in bootloader IRAM, not mapped flash.
#[cfg(feature = "esp32c3")]
const RTC_CONFIG: usize = 0x90;
#[cfg(feature = "esp32c3")]
const RTC_PROTECT: usize = 0xa8;
#[cfg(feature = "esp32s3")]
const RTC_CONFIG: usize = 0x98;
#[cfg(feature = "esp32s3")]
const RTC_PROTECT: usize = 0xb0;
const WDT_WKEY: u32 = 0x50D8_3AA1;

unsafe extern "C" {
    fn esp_rom_spiflash_read(src_addr: u32, data: *mut u32, len: u32) -> i32;
    fn esp_rom_spiflash_write(dest_addr: u32, data: *const u32, len: u32) -> i32;
    fn esp_rom_spiflash_erase_sector(sector_number: u32) -> i32;
    fn esp_rom_spiflash_unlock() -> i32;
    fn esp_rom_spiflash_attach(config: u32, legacy: bool);
    fn esp_rom_spiflash_config_param(
        device_id: u32,
        chip_size: u32,
        block_size: u32,
        sector_size: u32,
        page_size: u32,
        status_mask: u32,
    ) -> u32;
    fn ets_efuse_get_spiconfig() -> u32;
    fn esp_rom_delay_us(us: u32);

    static rom_spiflash_legacy_data: *mut [u32; 6];

    fn Cache_MMU_Init();
    fn Cache_Enable_ICache(autoload: u32);
    fn Cache_Suspend_ICache() -> u32;
    fn Cache_Resume_ICache(autoload: u32);
    fn Cache_Invalidate_ICache_All();
    fn Cache_Ibus_MMU_Set(
        ext_ram: u32,
        vaddr: u32,
        paddr: u32,
        psize: u32,
        num: u32,
        fixed: u32,
    ) -> i32;
    fn Cache_Dbus_MMU_Set(
        ext_ram: u32,
        vaddr: u32,
        paddr: u32,
        psize: u32,
        num: u32,
        fixed: u32,
    ) -> i32;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlashError {
    Read(u32),
    Program(u32),
    Erase(u32),
    Unlock,
}

/// ROM flash read accepting arbitrary byte alignment.
/// # Safety
/// Call only in the single-core second-stage boot phase, executing from
/// bootloader IRAM, with interrupts/concurrent flash owners absent. Mapping
/// addresses must come from validated image segments and preserve boot memory.
pub unsafe fn flash_read(offset: u32, out: &mut [u8]) -> Result<(), FlashError> {
    let mut done = 0usize;
    while done < out.len() {
        let pos = offset + done as u32;
        let start = pos & !3;
        let skip = (pos - start) as usize;
        let want = (out.len() - done).min(252);
        let words = (skip + want).div_ceil(4);
        let mut window = [0u32; 64];
        // SAFETY: window is word aligned and large enough for words*4.
        let rc = unsafe { esp_rom_spiflash_read(start, window.as_mut_ptr(), (words * 4) as u32) };
        if rc != 0 {
            return Err(FlashError::Read(start));
        }
        // SAFETY: initialized local u32 window; byte view stays within it.
        let bytes = unsafe { core::slice::from_raw_parts(window.as_ptr().cast::<u8>(), words * 4) };
        out[done..done + want].copy_from_slice(&bytes[skip..skip + want]);
        done += want;
    }
    Ok(())
}

/// Program aligned bytes through the selected ROM flash driver.
/// # Safety
/// Call only in the single-core second-stage boot phase, executing from
/// bootloader IRAM, with interrupts/concurrent flash owners absent. Mapping
/// addresses must come from validated image segments and preserve boot memory.
pub unsafe fn flash_program(at: u32, data: &[u8]) -> Result<(), FlashError> {
    if at & 3 != 0 || data.len() & 3 != 0 {
        return Err(FlashError::Program(at));
    }
    let mut done = 0usize;
    while done < data.len() {
        let n = (data.len() - done).min(256);
        let mut words = [0u32; 64];
        for (word, chunk) in words.iter_mut().zip(data[done..done + n].chunks_exact(4)) {
            *word = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        }
        let dest = at + done as u32;
        // SAFETY: aligned words contains all n initialized bytes passed to ROM.
        if unsafe { esp_rom_spiflash_write(dest, words.as_ptr(), n as u32) } != 0 {
            return Err(FlashError::Program(dest));
        }
        done += n;
    }
    Ok(())
}

/// # Safety
/// Call only in the single-core second-stage boot phase, executing from
/// bootloader IRAM, with interrupts/concurrent flash owners absent. Mapping
/// addresses must come from validated image segments and preserve boot memory.
pub unsafe fn flash_unlock() -> Result<(), FlashError> {
    // SAFETY: selected ROM symbol, boot phase has exclusive flash access.
    if unsafe { esp_rom_spiflash_unlock() } == 0 {
        Ok(())
    } else {
        Err(FlashError::Unlock)
    }
}

/// # Safety
/// Call only in the single-core second-stage boot phase, executing from
/// bootloader IRAM, with interrupts/concurrent flash owners absent. Mapping
/// addresses must come from validated image segments and preserve boot memory.
pub unsafe fn flash_erase_sector(sector_number: u32) -> Result<(), FlashError> {
    // SAFETY: selected ROM driver, no mapped application is executing.
    if unsafe { esp_rom_spiflash_erase_sector(sector_number) } == 0 {
        Ok(())
    } else {
        Err(FlashError::Erase(sector_number))
    }
}

fn flash_size_from_header() -> Result<u32, FlashError> {
    let mut header = [0u8; 4];
    // SAFETY: internal helper called only by the boot-phase entry points.
    unsafe {
        flash_read(0, &mut header)?;
    }
    Ok(match header[3] >> 4 {
        0 => 1 << 20,
        1 => 2 << 20,
        2 => 4 << 20,
        3 => 8 << 20,
        4 => 16 << 20,
        _ => 4 << 20,
    })
}

/// Update the ROM driver's flash-size bound from the boot image header.
/// # Safety
/// Call only in the single-core second-stage boot phase, executing from
/// bootloader IRAM, with interrupts/concurrent flash owners absent. Mapping
/// addresses must come from validated image segments and preserve boot memory.
pub unsafe fn set_flash_size_from_header() -> Result<u32, FlashError> {
    let size = flash_size_from_header()?;
    // SAFETY: ROM initialized its legacy driver descriptor before second stage.
    unsafe { (*rom_spiflash_legacy_data)[1] = size };
    Ok(size)
}

/// Re-attach and configure flash after a failed early ROM read.
/// # Safety
/// Call only in the single-core second-stage boot phase, executing from
/// bootloader IRAM, with interrupts/concurrent flash owners absent. Mapping
/// addresses must come from validated image segments and preserve boot memory.
pub unsafe fn flash_reinit() -> u32 {
    let chip_size = flash_size_from_header().unwrap_or(4 << 20);
    // SAFETY: boot phase uses ROM eFuse SPI configuration and driver geometry.
    unsafe {
        esp_rom_spiflash_attach(ets_efuse_get_spiconfig(), false);
        esp_rom_spiflash_config_param(0, chip_size, 0x1_0000, 0x1000, 0x100, 0xFFFF);
    }
    chip_size
}

/// Clear the ROM flash-boot watchdog mode and return previous registers.
/// # Safety
/// Call only in the single-core second-stage boot phase, executing from
/// bootloader IRAM, with interrupts/concurrent flash owners absent. Mapping
/// addresses must come from validated image segments and preserve boot memory.
pub unsafe fn clear_flashboot_watchdogs() -> (u32, u32) {
    use core::ptr::{read_volatile, write_volatile};

    unsafe fn clear_bit(base: usize, config: usize, protect: usize, bit: u32) -> u32 {
        let cfg = (base + config) as *mut u32;
        let wprotect = (base + protect) as *mut u32;
        // SAFETY: caller supplies selected chip MMIO register offsets.
        let before = unsafe { read_volatile(cfg) };
        // SAFETY: same registers, watchdog key unlocks the protected write.
        unsafe {
            write_volatile(wprotect, WDT_WKEY);
            write_volatile(cfg, before & !(1 << bit));
            write_volatile(wprotect, 0);
        }
        before
    }

    // SAFETY: aligned TIMG0/RTC registers; RTC offsets selected per chip.
    unsafe {
        (
            clear_bit(0x6001_F000, 0x48, 0x64, 14),
            clear_bit(0x6000_8000, RTC_CONFIG, RTC_PROTECT, 12),
        )
    }
}

/// Initialize cache/MMU and suspend I-cache while mappings are changed.
/// # Safety
/// Call only in the single-core second-stage boot phase, executing from
/// bootloader IRAM, with interrupts/concurrent flash owners absent. Mapping
/// addresses must come from validated image segments and preserve boot memory.
pub unsafe fn cache_begin_mapping() -> u32 {
    // SAFETY: caller guarantees boot IRAM execution and exclusive mappings.
    unsafe {
        Cache_MMU_Init();
        Cache_Enable_ICache(0);
        Cache_Suspend_ICache()
    }
}

/// # Safety
/// Call only in the single-core second-stage boot phase, executing from
/// bootloader IRAM, with interrupts/concurrent flash owners absent. Mapping
/// addresses must come from validated image segments and preserve boot memory.
pub unsafe fn map_irom(vaddr: u32, paddr: u32, pages: u32) -> i32 {
    // SAFETY: caller validated image virtual/physical page ranges.
    unsafe { Cache_Ibus_MMU_Set(0, vaddr, paddr, 64, pages, 0) }
}

/// # Safety
/// Call only in the single-core second-stage boot phase, executing from
/// bootloader IRAM, with interrupts/concurrent flash owners absent. Mapping
/// addresses must come from validated image segments and preserve boot memory.
pub unsafe fn map_drom(vaddr: u32, paddr: u32, pages: u32) -> i32 {
    // SAFETY: caller validated image virtual/physical page ranges.
    unsafe { Cache_Dbus_MMU_Set(0, vaddr, paddr, 64, pages, 0) }
}

/// # Safety
/// Call only in the single-core second-stage boot phase, executing from
/// bootloader IRAM, with interrupts/concurrent flash owners absent. Mapping
/// addresses must come from validated image segments and preserve boot memory.
pub unsafe fn cache_finish_mapping(autoload: u32) {
    // SAFETY: caller guarantees boot IRAM execution and exclusive mappings.
    unsafe {
        Cache_Invalidate_ICache_All();
        Cache_Resume_ICache(autoload);
    }
}

pub fn delay_us(us: u32) {
    // SAFETY: delay ROM routine accepts any duration.
    unsafe { esp_rom_delay_us(us) };
}
