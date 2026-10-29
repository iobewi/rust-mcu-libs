//! ESP second-stage bootloader for supported ESP targets.
//!
//!   ROM -> iobewi-esp-bootloader -> the slot IOBEWI OTA selects -> application
//!
//! Every *decision* -- which slot, what to write to `otadata`, whether an image
//! is bootable -- is `iobewi-firmware-boot` (transactional boot metadata) with `iobewi-firmware-image` (ESP image validation), tested on the host against power cuts.
//! This file only performs them on the real flash and hands over control.
//!
//! What it does today (boot chain step 5):
//! * clears the ROM's "flash boot" watchdog protection (see below);
//! * reads the partition table, then the two `otadata` entries;
//! * asks `plan_boot` what to do. A blank `otadata` is a first boot: slot 0 is
//!   validated, then `Valid(seq=1)` is written by the protocol below, and only
//!   then does it boot. Anything unexpected halts -- it never guesses, and
//!   `otadata` that is not blank but holds no accepted entry (torn, corrupt or
//!   unknown format) is never re-seeded;
//! * loads the chosen image (RAM segments copied, DROM/IROM mapped through the
//!   flash MMU) and jumps.
//!
//! Structure-only image checks preserve the historical boot policy. Application
//! startup must rearm the pending-image watchdog after HAL/runtime init.
#![no_std]
#![no_main]

#[cfg(not(any(feature = "esp32c3", feature = "esp32s3")))]
compile_error!("select a supported ESP boot target feature");
#[cfg(all(feature = "esp32c3", feature = "esp32s3"))]
compile_error!("select exactly one ESP boot target feature");

use iobewi_firmware_boot as boot_core;

#[cfg(not(feature = "verified-boot"))]
use boot_core::plan_boot;
use boot_core::{Boot, Halt, Raw, Write};
use esp_println::Printer;
use iobewi_esp_boot::hw;
use iobewi_firmware_image::esp::{self as image, MemoryMap, Verify};
/// What `log!` can print. Text and hex only, on purpose: `core::fmt` (Debug,
/// padding, Unicode tables) costs ~10 KiB, and the whole bootloader has to
/// fit in the 32 KiB in front of the partition table.
trait Loggable {
    fn put(&self);
}

impl Loggable for &str {
    fn put(&self) {
        Printer::write_bytes(self.as_bytes());
    }
}

/// Printed as `0x` + 8 hex digits.
impl Loggable for u32 {
    fn put(&self) {
        let mut out = *b"0x00000000";
        for i in 0..8 {
            let nibble = ((*self >> ((7 - i) * 4)) & 0xF) as u8;
            out[2 + i] = if nibble < 10 {
                b'0' + nibble
            } else {
                b'a' + nibble - 10
            };
        }
        Printer::write_bytes(&out);
    }
}

macro_rules! log {
    ($($part:expr),+ $(,)?) => {{
        $( Loggable::put(&$part); )+
        Printer::write_bytes(b"\r\n");
    }};
}

// --- Flash layout (embewi-ab-v1, see partitions.csv) -----------------------

/// Where the partition table lives (the ESP-IDF default, kept by espflash).
const PARTITION_TABLE_OFFSET: u32 = 0x8000;
const PARTITION_TABLE_LEN: usize = 0xC00;
const SECTOR: u32 = 0x1000;
const SLOT_COUNT: u8 = 2;

fn memory_map() -> MemoryMap {
    iobewi_esp_boot::MEMORY_MAP
}

// ROM/MMU/watchdog access is provided by iobewi-esp-boot, per chip.

enum BootError {
    FlashRead(u32),
    /// Missing, ambiguous or physically invalid layout.
    InvalidLayout,
    /// `plan_boot` says there is nothing safe to boot: 1 = first boot, slot 0 unbootable; 2 = no usable entry; 3 = `otadata` is neither blank nor holds an accepted entry.
    Halt(u32),
    /// Physical metadata transaction failed: driver, bounds, readback or commit.
    OtadataWrite {
        reason: u32,
        at: u32,
    },
    /// The chosen image failed validation when re-read to be loaded.
    Image,
    Mmu(i32),
    /// Verified boot: flash reads kept failing while checking a slot. Nothing was written.
    #[cfg(feature = "verified-boot")]
    Io,
}

/// Reads `out.len()` bytes at flash offset `offset`. The ROM reader wants
/// word-aligned offsets, buffers and lengths, so this reads aligned windows
/// and copies the requested bytes out of them.
fn flash_read(offset: u32, out: &mut [u8]) -> Result<(), BootError> {
    // SAFETY: sole ROM boot phase; flash ranges/images are validated before load.
    unsafe { hw::flash_read(offset, out) }.map_err(|e| match e {
        hw::FlashError::Read(at) => BootError::FlashRead(at),
        _ => BootError::FlashRead(offset),
    })
}

/// The image validator reads through this.
struct RomFlash;

impl image::Read for RomFlash {
    fn read(&mut self, offset: u32, buf: &mut [u8]) -> Result<(), ()> {
        flash_read(offset, buf).map_err(|_| ())
    }
}

/// Tell the ROM flash driver how large the physical flash is.
fn set_flash_size() -> Result<u32, BootError> {
    // SAFETY: sole ROM boot phase; flash ranges/images are validated before load.
    let size = unsafe { hw::set_flash_size_from_header() }.map_err(|e| match e {
        hw::FlashError::Read(at) => BootError::FlashRead(at),
        _ => BootError::FlashRead(0),
    })?;
    log!("boot: flash size ", size);
    Ok(size)
}

/// Re-attach/configure flash after an early ROM read failure.
fn flash_reinit() {
    // SAFETY: sole ROM boot phase; flash ranges/images are validated before load.
    let chip_size = unsafe { hw::flash_reinit() };
    log!("boot: re-attaching flash, chip_size=", chip_size);
}

use iobewi_firmware_layout::domain_a::Layout;

fn read_layout(flash_size: u32) -> Result<Layout, BootError> {
    let mut table = [0u8; PARTITION_TABLE_LEN];
    if flash_read(PARTITION_TABLE_OFFSET, &mut table).is_err() {
        flash_reinit();
        flash_read(PARTITION_TABLE_OFFSET, &mut table)?;
    }
    iobewi_esp_boot::layout::discover(&table, flash_size).map_err(|_| BootError::InvalidLayout)
}

impl boot_core::transaction::Flash for RomFlash {
    type Error = hw::FlashError;
    fn read(&mut self, at: u32, out: &mut [u8]) -> Result<(), Self::Error> {
        // SAFETY: sole ROM boot phase; flash ranges/images are validated before load.
        unsafe { hw::flash_read(at, out) }
    }
    fn erase(&mut self, at: u32, len: u32) -> Result<(), Self::Error> {
        // SAFETY: sole ROM boot phase; flash ranges/images are validated before load.
        unsafe { hw::flash_unlock() }?;
        for sector in at / SECTOR..(at + len) / SECTOR {
            // SAFETY: sole ROM boot phase; flash ranges/images are validated before load.
            unsafe { hw::flash_erase_sector(sector) }?;
        }
        Ok(())
    }
    fn program(&mut self, at: u32, bytes: &[u8]) -> Result<(), Self::Error> {
        // SAFETY: sole ROM boot phase; flash ranges/images are validated before load.
        unsafe { hw::flash_program(at, bytes) }
    }
}
fn metadata(layout: &Layout) -> boot_core::transaction::Region {
    boot_core::transaction::Region {
        offset: layout.otadata.offset,
        size: layout.otadata.size,
    }
}
fn read_otadata(layout: &Layout) -> Result<[Raw; 2], BootError> {
    boot_core::transaction::read_entries(&mut RomFlash, metadata(layout))
        .map_err(|_| BootError::FlashRead(layout.otadata.offset))
}
fn execute(write: Write, layout: &Layout) -> Result<(), BootError> {
    boot_core::transaction::execute(&mut RomFlash, metadata(layout), write).map_err(|e| {
        use boot_core::transaction::Error;
        let reason = match e {
            Error::Flash(_) => 1,
            Error::BadPartition | Error::BadSector => 2,
            Error::ReadbackMismatch => 3,
            Error::CommitMismatch => 4,
        };
        BootError::OtadataWrite {
            reason,
            at: layout.otadata.offset + u32::from(write.sector) * SECTOR,
        }
    })
}

#[cfg(feature = "verified-boot")]
mod verified {
    include!(concat!(env!("OUT_DIR"), "/verified_config.rs"));
}

/// The bootloader's flash reader as the boot-proof verifier sees it.
#[cfg(feature = "verified-boot")]
struct ProofReader;

#[cfg(feature = "verified-boot")]
impl iobewi_firmware_bootauth::SlotRead for ProofReader {
    fn read(
        &mut self,
        offset: u32,
        out: &mut [u8],
    ) -> Result<(), iobewi_firmware_bootauth::IoError> {
        flash_read(offset, out).map_err(|_| iobewi_firmware_bootauth::IoError)
    }
}

/// Attempts at checking the slots before a persistent flash read error stops the boot.
#[cfg(feature = "verified-boot")]
const VERIFY_ATTEMPTS: u8 = 3;

/// `plan_boot` with the verified profile: every slot must carry a valid proof. A flash read
/// error never becomes an `Invalid` entry; after a few attempts the boot stops, writing nothing.
#[cfg(feature = "verified-boot")]
fn verified_plan(
    layout: &Layout,
    entries: [Raw; 2],
    map: &MemoryMap,
) -> Result<boot_core::Plan, BootError> {
    use iobewi_esp_boot::rom_verify::{RomRsaPss3072, RomSha256};
    use iobewi_firmware_auth::{ImageFormat, Mcu, Product, Target};
    use iobewi_firmware_bootauth::select::{self, Outcome, Structure};
    use iobewi_firmware_bootauth::{BootPolicy, VerifiedBoot};

    #[cfg(feature = "esp32c3")]
    const MCU: Mcu = Mcu::Esp32C3;
    #[cfg(feature = "esp32s3")]
    const MCU: Mcu = Mcu::Esp32S3;
    let target = Target {
        product: Product::new(verified::PRODUCT).ok_or(BootError::InvalidLayout)?,
        mcu: MCU,
        image_format: ImageFormat::EspApp,
        hw_variants: &verified::VARIANTS,
        min_security_version: verified::MIN_SECURITY_VERSION,
    };
    let policy =
        VerifiedBoot::<RomRsaPss3072, RomSha256>::new(target, &verified::PINS, RomRsaPss3072)
            .map_err(|_| BootError::InvalidLayout)?;
    log!(VerifiedBoot::<RomRsaPss3072, RomSha256>::LABEL);

    for _ in 0..VERIFY_ATTEMPTS {
        let mut structure = |slot: u8| {
            let part = layout.images[usize::from(slot)];
            match image::validate(
                &mut RomFlash,
                part.offset,
                part.usable_size(),
                map,
                Verify::Structure,
            ) {
                Ok(_) => Structure::Bootable,
                Err(image::ImageError::Read) => Structure::Io,
                Err(e) => Structure::Refused(image_error_code(&e)),
            }
        };
        // One terse line per refused slot; proof reasons are numbered from 100.
        let mut report = |slot: u8, outcome: Outcome| {
            let reason = match outcome {
                Outcome::Structure(code) => code,
                Outcome::Proof(refusal) => 100 + refusal.code(),
                Outcome::Io => 0,
            };
            log!("boot: slot ", u32::from(slot), " refused, reason ", reason);
        };
        match select::plan(
            entries,
            SLOT_COUNT,
            &policy,
            &mut ProofReader,
            layout.images,
            &mut structure,
            &mut report,
        ) {
            Ok(plan) => return Ok(plan),
            Err(select::Interrupted) => flash_reinit(),
        }
    }
    Err(BootError::Io)
}

fn boot() -> Result<core::convert::Infallible, BootError> {
    let flash_size = set_flash_size()?;
    let layout = read_layout(flash_size)?;
    let map = memory_map();
    log!(
        "boot: otadata at ",
        layout.otadata.offset,
        " ota_0 ",
        layout.images[0].offset,
        " ota_1 ",
        layout.images[1].offset
    );
    let entries = read_otadata(&layout)?;

    #[cfg(not(feature = "verified-boot"))]
    let plan = {
        log!("boot authentication: UNSIGNED");
        let mut image_ok = |slot: u8| {
            let part = layout.images[usize::from(slot)];
            // The last sector of the slot is its proof sector: never part of the image.
            let (offset, size) = (part.offset, part.usable_size());
            match image::validate(&mut RomFlash, offset, size, &map, Verify::Structure) {
                Ok(_) => true,
                Err(e) => {
                    log!(
                        "boot: slot ",
                        u32::from(slot),
                        " image refused, reason ",
                        image_error_code(&e)
                    );
                    false
                }
            }
        };
        plan_boot(entries, SLOT_COUNT, &mut image_ok)
    };
    #[cfg(feature = "verified-boot")]
    let plan = verified_plan(&layout, entries, &map)?;

    let (slot, seq) = match plan.boot {
        Boot::Slot { slot, seq, .. } => (slot, seq),
        Boot::Halt(Halt::NoImage) => return Err(BootError::Halt(1)),
        Boot::Halt(Halt::NoUsableEntry) => return Err(BootError::Halt(2)),
        Boot::Halt(Halt::UnknownMetadata) => return Err(BootError::Halt(3)),
    };
    log!("boot: plan slot=", u32::from(slot), " seq=", seq);

    for write in plan.writes() {
        log!(
            "boot: otadata write sector=",
            u32::from(write.sector),
            " seq=",
            write.entry.seq,
            " state=",
            write.entry.state
        );
        execute(write, &layout)?;
    }

    load(&layout, slot)
}

/// Loads slot `slot`'s image and jumps to it.
fn load(layout: &Layout, slot: u8) -> Result<core::convert::Infallible, BootError> {
    let map = memory_map();
    let part = layout.images[usize::from(slot)];
    // The last sector of the slot is its proof sector: never part of the image.
    let (offset, size) = (part.offset, part.usable_size());
    let image = image::validate(&mut RomFlash, offset, size, &map, Verify::Structure)
        .map_err(|_| BootError::Image)?;
    log!(
        "boot: image ok, segments=",
        image.count as u32,
        " entry=",
        image.entry
    );

    // Copy the RAM segments (validated: inside the map, clear of this bootloader).
    for seg in image.segments().iter().filter(|s| s.len > 0) {
        let Some(target) = seg.ram_target(&map) else {
            continue;
        };
        let dst = target as *mut u8;
        let mut chunk = [0u8; 256];
        let mut copied = 0u32;
        while copied < seg.len {
            let n = (seg.len - copied).min(chunk.len() as u32) as usize;
            flash_read(seg.data_offset + copied, &mut chunk[..n])?;
            // SAFETY: validated RAM targets exclude the bootloader, stack and chunk;
            // each copy stays inside the validated segment.
            unsafe { core::ptr::copy_nonoverlapping(chunk.as_ptr(), dst.add(copied as usize), n) };
            copied += n as u32;
        }
    }

    // Flash MMU + cache for the DROM/IROM segments.
    // SAFETY: sole ROM boot phase; flash ranges/images are validated before load.
    let autoload = unsafe { hw::cache_begin_mapping() };
    for seg in image.segments().iter().filter(|s| s.len > 0) {
        let is_drom = map.drom.contains(&seg.load);
        if !(is_drom || map.irom.contains(&seg.load)) {
            continue;
        }
        let vaddr = seg.load & !(map.mmu_page - 1);
        let paddr = seg.data_offset & !(map.mmu_page - 1);
        let pages = (seg.len + (seg.load - vaddr)).div_ceil(map.mmu_page);
        let rc = if is_drom {
            // SAFETY: sole ROM boot phase; flash ranges/images are validated before load.
            unsafe { hw::map_drom(vaddr, paddr, pages) }
        } else {
            // SAFETY: sole ROM boot phase; flash ranges/images are validated before load.
            unsafe { hw::map_irom(vaddr, paddr, pages) }
        };
        log!(
            "boot: map ",
            vaddr,
            " <- flash ",
            paddr,
            " pages=",
            pages,
            " rc=",
            rc as u32
        );
        if rc != 0 {
            return Err(BootError::Mmu(rc));
        }
    }
    // SAFETY: sole ROM boot phase; flash ranges/images are validated before load.
    unsafe { hw::cache_finish_mapping(autoload) };

    log!("boot: jump ", image.entry);
    // Let the USB-Serial-JTAG FIFO drain before the application reconfigures it.
    hw::delay_us(50_000);
    // SAFETY: entry validated in an executable range; segments have been
    // loaded/mapped and no borrowed application memory survives the jump.
    let entry: extern "C" fn() -> ! = unsafe { core::mem::transmute(image.entry as usize) };
    entry()
}

/// A stable number per refusal reason (no `Debug`: it costs code size).
fn image_error_code(e: &image::ImageError) -> u32 {
    use image::ImageError::*;
    match e {
        Read => 1,
        BadMagic(_) => 2,
        BadChip(_) => 3,
        BadSegmentCount(_) => 4,
        SegmentOutsidePartition(_) => 5,
        BadLoadRange(_) => 6,
        Misaligned(_) => 7,
        OverlapsBootloader(_) => 8,
        BadEntry => 9,
        Truncated => 10,
        BadChecksum => 11,
        HashMissing => 12,
        BadHash => 13,
    }
}

/// One line per failure, with the offending value where there is one.
fn report(e: &BootError) {
    match e {
        BootError::FlashRead(at) => log!("boot: FAILED flash read at ", *at),
        BootError::InvalidLayout => log!("boot: FAILED invalid partition layout"),
        BootError::Halt(why) => log!("boot: HALT, nothing safe to boot, reason ", *why),
        BootError::OtadataWrite { reason, at } => {
            log!("boot: FAILED otadata write reason ", *reason, " at ", *at)
        }
        BootError::Image => log!("boot: FAILED image no longer validates"),
        BootError::Mmu(rc) => log!("boot: FAILED MMU rc ", *rc as u32),
        #[cfg(feature = "verified-boot")]
        BootError::Io => log!("boot: HALT, flash read errors while verifying, nothing written"),
    }
}

/// Clear the ROM flash-boot watchdog mode before handing control to the application.
fn clear_flashboot_watchdogs() -> (u32, u32) {
    // SAFETY: sole ROM boot phase; flash ranges/images are validated before load.
    unsafe { hw::clear_flashboot_watchdogs() }
}

#[esp_hal::main]
fn main() -> ! {
    // First thing: the ROM's watchdogs are already ticking.
    let (tg0_wdt, rtc_wdt) = clear_flashboot_watchdogs();
    esp_hal::init(esp_hal::Config::default());
    log!("\r\niobewi-esp-bootloader ", env!("CARGO_PKG_VERSION"));
    log!(
        "boot: wdt tg0=",
        tg0_wdt,
        " rtc=",
        rtc_wdt,
        " (flashboot bits cleared)"
    );
    // `boot` only ever returns on failure (success ends in a jump).
    let Err(e) = boot();
    report(&e);
    // Nothing safe left to do; stay put so the message can be read.
    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    log!("boot: PANIC");
    loop {
        core::hint::spin_loop();
    }
}
