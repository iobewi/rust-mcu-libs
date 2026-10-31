//! IAP1: the boot proof of one application slot, stored in the slot's last sector.
//!
//! ```text
//!   0  magic      "IAP1"
//!   4  version    u16 = 1
//!   6  algorithm  u16            IAM1 algorithm identifier
//!   8  length     u32            payload length, fixed by the algorithm
//!  12  crc32      u32            over bytes 0..12 and the payload
//!  16  commit     u32 = COMMIT   programmed last, by its own flash command
//!  20  payload                   IAM1 envelope | public key
//! ```
//!
//! The proof is bound to its image by the IAM1 manifest it carries (size and SHA-256), not by
//! position: a stale or foreign proof simply fails the digest comparison. A write interrupted
//! at any point leaves an erased sector, a header without commit word, or the complete
//! record. Nothing here decides authenticity: the CRC only tells a damaged record from a
//! forged one (different log, same refusal).

use iobewi_firmware_auth::Algorithm;
use iobewi_firmware_auth::manifest::MANIFEST_LEN;

pub const MAGIC: [u8; 4] = *b"IAP1";
pub const VERSION: u16 = 1;
pub const HEADER_LEN: usize = 20;
pub const OFF_CRC: usize = 12;
pub const OFF_COMMIT: usize = 16;
/// Balanced bits: a partly programmed word is never this value.
pub const COMMIT: u32 = 0x5AC3_A53C;
pub const SECTOR: usize = iobewi_firmware_layout::domain_a::PROOF_SECTOR_SIZE as usize;
/// Largest payload (RSA-PSS 3072): envelope 460 + key 776.
pub const MAX_PAYLOAD: usize = MANIFEST_LEN + 384 + 776;

/// Payload length of an algorithm: envelope (manifest + signature) then public key.
pub const fn payload_len(algorithm: Algorithm) -> usize {
    envelope_len(algorithm) + algorithm.public_key_len()
}

pub const fn envelope_len(algorithm: Algorithm) -> usize {
    MANIFEST_LEN + algorithm.signature_len()
}

/// What the first [`HEADER_LEN`] bytes of a proof sector say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Header {
    /// All erased: no proof was ever written (or its erase completed).
    Erased,
    /// A well-formed header whose commit word is still erased: a write was interrupted.
    Truncated,
    /// Anything else that is not an exact, committed header.
    Corrupt,
    /// Exact and committed. The payload still has to pass its CRC.
    Committed { algorithm: Algorithm, crc: u32 },
}

fn word(raw: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([raw[at], raw[at + 1], raw[at + 2], raw[at + 3]])
}

fn algorithm_of(id: u16) -> Option<Algorithm> {
    [Algorithm::EcdsaP256Sha256, Algorithm::RsaPss3072Sha256]
        .into_iter()
        .find(|a| u16::from(a.id()) == id)
}

pub fn classify(header: &[u8; HEADER_LEN]) -> Header {
    if header.iter().all(|b| *b == 0xFF) {
        return Header::Erased;
    }
    let algorithm = u16::from_le_bytes([header[6], header[7]]);
    let Some(algorithm) = algorithm_of(algorithm) else {
        return Header::Corrupt;
    };
    let exact = header[..4] == MAGIC
        && u16::from_le_bytes([header[4], header[5]]) == VERSION
        && word(header, 8) as usize == payload_len(algorithm);
    if !exact {
        return Header::Corrupt;
    }
    match word(header, OFF_COMMIT) {
        COMMIT => Header::Committed {
            algorithm,
            crc: word(header, OFF_CRC),
        },
        u32::MAX => Header::Truncated,
        _ => Header::Corrupt,
    }
}

/// CRC32 of header bytes 0..12 followed by the payload.
pub fn crc(header: &[u8], payload: &[u8]) -> u32 {
    let first = iobewi_firmware_boot::crc32_le(u32::MAX, &header[..OFF_CRC]);
    iobewi_firmware_boot::crc32_le(first, payload)
}

/// The three flash commands that write a proof, in the order they must run.
/// An executor erases the sector, then programs `head` (header without commit word) and
/// `payload`, reads both back, and only then programs the commit word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Write<'a> {
    pub head: [u8; OFF_COMMIT],
    pub payload: &'a [u8],
}

impl<'a> Write<'a> {
    /// `envelope` and `key` must have exactly the lengths `algorithm` fixes.
    pub fn new(
        algorithm: Algorithm,
        envelope: &[u8],
        key: &[u8],
        out: &'a mut [u8; MAX_PAYLOAD],
    ) -> Option<Self> {
        if envelope.len() != envelope_len(algorithm) || key.len() != algorithm.public_key_len() {
            return None;
        }
        let total = payload_len(algorithm);
        out[..envelope.len()].copy_from_slice(envelope);
        out[envelope.len()..total].copy_from_slice(key);
        let mut head = [0xFFu8; OFF_COMMIT];
        head[..4].copy_from_slice(&MAGIC);
        head[4..6].copy_from_slice(&VERSION.to_le_bytes());
        head[6..8].copy_from_slice(&u16::from(algorithm.id()).to_le_bytes());
        head[8..12].copy_from_slice(&(total as u32).to_le_bytes());
        let crc = crc(&head, &out[..total]);
        head[OFF_CRC..OFF_COMMIT].copy_from_slice(&crc.to_le_bytes());
        // Flash programs whole words: the tail stays erased (0xFF) and is not part of the CRC.
        let padded = total.next_multiple_of(4);
        out[total..padded].fill(0xFF);
        Some(Self {
            head,
            payload: &out[..padded],
        })
    }
}
