# Virtual FAT16

## Summary

Allocation-free, read-only FAT16 volume with one contiguous virtual file backed by a
caller-provided non-blocking sector source. Metadata generation is extracted from the
USB Radio golden implementation without embedding its product settings.

## Responsibilities

- Validate geometry and derive disk capacity and file sector mapping.
- Generate a boot sector, two identical FATs and a root directory with a volume label
  and one read-only archive file.
- Delegate file reads and session callbacks to the source.
- Preserve zero-filled output for unavailable reads and out-of-range addresses.

## Non-responsibilities

- No radio URL, MP3 processing, USB/SCSI, network, timers or synchronization.
- No stream retention, rebase policy, prebuffer threshold or far-ahead probe policy.
- No physical storage ownership, mounting an existing disk, writes or allocation.

## Architecture

Portable service at `common/fs/fat16`, implementing the `common/fs/block` read-only contract.
Products supply their own file source and choose volume identity and geometry.

## Public API

`Fat16Config` takes `sectors_per_cluster`, `file_cluster_count`, `root_entries`, an
encoded 11-byte FAT short `file_name`, 11-byte `volume_label` and `volume_serial`.
Names/labels are already encoded bytes supplied by the caller; encoding and character
validity are the caller's responsibility. No long filename support is provided.

`VirtualFat16::new(source, config)` returns a validated volume or `ConfigError`.
Sectors per cluster must be a power of two from 1 through 64. Data cluster count must
classify as FAT16 (at least 4085) and keep the last cluster below reserved ID 0xFFF0
(at most 65518 clusters). Root entries must be at least 16 and a multiple of 16.
The file fills every data cluster; its size is derived, not separately configurable.
Checked arithmetic protects derived file size and capacity. `Fat16Config::geometry`
and `VirtualFat16::geometry` expose the immutable derived `Geometry`.

`FileSource::read_file_sector` takes a file-relative sector index and returns `ReadStatus`.
`VirtualFat16` exposes inherent `last_lba`, `read_sector`, `begin_session`, `end_session`
and implements `ReadOnlyBlockDevice`. `ReadStatus` and `SECTOR_SIZE` are re-exported.
Metadata reads are always ready. Out-of-range reads return Expired with zero output.
Pending/Expired file reads also zero output even if the source changed the buffer.

## Lifecycle

Construction validates immutable metadata. Session callbacks pass to the source;
metadata is independent of session state. File contents may depend on a session or
change over time. A stream source may rebase file offset zero when a session begins.
Read-only does not promise that all advertised bytes are currently retained or available.
The caller decides whether to retry, substitute zeros or report an unavailable read.

## Validation

`cargo check -p iobewi-fat16` and `cargo test -p iobewi-fat16`.
Host tests compare every metadata sector byte-for-byte with independently generated
golden bytes, check complete FAT chains/copies, alternate configuration, validation
boundaries, file mapping, unavailable output and callback forwarding via the block trait.
This implementation does not access physical flash. Consumer hardware replay is
required before qualifying a recomposed USB product.

The test-only fixture `tests/fixtures/metadata-golden-c118e0c.bin` contains LBA 0..261
(exclusive), 133632 bytes. SHA256:
`41ec718b804d5e317f8c46670856d6abfe9ebfb8fd38f59964134654426c6eed`.
It was generated independently using unchanged `poc/usb-radio/core/src/lib.rs` and its
`config_store.rs` module from `iobewi/embewi-agent` commit
`c118e0ca6a5ebb8a18e3081907aa57bfa28ff003`, compiled as an rlib with
`rustc --edition=2021 --crate-name usb_radio_golden --crate-type rlib` (rustc 1.99.0).
A host executable instantiated `VirtualFat16::new(DiagnosticSource)` and wrote each
512-byte sector for `0..DATA_START_LBA`, asserting every read returned Ready.
The product identity appears only in this compatibility fixture and its test setup.

## Known limitations

One file beginning at cluster 2, no subdirectories, no partial final cluster, no MBR,
no bootable code, no timestamps and no writes. One reserved sector and two FAT copies
are fixed. Metadata keeps the golden OEM identity `IOBEWI  `, media descriptor 0xF8,
legacy geometry fields 63/255, and 32-bit BPB total-sector encoding even for small volumes.
Only host behaviour is validated here; third-party filesystem and USB compatibility
requires consumer-specific qualification. File source calls must remain non-blocking.

## Migration provenance

Ported from `iobewi/iobewi_old` at `345378e`, `fs/fat16`.
See [issue #66 parity and evidence](../../../docs/migrations/66-usb-stream.md).
Historical tests and behavior are retained; upstream Embassy and HAL types remain exposed.

## Resource bounds

The volume stores only its source, config and geometry; it never materializes the
advertised disk. The caller supplies each 512-byte output sector. No heap is used.

## Example

```rust
use iobewi_fat16::{Fat16Config, FileSource, ReadStatus, VirtualFat16, SECTOR_SIZE};
struct Source;
impl FileSource for Source {
    fn read_file_sector(&mut self, _: u32, out: &mut [u8; SECTOR_SIZE]) -> ReadStatus {
        out.fill(0x5a);
        ReadStatus::Ready
    }
}
let mut disk = VirtualFat16::new(Source, Fat16Config {
    sectors_per_cluster: 1,
    file_cluster_count: 4085,
    root_entries: 16,
    file_name: *b"LIVE    BIN",
    volume_label: *b"TEST VOLUME",
    volume_serial: 42,
}).unwrap();
let mut sector = [0; SECTOR_SIZE];
assert_eq!(disk.read_sector(0, &mut sector), ReadStatus::Ready);
assert_eq!(&sector[510..], &[0x55, 0xaa]);
assert_eq!(disk.read_sector(disk.geometry().data_start_lba, &mut sector), ReadStatus::Ready);
assert_eq!(sector, [0x5a; SECTOR_SIZE]);
```
