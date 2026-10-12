//! OTM2 codec: the binary record only. No policy, no storage, no allocation.
//!
//! Layout (little-endian, packed, no implicit padding, `RECORD_LEN` = 180 bytes):
//!
//! | offset | size | field | meaning |
//! |---|---|---|---|
//! | 0 | 4 | magic | `b"OTM2"` |
//! | 4 | 1 | format_version | `1` |
//! | 5 | 1 | state | [`State`] code |
//! | 6 | 1 | active | slot selected to run: `0`=A, `1`=B, `0xFF`=none |
//! | 7 | 1 | candidate | staged slot not yet selected, `0xFF`=none |
//! | 8 | 1 | previous_valid | last confirmed slot to return to, `0xFF`=none |
//! | 9 | 3 | reserved | zero |
//! | 12 | 4 | sequence | u32 LE, serial-number arithmetic (wrap-safe) |
//! | 16 | 80 | meta[A] | slot A artifact metadata |
//! | 96 | 80 | meta[B] | slot B artifact metadata |
//! | 176 | 4 | crc32 | zlib CRC-32 of bytes `0..176`, u32 LE |
//!
//! Slot metadata (80 bytes): `digest[32]` (SHA-256), `size` u32 LE, `req_major`
//! u16 LE, `req_minor` u16 LE (required runtime API), `version[16]` and `id[24]`
//! (UTF-8, NUL padded, no embedded NUL). An unused slot is all zero.
//!
//! The CRC protects the *record*; the SHA-256 in the metadata protects the
//! *artifact*.

/// The two physical slots *inside* the OTM2 legacy domain; not firmware domains A/B.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    A,
    B,
}

/// API compatibility requirement carried by a legacy OTM2 artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeApi {
    pub major: u16,
    pub minor: u16,
}
impl RuntimeApi {
    pub const fn new(major: u16, minor: u16) -> Self {
        Self { major, minor }
    }
}

pub const MAGIC: [u8; 4] = *b"OTM2";
pub const FORMAT_VERSION: u8 = 1;
pub const RECORD_LEN: usize = 180;
pub const META_LEN: usize = 80;
pub const ID_LEN: usize = 24;
pub const VERSION_LEN: usize = 16;
const NONE: u8 = 0xFF;
const OFF_CRC: usize = RECORD_LEN - 4;

/// Persistent state of the Workload OTA (6 states; the engine's own
/// `Staged`/`Activating`/`PendingConfirmation` vocabulary is reused).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// No Workload, no candidate.
    Empty = 0,
    /// A confirmed Workload is selected; no candidate.
    Valid = 1,
    /// A candidate is written and verified, not selected yet.
    Staged = 2,
    /// Activation intent recorded; the supervisor switch may or may not have happened.
    Activating = 3,
    /// The candidate is selected and running, not yet confirmed.
    PendingConfirmation = 4,
    /// Rollback intent recorded; restoring `previous_valid`.
    RollingBack = 5,
}

impl State {
    fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            0 => Self::Empty,
            1 => Self::Valid,
            2 => Self::Staged,
            3 => Self::Activating,
            4 => Self::PendingConfirmation,
            5 => Self::RollingBack,
            _ => return None,
        })
    }
}

/// Why a field cannot be stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldError {
    IdTooLong,
    VersionTooLong,
    EmbeddedNul,
    SizeTooLarge,
}

/// Artifact metadata of one slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotMeta {
    pub digest: [u8; 32],
    pub size: u32,
    pub requires: RuntimeApi,
    version: [u8; VERSION_LEN],
    id: [u8; ID_LEN],
}

fn pack<const N: usize>(text: &str, too_long: FieldError) -> Result<[u8; N], FieldError> {
    let bytes = text.as_bytes();
    if bytes.len() > N {
        return Err(too_long);
    }
    if bytes.contains(&0) {
        return Err(FieldError::EmbeddedNul);
    }
    let mut out = [0u8; N];
    out[..bytes.len()].copy_from_slice(bytes);
    Ok(out)
}

fn unpack(field: &[u8]) -> &str {
    let end = field.iter().position(|b| *b == 0).unwrap_or(field.len());
    core::str::from_utf8(&field[..end]).unwrap_or("")
}

impl SlotMeta {
    pub const EMPTY: SlotMeta = SlotMeta {
        digest: [0; 32],
        size: 0,
        requires: RuntimeApi::new(0, 0),
        version: [0; VERSION_LEN],
        id: [0; ID_LEN],
    };

    pub fn new(id: &str, version: &str, digest: [u8; 32], size: u64, requires: RuntimeApi) -> Result<Self, FieldError> {
        Ok(Self {
            digest,
            size: u32::try_from(size).map_err(|_| FieldError::SizeTooLarge)?,
            requires,
            version: pack(version, FieldError::VersionTooLong)?,
            id: pack(id, FieldError::IdTooLong)?,
        })
    }

    pub fn id(&self) -> &str {
        unpack(&self.id)
    }

    pub fn version(&self) -> &str {
        unpack(&self.version)
    }

    fn encode(&self, out: &mut [u8]) {
        out[0..32].copy_from_slice(&self.digest);
        out[32..36].copy_from_slice(&self.size.to_le_bytes());
        out[36..38].copy_from_slice(&self.requires.major.to_le_bytes());
        out[38..40].copy_from_slice(&self.requires.minor.to_le_bytes());
        out[40..56].copy_from_slice(&self.version);
        out[56..80].copy_from_slice(&self.id);
    }

    fn decode(raw: &[u8]) -> Self {
        let mut digest = [0u8; 32];
        digest.copy_from_slice(&raw[0..32]);
        let mut version = [0u8; VERSION_LEN];
        version.copy_from_slice(&raw[40..56]);
        let mut id = [0u8; ID_LEN];
        id.copy_from_slice(&raw[56..80]);
        Self {
            digest,
            size: u32::from_le_bytes([raw[32], raw[33], raw[34], raw[35]]),
            requires: RuntimeApi::new(u16::from_le_bytes([raw[36], raw[37]]), u16::from_le_bytes([raw[38], raw[39]])),
            version,
            id,
        }
    }
}

/// One decoded OTM2 record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Record {
    pub state: State,
    pub active: Option<Side>,
    pub candidate: Option<Side>,
    pub previous_valid: Option<Side>,
    pub sequence: u32,
    /// Metadata indexed by slot: `[A, B]`.
    pub meta: [SlotMeta; 2],
}

/// Why a buffer is not an acceptable OTM2 record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// Erased (all `0xFF`): never written, not corrupt.
    Blank,
    Truncated,
    BadMagic,
    BadVersion(u8),
    BadCrc,
    /// Valid CRC but an impossible content (unknown code, non-zero reserved,
    /// fields incoherent with the state).
    Malformed,
}

pub const fn slot_index(side: Side) -> usize {
    match side {
        Side::A => 0,
        Side::B => 1,
    }
}

fn slot_code(slot: Option<Side>) -> u8 {
    slot.map_or(NONE, |s| slot_index(s) as u8)
}

fn slot_from(code: u8) -> Result<Option<Side>, DecodeError> {
    match code {
        0 => Ok(Some(Side::A)),
        1 => Ok(Some(Side::B)),
        NONE => Ok(None),
        _ => Err(DecodeError::Malformed),
    }
}

/// zlib CRC-32 (poly 0xEDB88320, init/final xor 0xFFFFFFFF).
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// Serial-number comparison: is `a` newer than `b`, wrap-safe.
pub const fn newer(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) > 0
}

impl Record {
    pub const fn empty(sequence: u32) -> Self {
        Self {
            state: State::Empty,
            active: None,
            candidate: None,
            previous_valid: None,
            sequence,
            meta: [SlotMeta::EMPTY; 2],
        }
    }

    pub fn encode(&self) -> [u8; RECORD_LEN] {
        let mut out = [0u8; RECORD_LEN];
        out[0..4].copy_from_slice(&MAGIC);
        out[4] = FORMAT_VERSION;
        out[5] = self.state as u8;
        out[6] = slot_code(self.active);
        out[7] = slot_code(self.candidate);
        out[8] = slot_code(self.previous_valid);
        out[12..16].copy_from_slice(&self.sequence.to_le_bytes());
        self.meta[0].encode(&mut out[16..96]);
        self.meta[1].encode(&mut out[96..176]);
        let crc = crc32(&out[..OFF_CRC]);
        out[OFF_CRC..].copy_from_slice(&crc.to_le_bytes());
        out
    }

    pub fn decode(raw: &[u8]) -> Result<Self, DecodeError> {
        if raw.iter().all(|b| *b == 0xFF) && !raw.is_empty() {
            return Err(DecodeError::Blank);
        }
        if raw.len() < RECORD_LEN {
            return Err(DecodeError::Truncated);
        }
        let raw = &raw[..RECORD_LEN];
        if raw[0..4] != MAGIC {
            return Err(DecodeError::BadMagic);
        }
        if raw[4] != FORMAT_VERSION {
            return Err(DecodeError::BadVersion(raw[4]));
        }
        let stored = u32::from_le_bytes([raw[OFF_CRC], raw[OFF_CRC + 1], raw[OFF_CRC + 2], raw[OFF_CRC + 3]]);
        if crc32(&raw[..OFF_CRC]) != stored {
            return Err(DecodeError::BadCrc);
        }
        if raw[9..12] != [0, 0, 0] {
            return Err(DecodeError::Malformed);
        }
        let record = Self {
            state: State::from_code(raw[5]).ok_or(DecodeError::Malformed)?,
            active: slot_from(raw[6])?,
            candidate: slot_from(raw[7])?,
            previous_valid: slot_from(raw[8])?,
            sequence: u32::from_le_bytes([raw[12], raw[13], raw[14], raw[15]]),
            meta: [SlotMeta::decode(&raw[16..96]), SlotMeta::decode(&raw[96..176])],
        };
        record.check_coherence()?;
        Ok(record)
    }

    /// The slot fields each state allows (see the module doc / `docs/otm2.md`).
    fn check_coherence(&self) -> Result<(), DecodeError> {
        let (a, c, p) = (self.active, self.candidate, self.previous_valid);
        let ok = match self.state {
            State::Empty => a.is_none() && c.is_none() && p.is_none(),
            State::Valid => a.is_some() && c.is_none() && p.is_none(),
            State::Staged | State::Activating => c.is_some() && p.is_none() && a != c,
            State::PendingConfirmation | State::RollingBack => a.is_some() && c.is_none() && a != p,
        };
        if ok { Ok(()) } else { Err(DecodeError::Malformed) }
    }
}
