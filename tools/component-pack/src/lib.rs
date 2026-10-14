//! ELF (intermediate build artefact) -> IWNI image. The ELF is *not* a public contract: only
//! the allocatable sections inside the target's code/data regions are read, nothing else.

use iobewi_firmware_update::compat::otm2::RuntimeApi;
use iobewi_workload_image::{HEADER_LEN, ImageError, ImageHeader, TargetLayout};

#[derive(Debug, PartialEq, Eq)]
pub enum PackError {
    NotElf32Le,
    Truncated,
    NoSection(&'static str),
    OutsideRegions { addr: u32, size: u32 },
    BssBeforeData,
    Image(ImageError),
}

impl From<ImageError> for PackError {
    fn from(e: ImageError) -> Self {
        PackError::Image(e)
    }
}

fn u16_at(b: &[u8], o: usize) -> Result<u16, PackError> {
    b.get(o..o + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
        .ok_or(PackError::Truncated)
}
fn u32_at(b: &[u8], o: usize) -> Result<u32, PackError> {
    b.get(o..o + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or(PackError::Truncated)
}

const SHF_ALLOC: u32 = 2;
const SHT_NOBITS: u32 = 8;

struct Section {
    addr: u32,
    offset: u32,
    size: u32,
    nobits: bool,
}

/// Options for building (test aids for hostile images are `overrides`).
pub struct PackOptions {
    pub layout: TargetLayout,
    pub requires: RuntimeApi,
    pub overrides: Overrides,
}

/// Header fields forced after the image is built, to produce deliberately bad images for
/// the negative tests. Never used by a normal build.
#[derive(Default, Clone, Copy)]
pub struct Overrides {
    pub target: Option<u16>,
    pub abi_version: Option<u16>,
    pub entry: Option<u32>,
    pub format_version: Option<u16>,
    pub magic: Option<[u8; 4]>,
    pub code_addr: Option<u32>,
}

pub struct Packed {
    pub bytes: Vec<u8>,
    pub header: ImageHeader,
}

pub fn pack(elf: &[u8], opts: &PackOptions) -> Result<Packed, PackError> {
    if elf.len() < 52 || elf[0..4] != *b"\x7fELF" || elf[4] != 1 || elf[5] != 1 {
        return Err(PackError::NotElf32Le);
    }
    let entry = u32_at(elf, 24)?;
    let shoff = u32_at(elf, 32)? as usize;
    let shentsize = u16_at(elf, 46)? as usize;
    let shnum = u16_at(elf, 48)? as usize;
    let mut sections = Vec::new();
    for i in 0..shnum {
        let o = shoff + i * shentsize;
        let sh_type = u32_at(elf, o + 4)?;
        let flags = u32_at(elf, o + 8)?;
        if flags & SHF_ALLOC == 0 {
            continue;
        }
        let size = u32_at(elf, o + 20)?;
        if size == 0 {
            continue;
        }
        sections.push(Section {
            addr: u32_at(elf, o + 12)?,
            offset: u32_at(elf, o + 16)?,
            size,
            nobits: sh_type == SHT_NOBITS,
        });
    }
    let l = &opts.layout;
    let in_range = |addr: u32, size: u32, base: u32, cap: u32| {
        addr >= base
            && addr
                .checked_add(size)
                .is_some_and(|e| e <= base.saturating_add(cap))
    };
    let mut code = Vec::new();
    let mut data = Vec::new();
    let mut bss_end = 0u32;
    let mut saw_code = false;
    for s in &sections {
        if in_range(s.addr, s.size, l.code_addr, l.code_capacity) {
            if s.nobits {
                return Err(PackError::OutsideRegions {
                    addr: s.addr,
                    size: s.size,
                });
            }
            saw_code = true;
            let off = (s.addr - l.code_addr) as usize;
            let src = elf
                .get(s.offset as usize..(s.offset + s.size) as usize)
                .ok_or(PackError::Truncated)?;
            if code.len() < off + src.len() {
                code.resize(off + src.len(), 0);
            }
            code[off..off + src.len()].copy_from_slice(src);
        } else if in_range(s.addr, s.size, l.data_addr, l.data_capacity) {
            if s.nobits {
                bss_end = bss_end.max(s.addr + s.size - l.data_addr);
            } else {
                let off = (s.addr - l.data_addr) as usize;
                let src = elf
                    .get(s.offset as usize..(s.offset + s.size) as usize)
                    .ok_or(PackError::Truncated)?;
                if data.len() < off + src.len() {
                    data.resize(off + src.len(), 0);
                }
                data[off..off + src.len()].copy_from_slice(src);
            }
        } else {
            return Err(PackError::OutsideRegions {
                addr: s.addr,
                size: s.size,
            });
        }
    }
    if !saw_code {
        return Err(PackError::NoSection(".text"));
    }
    // Pad every region to a 4-byte multiple so offsets stay word-aligned.
    while code.len() % 4 != 0 {
        code.push(0);
    }
    while data.len() % 4 != 0 {
        data.push(0);
    }
    if (bss_end as usize) < data.len() && bss_end != 0 {
        return Err(PackError::BssBeforeData);
    }
    let bss_size = if bss_end == 0 {
        0
    } else {
        bss_end - data.len() as u32
    };
    let code_offset = HEADER_LEN as u32;
    let data_offset = if data.is_empty() {
        0
    } else {
        code_offset + code.len() as u32
    };
    let image_size = HEADER_LEN as u32 + code.len() as u32 + data.len() as u32;
    let header = ImageHeader {
        target: l.target.code(),
        abi_version: l.abi_version,
        requires: opts.requires,
        flags: 0,
        image_size,
        entry,
        code_addr: l.code_addr,
        code_offset,
        code_size: code.len() as u32,
        data_addr: l.data_addr,
        data_offset,
        data_size: data.len() as u32,
        bss_size,
    };
    header.validate(l, u64::from(image_size))?;
    let mut hdr = header;
    let o = &opts.overrides;
    if let Some(v) = o.target {
        hdr.target = v;
    }
    if let Some(v) = o.abi_version {
        hdr.abi_version = v;
    }
    if let Some(v) = o.entry {
        hdr.entry = v;
    }
    if let Some(v) = o.code_addr {
        hdr.code_addr = v;
    }
    let mut bytes = Vec::with_capacity(image_size as usize);
    let mut h = hdr.encode();
    if let Some(v) = o.format_version {
        h[4..6].copy_from_slice(&v.to_le_bytes());
    }
    if let Some(m) = o.magic {
        h[0..4].copy_from_slice(&m);
    }
    bytes.extend_from_slice(&h);
    bytes.extend_from_slice(&code);
    bytes.extend_from_slice(&data);
    Ok(Packed { bytes, header })
}

#[cfg(test)]
mod tests;
