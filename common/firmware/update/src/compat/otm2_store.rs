//! Portable double-copy metadata store for OTM2. Two fixed-size copies are
//! kept; a commit overwrites the **older** one (or the invalid one), so the
//! last good record always survives an interrupted write. A reader picks the
//! newest valid copy (serial-number arithmetic on the sequence).

use super::otm2::{DecodeError, RECORD_LEN, Record, newer};

/// Where the two copies physically live (flash sectors, NVS blobs, memory...).
/// The store never assumes more than: reading a copy returns its bytes, and a
/// write that is interrupted leaves *that copy* torn but the other intact.
pub trait MetadataBackend {
    type Error;

    /// Number of copies: always [`COPIES`].
    fn read(&mut self, copy: usize) -> Result<[u8; RECORD_LEN], Self::Error>;
    fn write(&mut self, copy: usize, bytes: &[u8; RECORD_LEN]) -> Result<(), Self::Error>;
}

pub const COPIES: usize = 2;

/// What `load` found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Loaded {
    /// Both copies erased: nothing was ever stored.
    Blank,
    /// The newest valid record, and which copy holds it. `degraded` is true
    /// when the other copy was present but invalid (torn/corrupt).
    Record { record: Record, copy: usize, degraded: bool },
    /// No valid copy although at least one is not blank: never replaced by an
    /// invented state.
    Corrupted,
}

pub struct MetadataStore<B> {
    backend: B,
}

impl<B: MetadataBackend> MetadataStore<B> {
    pub fn new(backend: B) -> Self {
        Self { backend }
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    pub fn load(&mut self) -> Result<Loaded, B::Error> {
        let mut decoded = [Err(DecodeError::Blank); COPIES];
        for (copy, slot) in decoded.iter_mut().enumerate() {
            *slot = Record::decode(&self.backend.read(copy)?);
        }
        let best = match (decoded[0], decoded[1]) {
            (Ok(a), Ok(b)) => Some(if newer(b.sequence, a.sequence) { (b, 1) } else { (a, 0) }),
            (Ok(a), Err(_)) => Some((a, 0)),
            (Err(_), Ok(b)) => Some((b, 1)),
            (Err(_), Err(_)) => None,
        };
        Ok(match best {
            Some((record, copy)) => Loaded::Record { record, copy, degraded: decoded[1 - copy].is_err() },
            None if decoded.iter().all(|d| matches!(d, Err(DecodeError::Blank))) => Loaded::Blank,
            None => Loaded::Corrupted,
        })
    }

    /// Writes `record` to the copy that does not hold the newest valid one.
    pub fn commit(&mut self, record: &Record) -> Result<(), B::Error> {
        let target = match self.load()? {
            Loaded::Record { copy, .. } => 1 - copy,
            _ => 0,
        };
        self.backend.write(target, &record.encode())
    }
}
