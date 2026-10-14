# ESP ConfigSpace Flash

`EspConfigFlash` implements the upstream `ekv::flash::Flash` trait on the existing
`SharedFlash`. It discovers a named `data/0x40` partition, validates 4 KiB erase
geometry, 4-byte read/write support, 32–256 pages and physical flash bounds.
Discovery also reuses the existing full-map validator to reject duplicate labels,
overlaps, out-of-flash entries and overlap with the reserved 0..0x9000 region.
This adapter targets the standard reference table at 0x8000; relocated tables
are not a supported profile. Every read/write/erase checks its page and offset, then holds the shared physical
mutex for that one operation. No second FlashStorage or physical mutex exists.

The portable `iobewi_config_space::storage::Store` owns EKV, the complete space
manifest, capacity admission, record generations and persistent tombstones.
The application constructs exactly one Store for a region and distributes its
copyable backend handles through ConfigManager. The logical writer mutex covers
generation read + EKV transaction + readback, independently of physical locking.

```rust
let flash = EspConfigFlash::from_label(shared, "config").await?;
let blank = flash.is_blank().await?;
let store = Store::new(flash, random_seed, SPACES)?;
if blank { store.format().await?; } // explicit first-boot policy
store.mount().await?;              // never reformat on error
```

`is_blank` reads the entire region without mutation. The composition root decides
whether to initialize a virgin development partition. A nonblank invalid region
causes an error; this adapter provides no rescue or migration. Generations,
quotas, serialization and recovery policies are not duplicated here.

EKV power-fail recovery has a known upstream defect tracked in repository issue
#70. Development proceeds by explicit maintainer decision; no power-loss
qualification is claimed. Geometry tests run on host; MCU profiles compile the
real adapter. Reference examples link on C3/S3 and still need physical testing.
