//! Transactional frame store for the OTA metadata (OTM1 bytes, verbatim).
//!
//! OTM1 carries neither a checksum nor a sequence, so it cannot tell a torn
//! write from a record. This store wraps the OTM1 bytes in a frame and keeps
//! two copies (see [`crate::double_copy`]): a commit erases and programs the
//! **older** copy, so the previous record stays intact whatever instant a
//! restart lands on. The frame is the persistence protocol; OTM1 stays the
//! business format and is neither read nor changed here.
//!
//! Frame (little-endian, `len` payload bytes zero-padded to a multiple of 4):
//!
//! | offset | size | field |
//! |---|---|---|
//! | 0 | 4 | magic `b"OTSF"` |
//! | 4 | 1 | format version, `1` |
//! | 5 | 1 | reserved, `0` |
//! | 6 | 2 | `len`, at most [`MAX_PAYLOAD`] |
//! | 8 | 4 | sequence, serial-number arithmetic |
//! | 12 | padded `len` | payload |
//! | 12 + padded | 4 | zlib CRC-32 of every byte before it |
//!
//! The CRC detects an interrupted erase or program. It is not an
//! authentication of the record.
//!
//! A region that is not entirely erased yet holds no valid copy (torn,
//! corrupt, other format) is reported as [`Loaded::Corrupted`]: it is never
//! reformatted behind the caller's back, and a commit refuses to write over it.
//! Recovery is [`FrameStore::erase_all`], an explicit reprovisioning step.

use crate::crc32::crc32;
use crate::double_copy::{COPIES, Found, Selection, select};
use alloc::vec::Vec;

pub const MAGIC: [u8; 4] = *b"OTSF";
pub const FORMAT_VERSION: u8 = 1;
/// Largest payload: the OTM1 record limit.
pub const MAX_PAYLOAD: usize = 512;
const HEADER_LEN: usize = 12;
const CRC_LEN: usize = 4;
/// Size of one copy as read from the medium; also the bound of any frame.
pub const MAX_FRAME: usize = HEADER_LEN + MAX_PAYLOAD + CRC_LEN;

const fn padded(len: usize) -> usize {
    (len + 3) & !3
}

/// Where the two copies physically live. The store assumes nothing more than:
/// reading a copy returns its bytes, and a `write` that is interrupted leaves
/// *that copy* torn but the other intact.
pub trait FrameBackend {
    type Error;

    /// Reads the first [`MAX_FRAME`] bytes of `copy` (0 or 1).
    fn read(&mut self, copy: usize, out: &mut [u8; MAX_FRAME]) -> Result<(), Self::Error>;
    /// Replaces the content of `copy` by `frame` (erase, program, verify).
    /// `frame.len()` is a multiple of 4 and at most [`MAX_FRAME`].
    fn write(&mut self, copy: usize, frame: &[u8]) -> Result<(), Self::Error>;
    /// Erases `copy` entirely.
    fn erase(&mut self, copy: usize) -> Result<(), Self::Error>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Loaded {
    /// Both copies erased: nothing was ever stored.
    Blank,
    /// The newest valid record. `degraded`: the other copy holds no valid one.
    Payload {
        bytes: Vec<u8>,
        sequence: u32,
        copy: usize,
        degraded: bool,
    },
    /// No valid copy although a copy is not blank.
    Corrupted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreError<E> {
    Backend(E),
    TooLarge,
    /// The region holds no valid copy: not overwritten, see the module doc.
    Corrupted,
    /// `provision` found a region that already holds a record.
    NotBlank,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Frame {
    sequence: u32,
    payload: Vec<u8>,
}

/// Returns the frame length, or `None` when `payload` does not fit.
fn encode(sequence: u32, payload: &[u8], out: &mut [u8; MAX_FRAME]) -> Option<usize> {
    if payload.len() > MAX_PAYLOAD {
        return None;
    }
    let body = HEADER_LEN + padded(payload.len());
    out.fill(0);
    out[..4].copy_from_slice(&MAGIC);
    out[4] = FORMAT_VERSION;
    out[6..8].copy_from_slice(&(payload.len() as u16).to_le_bytes());
    out[8..12].copy_from_slice(&sequence.to_le_bytes());
    out[HEADER_LEN..HEADER_LEN + payload.len()].copy_from_slice(payload);
    let crc = crc32(&out[..body]);
    out[body..body + CRC_LEN].copy_from_slice(&crc.to_le_bytes());
    Some(body + CRC_LEN)
}

fn decode(raw: &[u8; MAX_FRAME]) -> Found<Frame> {
    if raw.iter().all(|byte| *byte == 0xFF) {
        return Found::Blank;
    }
    if raw[..4] != MAGIC || raw[4] != FORMAT_VERSION || raw[5] != 0 {
        return Found::Invalid;
    }
    let len = usize::from(u16::from_le_bytes([raw[6], raw[7]]));
    if len > MAX_PAYLOAD {
        return Found::Invalid;
    }
    let body = HEADER_LEN + padded(len);
    let stored = u32::from_le_bytes([raw[body], raw[body + 1], raw[body + 2], raw[body + 3]]);
    if crc32(&raw[..body]) != stored || raw[HEADER_LEN + len..body].iter().any(|b| *b != 0) {
        return Found::Invalid;
    }
    let sequence = u32::from_le_bytes([raw[8], raw[9], raw[10], raw[11]]);
    Found::Valid {
        value: Frame {
            sequence,
            payload: raw[HEADER_LEN..HEADER_LEN + len].to_vec(),
        },
        sequence,
    }
}

/// Bytes of a freshly provisioned region of two `erase_size` sectors: copy 0
/// holds `payload` as sequence 1, copy 1 stays erased. This is the image a
/// factory flashes; it is byte-identical to what `FrameStore::provision`
/// programs. `None` when `payload` or the geometry does not fit.
pub fn provisioned_image(erase_size: usize, payload: &[u8]) -> Option<Vec<u8>> {
    let mut frame = [0u8; MAX_FRAME];
    let len = encode(1, payload, &mut frame)?;
    if erase_size < len {
        return None;
    }
    let mut image = alloc::vec![0xFFu8; 2 * erase_size];
    image[..len].copy_from_slice(&frame[..len]);
    Some(image)
}

pub struct FrameStore<B> {
    backend: B,
}

impl<B: FrameBackend> FrameStore<B> {
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
        Ok(match self.selection()? {
            Selection::Blank => Loaded::Blank,
            Selection::Record {
                value,
                copy,
                degraded,
            } => Loaded::Payload {
                sequence: value.sequence,
                bytes: value.payload,
                copy,
                degraded,
            },
            Selection::Corrupted => Loaded::Corrupted,
        })
    }

    fn selection(&mut self) -> Result<Selection<Frame>, B::Error> {
        let mut raw = [0u8; MAX_FRAME];
        let mut found = [Found::Blank, Found::Blank];
        for (copy, slot) in found.iter_mut().enumerate().take(COPIES) {
            self.backend.read(copy, &mut raw)?;
            *slot = decode(&raw);
        }
        Ok(select(found))
    }

    /// Publishes `payload` as the new record. On a blank region the first
    /// commit goes to copy 0 with sequence 1; afterwards it overwrites the
    /// copy that does not hold the newest valid record.
    pub fn commit(&mut self, payload: &[u8]) -> Result<(), StoreError<B::Error>> {
        if payload.len() > MAX_PAYLOAD {
            return Err(StoreError::TooLarge);
        }
        let (target, sequence) = match self.selection().map_err(StoreError::Backend)? {
            Selection::Blank => (0, 1),
            Selection::Record { value, copy, .. } => (1 - copy, value.sequence.wrapping_add(1)),
            Selection::Corrupted => return Err(StoreError::Corrupted),
        };
        let mut frame = [0u8; MAX_FRAME];
        let len = encode(sequence, payload, &mut frame).ok_or(StoreError::TooLarge)?;
        self.backend
            .write(target, &frame[..len])
            .map_err(StoreError::Backend)
    }

    /// Factory provisioning: publishes `payload` as sequence 1 in copy 0 of a
    /// **blank** region (the other copy stays erased). It refuses a region
    /// that holds a record or is corrupted, so it can never overwrite data.
    pub fn provision(&mut self, payload: &[u8]) -> Result<(), StoreError<B::Error>> {
        match self.selection().map_err(StoreError::Backend)? {
            Selection::Blank => self.commit(payload),
            Selection::Record { .. } => Err(StoreError::NotBlank),
            Selection::Corrupted => Err(StoreError::Corrupted),
        }
    }

    /// Explicit reprovisioning: erases both copies. Never called implicitly.
    pub fn erase_all(&mut self) -> Result<(), B::Error> {
        for copy in 0..COPIES {
            self.backend.erase(copy)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// Two in-memory copies; `cut` simulates an interrupted write by keeping
    /// only a prefix of the frame (the rest stays erased) after the erase.
    struct Mem {
        copies: [[u8; MAX_FRAME]; 2],
        cut: Option<usize>,
    }
    impl Mem {
        fn new() -> Self {
            Self {
                copies: [[0xFF; MAX_FRAME]; 2],
                cut: None,
            }
        }
    }
    #[derive(Debug, PartialEq, Eq, Clone, Copy)]
    struct Cut;
    impl FrameBackend for Mem {
        type Error = Cut;
        fn read(&mut self, copy: usize, out: &mut [u8; MAX_FRAME]) -> Result<(), Cut> {
            *out = self.copies[copy];
            Ok(())
        }
        fn write(&mut self, copy: usize, frame: &[u8]) -> Result<(), Cut> {
            self.copies[copy] = [0xFF; MAX_FRAME];
            let keep = self.cut.unwrap_or(frame.len()).min(frame.len());
            self.copies[copy][..keep].copy_from_slice(&frame[..keep]);
            if keep < frame.len() { Err(Cut) } else { Ok(()) }
        }
        fn erase(&mut self, copy: usize) -> Result<(), Cut> {
            self.copies[copy] = [0xFF; MAX_FRAME];
            Ok(())
        }
    }

    fn payload(seed: u8, len: usize) -> Vec<u8> {
        (0..len).map(|i| seed.wrapping_add(i as u8)).collect()
    }
    fn bytes(loaded: &Loaded) -> &[u8] {
        match loaded {
            Loaded::Payload { bytes, .. } => bytes,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_blank_region_loads_as_blank_and_the_first_commit_is_sequence_one() {
        let mut store = FrameStore::new(Mem::new());
        assert_eq!(store.load().unwrap(), Loaded::Blank);
        store.commit(b"first").unwrap();
        assert_eq!(
            store.load().unwrap(),
            Loaded::Payload {
                bytes: b"first".to_vec(),
                sequence: 1,
                copy: 0,
                degraded: true
            }
        );
    }

    #[test]
    fn commits_alternate_copies_and_the_newest_wins() {
        let mut store = FrameStore::new(Mem::new());
        for n in 1..=9u8 {
            store.commit(&payload(n, usize::from(n) * 7)).unwrap();
            let loaded = store.load().unwrap();
            assert_eq!(bytes(&loaded), payload(n, usize::from(n) * 7));
            assert!(matches!(loaded, Loaded::Payload { copy, sequence, .. }
                if copy == usize::from((n - 1) % 2) && sequence == u32::from(n)));
        }
    }

    #[test]
    fn every_length_from_empty_to_the_maximum_round_trips() {
        for len in 0..=MAX_PAYLOAD {
            let mut store = FrameStore::new(Mem::new());
            store.commit(&payload(len as u8, len)).unwrap();
            assert_eq!(bytes(&store.load().unwrap()), payload(len as u8, len));
        }
        let mut store = FrameStore::new(Mem::new());
        assert_eq!(
            store.commit(&vec![0; MAX_PAYLOAD + 1]),
            Err(StoreError::TooLarge)
        );
    }

    #[test]
    fn an_interrupted_commit_leaves_the_previous_record_at_every_prefix() {
        let mut base = FrameStore::new(Mem::new());
        base.commit(&payload(1, 300)).unwrap();
        base.commit(&payload(2, 301)).unwrap();
        let snapshot = base.backend().copies;
        let next = payload(3, 302);
        let mut frame = [0u8; MAX_FRAME];
        let len = encode(3, &next, &mut frame).unwrap();
        for cut in 0..len {
            let mut store = FrameStore::new(Mem {
                copies: snapshot,
                cut: Some(cut),
            });
            assert_eq!(store.commit(&next), Err(StoreError::Backend(Cut)));
            store.backend_mut().cut = None;
            let loaded = store.load().unwrap();
            assert_eq!(bytes(&loaded), payload(2, 301), "cut {cut}");
        }
        // A completed commit publishes the new record.
        let mut store = FrameStore::new(Mem {
            copies: snapshot,
            cut: None,
        });
        store.commit(&next).unwrap();
        assert_eq!(bytes(&store.load().unwrap()), next);
    }

    #[test]
    fn a_torn_first_commit_is_corrupted_and_never_silently_reformatted() {
        let mut store = FrameStore::new(Mem {
            copies: [[0xFF; MAX_FRAME]; 2],
            cut: Some(9),
        });
        assert!(store.commit(b"first").is_err());
        store.backend_mut().cut = None;
        assert_eq!(store.load().unwrap(), Loaded::Corrupted);
        let before = store.backend().copies;
        assert_eq!(store.commit(b"again"), Err(StoreError::Corrupted));
        assert_eq!(
            store.backend().copies,
            before,
            "no write over a corrupt region"
        );
        store.erase_all().unwrap();
        assert_eq!(store.load().unwrap(), Loaded::Blank);
        store.commit(b"again").unwrap();
        assert_eq!(bytes(&store.load().unwrap()), b"again");
    }

    #[test]
    fn every_single_byte_corruption_of_a_copy_is_detected() {
        let mut store = FrameStore::new(Mem::new());
        store.commit(&payload(7, 100)).unwrap();
        store.commit(&payload(8, 101)).unwrap();
        let good = store.backend().copies;
        for copy in 0..2 {
            for at in 0..HEADER_LEN + padded(101) + CRC_LEN {
                for flip in [0x01u8, 0x80, 0xFF] {
                    let mut copies = good;
                    copies[copy][at] ^= flip;
                    let mut damaged = FrameStore::new(Mem { copies, cut: None });
                    let loaded = damaged.load().unwrap();
                    // The undamaged copy still answers; a flipped copy never does.
                    let survivor = if copy == 1 {
                        payload(7, 100)
                    } else {
                        payload(8, 101)
                    };
                    assert_eq!(bytes(&loaded), survivor, "copy {copy} byte {at}");
                }
            }
        }
    }

    #[test]
    fn unknown_magic_version_or_length_is_invalid_not_blank() {
        let mut store = FrameStore::new(Mem::new());
        store.commit(b"x").unwrap();
        for patch in [(0usize, 0x00u8), (4, 2), (5, 1), (7, 0x7F)] {
            let mut copies = store.backend().copies;
            copies[0][patch.0] = patch.1;
            copies[1] = [0xFF; MAX_FRAME];
            let mut damaged = FrameStore::new(Mem { copies, cut: None });
            assert_eq!(damaged.load().unwrap(), Loaded::Corrupted, "{patch:?}");
        }
    }

    #[test]
    fn a_degraded_store_is_repaired_by_the_next_commit() {
        let mut store = FrameStore::new(Mem::new());
        store.commit(b"one").unwrap();
        store.commit(b"two").unwrap();
        store.backend_mut().copies[0][13] ^= 0x55; // damage the older copy
        assert!(matches!(
            store.load().unwrap(),
            Loaded::Payload { degraded: true, .. }
        ));
        store.commit(b"three").unwrap();
        assert!(matches!(
            store.load().unwrap(),
            Loaded::Payload { degraded: false, ref bytes, .. } if bytes == b"three"
        ));
    }

    #[test]
    fn the_sequence_wraps_without_losing_the_newest_record() {
        let mut store = FrameStore::new(Mem::new());
        let mut frame = [0u8; MAX_FRAME];
        let len = encode(u32::MAX - 1, b"a", &mut frame).unwrap();
        store.backend_mut().write(0, &frame[..len]).unwrap();
        for (n, expect) in [u32::MAX, 0, 1, 2].into_iter().enumerate() {
            store.commit(&[n as u8]).unwrap();
            assert!(matches!(
                store.load().unwrap(),
                Loaded::Payload { sequence, ref bytes, .. } if sequence == expect && *bytes == [n as u8]
            ));
        }
    }
}
