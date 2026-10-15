use core::cell::Cell;
use iobewi_fat16::{
    ConfigError, FAT_COUNT, Fat16Config, FileSource, RESERVED_SECTORS, ReadOnlyBlockDevice,
    ReadStatus, SECTOR_SIZE, VirtualFat16,
};

fn golden_config() -> Fat16Config {
    Fat16Config {
        sectors_per_cluster: 64,
        file_cluster_count: 32768,
        root_entries: 32,
        file_name: *b"RADIO   MP3",
        volume_label: *b"RADIOUSB   ",
        volume_serial: 0x5241_4449,
    }
}

struct Source<'a> {
    index: &'a Cell<u32>,
    begins: &'a Cell<u32>,
    ends: &'a Cell<u32>,
    status: ReadStatus,
}
impl FileSource for Source<'_> {
    fn begin_session(&mut self) {
        self.begins.set(self.begins.get() + 1);
    }
    fn end_session(&mut self) {
        self.ends.set(self.ends.get() + 1);
    }
    fn read_file_sector(&mut self, index: u32, out: &mut [u8; SECTOR_SIZE]) -> ReadStatus {
        self.index.set(index);
        out.fill(0xa5);
        out[..4].copy_from_slice(&index.to_le_bytes());
        self.status
    }
}
struct NeverRead;
impl FileSource for NeverRead {
    fn read_file_sector(&mut self, _: u32, _: &mut [u8; SECTOR_SIZE]) -> ReadStatus {
        panic!("metadata must not access file source")
    }
}

#[test]
fn all_metadata_matches_independent_golden_fixture() {
    let fixture = include_bytes!("fixtures/metadata-golden-c118e0c.bin");
    let mut disk = VirtualFat16::new(NeverRead, golden_config()).unwrap();
    assert_eq!(disk.geometry().data_start_lba, 261);
    assert_eq!(fixture.len(), 261 * SECTOR_SIZE);
    for (lba, expected) in fixture.as_chunks::<SECTOR_SIZE>().0.iter().enumerate() {
        let mut actual = [0xa5; SECTOR_SIZE];
        assert_eq!(disk.read_sector(lba as u32, &mut actual), ReadStatus::Ready);
        assert_eq!(&actual[..], expected, "metadata LBA {lba}");
    }
}

#[test]
fn alternate_geometry_and_identity_are_used() {
    let config = Fat16Config {
        sectors_per_cluster: 1,
        file_cluster_count: 4096,
        root_entries: 16,
        file_name: *b"LOG     BIN",
        volume_label: *b"DATA       ",
        volume_serial: 0x1234_5678,
    };
    let mut disk = VirtualFat16::new(NeverRead, config).unwrap();
    let geom = disk.geometry();
    assert_eq!(
        (geom.fat_sectors, geom.root_sectors, geom.data_start_lba),
        (17, 1, 36)
    );
    assert_eq!(geom.file_size, 4096 * 512);
    assert_eq!(disk.last_lba(), 36 + 4096 - 1);
    let mut sector = [0; SECTOR_SIZE];
    disk.read_sector(0, &mut sector);
    assert_eq!(sector[13], 1);
    assert_eq!(&sector[17..19], &16u16.to_le_bytes());
    assert_eq!(&sector[19..21], &[0, 0]);
    assert_eq!(&sector[32..36], &geom.total_sectors.to_le_bytes());
    assert_eq!(&sector[39..43], &config.volume_serial.to_le_bytes());
    assert_eq!(&sector[43..54], &config.volume_label);
    disk.read_sector(RESERVED_SECTORS + FAT_COUNT * geom.fat_sectors, &mut sector);
    assert_eq!(&sector[..11], &config.volume_label);
    assert_eq!(&sector[32..43], &config.file_name);
    assert_eq!(&sector[60..64], &geom.file_size.to_le_bytes());
}

#[test]
fn fat_copies_and_complete_contiguous_chain() {
    let mut disk = VirtualFat16::new(NeverRead, golden_config()).unwrap();
    let geom = disk.geometry();
    let mut first = [0; SECTOR_SIZE];
    let mut second = [0; SECTOR_SIZE];
    for fat_sector in 0..geom.fat_sectors {
        disk.read_sector(RESERVED_SECTORS + fat_sector, &mut first);
        disk.read_sector(
            RESERVED_SECTORS + geom.fat_sectors + fat_sector,
            &mut second,
        );
        assert_eq!(first, second);
        for (slot, encoded) in first.as_chunks::<2>().0.iter().enumerate() {
            let cluster = fat_sector * 256 + slot as u32;
            let expected = match cluster {
                0 => 0xfff8,
                1 => 0xffff,
                c if c < u32::from(geom.file_last_cluster) => (c + 1) as u16,
                c if c == u32::from(geom.file_last_cluster) => 0xffff,
                _ => 0,
            };
            assert_eq!(u16::from_le_bytes(*encoded), expected);
        }
    }
}

#[test]
fn file_mapping_bounds_and_session_callbacks_through_block_contract() {
    let index = Cell::new(u32::MAX);
    let begins = Cell::new(0);
    let ends = Cell::new(0);
    let source = Source {
        index: &index,
        begins: &begins,
        ends: &ends,
        status: ReadStatus::Ready,
    };
    let mut disk = VirtualFat16::new(source, golden_config()).unwrap();
    let geom = disk.geometry();
    let block: &mut dyn ReadOnlyBlockDevice = &mut disk;
    assert_eq!(block.last_lba(), geom.total_sectors - 1);
    block.begin_session();
    block.end_session();
    block.begin_session();
    assert_eq!((begins.get(), ends.get()), (2, 1));
    let mut sector = [0xa5; SECTOR_SIZE];
    assert_eq!(
        block.read_sector(geom.data_start_lba - 1, &mut sector),
        ReadStatus::Ready
    );
    assert!(sector.iter().all(|&byte| byte == 0));
    assert_eq!(index.get(), u32::MAX);
    for (lba, expected_index) in [
        (geom.data_start_lba, 0),
        (geom.data_start_lba + 7, 7),
        (geom.total_sectors - 1, 32768 * 64 - 1),
    ] {
        assert_eq!(block.read_sector(lba, &mut sector), ReadStatus::Ready);
        assert_eq!(index.get(), expected_index);
        assert_eq!(&sector[..4], &expected_index.to_le_bytes());
    }
    let previous_index = index.get();
    for lba in [geom.total_sectors, u32::MAX] {
        sector.fill(0xa5);
        assert_eq!(block.read_sector(lba, &mut sector), ReadStatus::Expired);
        assert!(sector.iter().all(|&byte| byte == 0));
        assert_eq!(index.get(), previous_index);
    }
}

#[test]
fn unavailable_source_status_is_propagated_with_zero_output() {
    for status in [ReadStatus::Pending, ReadStatus::Expired] {
        let index = Cell::new(0);
        let begins = Cell::new(0);
        let ends = Cell::new(0);
        let source = Source {
            index: &index,
            begins: &begins,
            ends: &ends,
            status,
        };
        let mut disk = VirtualFat16::new(source, golden_config()).unwrap();
        let mut sector = [0xa5; SECTOR_SIZE];
        assert_eq!(
            disk.read_sector(disk.geometry().data_start_lba, &mut sector),
            status
        );
        assert!(sector.iter().all(|&byte| byte == 0));
    }
}

#[test]
fn rejects_invalid_geometry_without_panicking_on_extreme_values() {
    for value in [0, 3, 128] {
        let config = Fat16Config {
            sectors_per_cluster: value,
            ..golden_config()
        };
        assert_eq!(
            config.geometry(),
            Err(ConfigError::InvalidSectorsPerCluster)
        );
    }
    for value in [0, 4084, 65519, 65525, u32::MAX] {
        let config = Fat16Config {
            file_cluster_count: value,
            ..golden_config()
        };
        assert_eq!(config.geometry(), Err(ConfigError::InvalidClusterCount));
        assert!(VirtualFat16::new(NeverRead, config).is_err());
    }
    for value in [0, 1, 2, 17, u16::MAX] {
        let config = Fat16Config {
            root_entries: value,
            ..golden_config()
        };
        assert_eq!(config.geometry(), Err(ConfigError::InvalidRootEntries));
    }
    for count in [4085, 65518] {
        let config = Fat16Config {
            file_cluster_count: count,
            root_entries: 65520,
            ..golden_config()
        };
        let geom = config.geometry().unwrap();
        assert_eq!(geom.file_size, count * 64 * 512);
        assert!(geom.file_last_cluster < 0xfff0);
    }
}
