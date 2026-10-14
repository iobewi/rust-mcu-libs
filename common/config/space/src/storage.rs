//! ConfigSpace persistence on EKV, using its upstream Flash trait directly.
//! Power-fail recovery has the known limitation tracked in issue #70.
use crate::{Budget, ConfigBackend, Snapshot};
use alloc::{vec, vec::Vec};
use ekv::{Database, flash::Flash};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex};

pub const PAGE_SIZE: usize = 4096;
pub const MAX_PAYLOAD: usize = 8176;
const HEADER: usize = 16;
const RESERVED_PAGES: usize = 16;
const MAGIC: &[u8; 4] = b"CSP2";

#[derive(Debug, Clone, Copy)]
pub struct SpaceSpec {
    pub name: &'static str,
    pub max_bytes: usize,
}
impl SpaceSpec {
    pub const fn new(name: &'static str, max_bytes: usize) -> Self {
        Self { name, max_bytes }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionError {
    Profile,
    Geometry,
    InvalidClaim,
    DuplicateClaim,
    Capacity,
}
#[derive(Debug)]
pub enum Error<E> {
    Read(ekv::ReadError<E>),
    Write(ekv::WriteError<E>),
    Commit(ekv::CommitError<E>),
    Mount(ekv::MountError<E>),
    Format(ekv::FormatError<E>),
    Cursor(ekv::CursorError<E>),
    Database(ekv::Error<E>),
    InvalidSpace,
    TooLarge,
    Corrupt,
    GenerationOverflow,
    ReadBack,
}
/// One store per region, with the application's complete manifest admitted at
/// construction. The writer serializes generation read, commit and readback;
/// the Flash implementation retains sole ownership of physical synchronization.
pub struct Store<F: Flash> {
    db: Database<F, CriticalSectionRawMutex>,
    writer: Mutex<CriticalSectionRawMutex, ()>,
    specs: &'static [SpaceSpec],
    capacity: usize,
}
pub struct EkvBackend<'a, F: Flash> {
    store: &'a Store<F>,
}
impl<F: Flash> Copy for EkvBackend<'_, F> {}
impl<F: Flash> Clone for EkvBackend<'_, F> {
    fn clone(&self) -> Self {
        *self
    }
}
fn units(name: &str, budget: usize) -> Option<usize> {
    // Half-page effective payload, fourfold live/replacement allowance and
    // one extra page per space; retain a separate 16-page global reserve.
    HEADER
        .checked_add(name.len())?
        .checked_add(budget)?
        .div_ceil(PAGE_SIZE / 2)
        .checked_add(1)?
        .checked_mul(4)
}
fn profile_ok() -> bool {
    use ekv::config as c;
    c::CRC
        && c::ALIGN == 4
        && c::PAGE_SIZE == PAGE_SIZE
        && c::MAX_PAGE_COUNT == 256
        && c::MAX_KEY_SIZE == 64
        && c::MAX_VALUE_SIZE == 8192
        && c::MAX_CHUNK_SIZE == 256
        && c::ERASE_VALUE == 255
        && c::BRANCHING_FACTOR == 2
        && c::SCRATCH_PAGE_COUNT == 4
}
fn decode(raw: &[u8]) -> Result<(u64, bool, &[u8]), ()> {
    if raw.len() < HEADER
        || &raw[..4] != MAGIC
        || raw[12] > 1
        || raw[13..16] != [0; 3]
        || (raw[12] == 0 && raw.len() != HEADER)
    {
        return Err(());
    }
    let generation = u64::from_le_bytes(raw[4..12].try_into().map_err(|_| ())?);
    if generation == 0 {
        return Err(());
    }
    Ok((generation, raw[12] == 1, &raw[HEADER..]))
}
fn encode(generation: u64, present: bool, payload: &[u8]) -> Vec<u8> {
    let mut raw = Vec::with_capacity(HEADER + payload.len());
    raw.extend_from_slice(MAGIC);
    raw.extend_from_slice(&generation.to_le_bytes());
    raw.push(u8::from(present));
    raw.extend_from_slice(&[0; 3]);
    raw.extend_from_slice(payload);
    raw
}
impl<F: Flash> Store<F> {
    /// Check profile, geometry and the full manifest before any flash I/O.
    pub fn new(
        flash: F,
        random_seed: u32,
        specs: &'static [SpaceSpec],
    ) -> Result<Self, AdmissionError> {
        if !profile_ok() {
            return Err(AdmissionError::Profile);
        }
        let pages = flash.page_count();
        if !(32..=256).contains(&pages) {
            return Err(AdmissionError::Geometry);
        }
        let capacity = pages - RESERVED_PAGES;
        let mut used = 0usize;
        for (i, spec) in specs.iter().enumerate() {
            if spec.name.is_empty()
                || spec.name.len() > 64
                || spec.name.as_bytes().contains(&0)
                || spec.max_bytes == 0
                || spec.max_bytes > MAX_PAYLOAD
            {
                return Err(AdmissionError::InvalidClaim);
            }
            if specs[..i].iter().any(|s| s.name == spec.name) {
                return Err(AdmissionError::DuplicateClaim);
            }
            used = used
                .checked_add(units(spec.name, spec.max_bytes).ok_or(AdmissionError::Capacity)?)
                .ok_or(AdmissionError::Capacity)?;
        }
        if used > capacity {
            return Err(AdmissionError::Capacity);
        }
        let mut config = ekv::Config::default();
        config.random_seed = random_seed;
        Ok(Self {
            db: Database::new(flash, config),
            writer: Mutex::new(()),
            specs,
            capacity,
        })
    }
    pub fn backend(&self) -> EkvBackend<'_, F> {
        EkvBackend { store: self }
    }
    /// Destructive initialization, explicitly selected by the composition root.
    /// Never use as automatic recovery from a failed mount.
    pub async fn format(&self) -> Result<(), Error<F::Error>> {
        let _writer = self.writer.lock().await;
        self.db.format().await.map_err(Error::Format)
    }
    pub async fn mount(&self) -> Result<(), Error<F::Error>> {
        let _writer = self.writer.lock().await;
        self.db.mount().await.map_err(Error::Mount)?;
        self.validate().await
    }
    async fn validate(&self) -> Result<(), Error<F::Error>> {
        let tx = self.db.read_transaction().await;
        let mut cursor = tx.read_all().await.map_err(Error::Database)?;
        let mut key = [0u8; 64];
        let mut value = vec![0; 8192];
        while let Some((k, v)) = cursor
            .next(&mut key, &mut value)
            .await
            .map_err(Error::Cursor)?
        {
            let name = core::str::from_utf8(&key[..k]).map_err(|_| Error::Corrupt)?;
            let spec = self
                .specs
                .iter()
                .find(|s| s.name == name)
                .ok_or(Error::InvalidSpace)?;
            let (_, _, payload) = decode(&value[..v]).map_err(|_| Error::Corrupt)?;
            if payload.len() > spec.max_bytes {
                return Err(Error::TooLarge);
            }
        }
        Ok(())
    }
    async fn read_raw(&self, name: &str) -> Result<Option<Vec<u8>>, Error<F::Error>> {
        let spec = self
            .specs
            .iter()
            .find(|s| s.name == name)
            .ok_or(Error::InvalidSpace)?;
        let mut value = vec![0; HEADER + spec.max_bytes];
        let tx = self.db.read_transaction().await;
        match tx.read(name.as_bytes(), &mut value).await {
            Ok(n) => {
                value.truncate(n);
                decode(&value).map_err(|_| Error::Corrupt)?;
                Ok(Some(value))
            }
            Err(ekv::ReadError::KeyNotFound) => Ok(None),
            Err(e) => Err(Error::Read(e)),
        }
    }
    async fn replace(
        &self,
        name: &str,
        present: bool,
        payload: &[u8],
    ) -> Result<u64, Error<F::Error>> {
        let spec = self
            .specs
            .iter()
            .find(|s| s.name == name)
            .ok_or(Error::InvalidSpace)?;
        if payload.len() > spec.max_bytes {
            return Err(Error::TooLarge);
        }
        let _writer = self.writer.lock().await;
        let generation = match self.read_raw(name).await? {
            Some(raw) => decode(&raw)
                .map_err(|_| Error::Corrupt)?
                .0
                .checked_add(1)
                .ok_or(Error::GenerationOverflow)?,
            None => 1,
        };
        let raw = encode(generation, present, payload);
        let mut tx = self.db.write_transaction().await;
        tx.write(name.as_bytes(), &raw)
            .await
            .map_err(Error::Write)?;
        tx.commit().await.map_err(Error::Commit)?;
        if self.read_raw(name).await?.as_deref() != Some(raw.as_slice()) {
            return Err(Error::ReadBack);
        }
        Ok(generation)
    }
}
impl<F: Flash> EkvBackend<'_, F> {
    /// Read-only record/CRC/manifest check: no canary programming on boot.
    pub async fn self_check(&self) -> bool {
        self.store.validate().await.is_ok()
    }
}
impl<F: Flash> ConfigBackend for EkvBackend<'_, F> {
    type Error = Error<F::Error>;
    fn capacity_units(&self) -> usize {
        self.store.capacity
    }
    fn reservation_units(&self, space: &str, budget: Budget) -> Option<usize> {
        let spec = self.store.specs.iter().find(|s| s.name == space)?;
        if budget.max_bytes() > spec.max_bytes {
            return None;
        }
        units(space, spec.max_bytes)
    }
    async fn load(&self, space: &str) -> Result<Option<Snapshot>, Self::Error> {
        let Some(raw) = self.store.read_raw(space).await? else {
            return Ok(None);
        };
        let (generation, present, data) = decode(&raw).map_err(|_| Error::Corrupt)?;
        Ok(present.then(|| Snapshot {
            generation,
            data: data.to_vec(),
        }))
    }
    async fn commit(&self, space: &str, data: &[u8]) -> Result<u64, Self::Error> {
        self.store.replace(space, true, data).await
    }
    async fn clear(&self, space: &str) -> Result<u64, Self::Error> {
        self.store.replace(space, false, &[]).await
    }
}
