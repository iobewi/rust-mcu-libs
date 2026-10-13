#![no_std]
//! ESP second-stage ROM bindings and chip image-validation memory maps.
#[cfg(all(feature = "esp32c3", feature = "esp32s3"))]
compile_error!("select exactly one ESP boot target feature");
#[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
pub mod hw;
pub mod layout;
#[cfg(feature = "esp32c3")]
pub const MEMORY_MAP: iobewi_firmware_image::esp::MemoryMap =
    iobewi_firmware_image::esp::MemoryMap {
        chip_id: 5,
        drom: 0x3c000000..0x3c800000,
        irom: 0x42000000..0x42800000,
        iram: 0x4037c000..0x403e0000,
        dram: 0x3fc80000..0x3fce0000,
        rtc: 0x50000000..0x50002000,
        sram_alias_offset: 0x00700000,
        boot_window: 0x3fccb000..0x3fce0000,
        mmu_page: 0x10000,
    };
#[cfg(feature = "esp32s3")]
pub const MEMORY_MAP: iobewi_firmware_image::esp::MemoryMap =
    iobewi_firmware_image::esp::MemoryMap {
        chip_id: 9,
        drom: 0x3c000000..0x3d000000,
        irom: 0x42000000..0x42800000,
        iram: 0x40370000..0x403e0000,
        dram: 0x3fc88000..0x3fd00000,
        rtc: 0x600fe000..0x60100000,
        sram_alias_offset: 0x006f0000,
        boot_window: 0x3fcc8000..0x3fd00000,
        mmu_page: 0x10000,
    };
