//! The two effect boundaries the engine writes through -- deliberately not
//! one trait. They carry different guarantees, and collapsing them into a
//! single "storage" abstraction would let a caller (or an implementer)
//! quietly assume the stronger one where only the weaker one actually holds.
//!
//! * [`ArtifactStorage`] is bulk, streaming, *not* read-back-verified by
//!   this crate: the engine trusts the durability watermark the backend
//!   reports, and computes the artifact's digest only over bytes that
//!   watermark has already covered -- never over bytes merely handed to
//!   `write`. What "durable" means (a flash sector landed, a block device
//!   fsynced, ...) and how it gets there (erase-then-program, RMW, ...) is
//!   entirely the backend's business; this trait has no notion of sectors,
//!   alignment or padding.
//! * [`TransactionMetadata`] is small, and its one write operation --
//!   [`TransactionMetadata::commit`] -- is the one primitive this whole
//!   crate trusts to be indivisible: a reader (`load`) must never observe
//!   anything between the previous record and the new one, including across
//!   a restart that interrupts `commit` at any point. How a backend gets
//!   there (a single atomic write, or several writes ordered behind one
//!   marker field/word programmed last and read back) is its own concern --
//!   this crate does not attempt to compose that guarantee out of weaker
//!   primitives on the backend's behalf.

/// Bulk artifact bytes: written once, streamed, never re-read by this crate
/// to check what was written (the digest is accumulated while writing --
/// see [`crate::artifact`]).
pub trait ArtifactStorage {
    type Error;

    /// Offers `pending` -- every byte appended since the last time
    /// `durable_offset` advanced, in order, starting at `durable_offset` --
    /// to the backend. The backend may commit all, some (a prefix, down to
    /// zero bytes), or all of it right now; whatever alignment or batching
    /// governs that is the backend's own decision, invisible here.
    ///
    /// Returns the new durable watermark: `durable_offset..=durable_offset +
    /// pending.len()`, and never smaller than `durable_offset` (durability
    /// only ever moves forward). The engine re-sends the *whole* undurable
    /// tail on every call, not just what's new, specifically so the backend
    /// never has to remember bytes across calls itself.
    fn write(&mut self, durable_offset: u64, pending: &[u8]) -> Result<u64, Self::Error>;

    /// Forces every byte in `pending` to become durable -- the final,
    /// possibly short-of-a-full-unit flush a stream's last call needs.
    /// Must return `durable_offset + pending.len() as u64` (everything
    /// consumed) or fail; a backend that can only align to physical units
    /// pads internally, but that padding must never be reported back as
    /// part of the durable watermark (see [`crate::artifact`]'s invariant
    /// on logical vs. physical length).
    fn finish(&mut self, durable_offset: u64, pending: &[u8]) -> Result<u64, Self::Error>;
}

/// A small, atomically-publishable record of where the transaction stands --
/// see the module doc comment for what "atomically" is trusted to mean here.
pub trait TransactionMetadata {
    type Error;
    type Record: Clone;

    /// The last record a `commit` durably published, or `None` if none was
    /// ever committed (or the last one was committed as `None`, i.e.
    /// cleared). Must never return a record from an interrupted `commit` --
    /// an incomplete publish must surface as whatever came *before* it.
    fn load(&mut self) -> Result<Option<Self::Record>, Self::Error>;

    /// Publishes `record` as the new state (`None` clears it). See the
    /// module doc comment: this is the one operation this crate trusts to
    /// be indivisible from `load`'s point of view, at any point a restart
    /// might land -- composing that guarantee out of weaker per-field
    /// writes (if that's what a given backend has to do) is the backend
    /// implementation's job, not something this trait does for it.
    fn commit(&mut self, record: Option<&Self::Record>) -> Result<(), Self::Error>;
}
