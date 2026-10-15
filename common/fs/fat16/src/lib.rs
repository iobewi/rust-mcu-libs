#![no_std]
#![doc = include_str!("../README.md")]

pub use iobewi_block::{ReadOnlyBlockDevice, ReadStatus, SECTOR_SIZE};

pub const RESERVED_SECTORS: u32 = 1;
pub const FAT_COUNT: u32 = 2;
const FILE_START_CLUSTER: u16 = 2;
const MEDIA_DESCRIPTOR: u8 = 0xF8;

/// Product-selected geometry and on-disk identity of one contiguous file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fat16Config {
    pub sectors_per_cluster: u8,
    pub file_cluster_count: u32,
    pub root_entries: u16,
    /// Encoded FAT short name: eight name bytes followed by three extension bytes.
    pub file_name: [u8; 11],
    pub volume_label: [u8; 11],
    pub volume_serial: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigError {
    InvalidSectorsPerCluster,
    InvalidClusterCount,
    InvalidRootEntries,
    ArithmeticOverflow,
}

/// Derived, immutable disk geometry. The file fills all data clusters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Geometry {
    pub fat_sectors: u32,
    pub root_sectors: u32,
    pub data_start_lba: u32,
    pub total_sectors: u32,
    pub file_size: u32,
    pub file_last_cluster: u16,
}

impl Fat16Config {
    pub fn geometry(&self) -> Result<Geometry, ConfigError> {
        if !self.sectors_per_cluster.is_power_of_two() || self.sectors_per_cluster > 64 {
            return Err(ConfigError::InvalidSectorsPerCluster);
        }
        // Keep FAT16 classification and stay below FAT16's reserved cluster IDs.
        if !(4085..65525).contains(&self.file_cluster_count)
            || self.file_cluster_count + 1 >= 0xFFF0
        {
            return Err(ConfigError::InvalidClusterCount);
        }
        // The label and file require two entries; a root sector holds 16 entries.
        if self.root_entries < 16 || !self.root_entries.is_multiple_of(16) {
            return Err(ConfigError::InvalidRootEntries);
        }
        let fat_sectors = ((self.file_cluster_count + 2) * 2).div_ceil(SECTOR_SIZE as u32);
        let root_sectors = u32::from(self.root_entries) / 16;
        let data_start_lba = RESERVED_SECTORS + FAT_COUNT * fat_sectors + root_sectors;
        let file_sectors = self
            .file_cluster_count
            .checked_mul(u32::from(self.sectors_per_cluster))
            .ok_or(ConfigError::ArithmeticOverflow)?;
        let file_size = file_sectors
            .checked_mul(SECTOR_SIZE as u32)
            .ok_or(ConfigError::ArithmeticOverflow)?;
        let total_sectors = data_start_lba
            .checked_add(file_sectors)
            .ok_or(ConfigError::ArithmeticOverflow)?;
        Ok(Geometry {
            fat_sectors,
            root_sectors,
            data_start_lba,
            total_sectors,
            file_size,
            file_last_cluster: (self.file_cluster_count + 1) as u16,
        })
    }
}

/// Supplies sectors of the single virtual file without waiting for unavailable data.
///
/// Session callbacks may rebase a stream-backed source. The volume ensures the
/// output is zeroed on unavailable reads, regardless of what the source writes.
pub trait FileSource {
    fn begin_session(&mut self) {}
    fn end_session(&mut self) {}
    fn read_file_sector(&mut self, index: u32, out: &mut [u8; SECTOR_SIZE]) -> ReadStatus;
}

pub struct VirtualFat16<S> {
    source: S,
    config: Fat16Config,
    geometry: Geometry,
}

impl<S: FileSource> VirtualFat16<S> {
    pub fn new(source: S, config: Fat16Config) -> Result<Self, ConfigError> {
        let geometry = config.geometry()?;
        Ok(Self {
            source,
            config,
            geometry,
        })
    }

    pub const fn geometry(&self) -> Geometry {
        self.geometry
    }

    pub const fn last_lba(&self) -> u32 {
        self.geometry.total_sectors - 1
    }

    pub fn begin_session(&mut self) {
        self.source.begin_session();
    }

    pub fn end_session(&mut self) {
        self.source.end_session();
    }

    pub fn read_sector(&mut self, lba: u32, out: &mut [u8; SECTOR_SIZE]) -> ReadStatus {
        out.fill(0);
        if lba >= self.geometry.total_sectors {
            return ReadStatus::Expired;
        }
        if lba == 0 {
            self.write_boot_sector(out);
            return ReadStatus::Ready;
        }
        let fat2_start = RESERVED_SECTORS + self.geometry.fat_sectors;
        let root_start = fat2_start + self.geometry.fat_sectors;
        if lba < root_start {
            let fat_sector = (lba - RESERVED_SECTORS) % self.geometry.fat_sectors;
            self.write_fat_sector(fat_sector, out);
        } else if lba < self.geometry.data_start_lba {
            self.write_root_sector(lba - root_start, out);
        } else {
            let status = self
                .source
                .read_file_sector(lba - self.geometry.data_start_lba, out);
            if status != ReadStatus::Ready {
                out.fill(0);
            }
            return status;
        }
        ReadStatus::Ready
    }

    fn write_boot_sector(&self, out: &mut [u8; SECTOR_SIZE]) {
        out[0..3].copy_from_slice(&[0xEB, 0x3C, 0x90]);
        out[3..11].copy_from_slice(b"IOBEWI  ");
        put_u16(out, 11, SECTOR_SIZE as u16);
        out[13] = self.config.sectors_per_cluster;
        put_u16(out, 14, RESERVED_SECTORS as u16);
        out[16] = FAT_COUNT as u8;
        put_u16(out, 17, self.config.root_entries);
        put_u16(out, 19, 0); // Preserve the golden 32-bit total-sector encoding.
        out[21] = MEDIA_DESCRIPTOR;
        put_u16(out, 22, self.geometry.fat_sectors as u16);
        put_u16(out, 24, 63);
        put_u16(out, 26, 255);
        put_u32(out, 28, 0);
        put_u32(out, 32, self.geometry.total_sectors);
        out[36] = 0x80;
        out[38] = 0x29;
        put_u32(out, 39, self.config.volume_serial);
        out[43..54].copy_from_slice(&self.config.volume_label);
        out[54..62].copy_from_slice(b"FAT16   ");
        out[510] = 0x55;
        out[511] = 0xAA;
    }

    fn write_fat_sector(&self, fat_sector: u32, out: &mut [u8; SECTOR_SIZE]) {
        let first_entry = fat_sector * (SECTOR_SIZE as u32 / 2);
        for slot in 0..(SECTOR_SIZE / 2) {
            let entry = first_entry + slot as u32;
            let value = match entry {
                0 => 0xFFF8,
                1 => 0xFFFF,
                c if c >= u32::from(FILE_START_CLUSTER)
                    && c < u32::from(self.geometry.file_last_cluster) =>
                {
                    (c + 1) as u16
                }
                c if c == u32::from(self.geometry.file_last_cluster) => 0xFFFF,
                _ => 0,
            };
            put_u16(out, slot * 2, value);
        }
    }

    fn write_root_sector(&self, index: u32, out: &mut [u8; SECTOR_SIZE]) {
        if index != 0 {
            return;
        }
        out[0..11].copy_from_slice(&self.config.volume_label);
        out[11] = 0x08;
        let e = 32;
        out[e..e + 11].copy_from_slice(&self.config.file_name);
        out[e + 11] = 0x21; // Read-only + archive.
        put_u16(out, e + 26, FILE_START_CLUSTER);
        put_u32(out, e + 28, self.geometry.file_size);
    }
}

impl<S: FileSource> ReadOnlyBlockDevice for VirtualFat16<S> {
    fn last_lba(&self) -> u32 {
        self.last_lba()
    }
    fn begin_session(&mut self) {
        self.begin_session();
    }
    fn end_session(&mut self) {
        self.end_session();
    }
    fn read_sector(&mut self, lba: u32, out: &mut [u8; SECTOR_SIZE]) -> ReadStatus {
        self.read_sector(lba, out)
    }
}

fn put_u16(out: &mut [u8], offset: usize, value: u16) {
    out[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn put_u32(out: &mut [u8], offset: usize, value: u32) {
    out[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
