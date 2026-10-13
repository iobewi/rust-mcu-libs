//! Legacy CSM1 envelope, independent of NVS and MCU storage.
//!
//! This preserves the existing byte format for compatibility and migration.
//! It provides framing only: atomic publication, durability and checksum
//! protection must be supplied by the persistence engine, not this codec.
//! NVS key limits and entry accounting do not belong to this module.

use alloc::vec::Vec;

pub const MAGIC: [u8; 4] = *b"CSM1";
pub const FLAG_PRESENT: u8 = 0x01;
/// magic (4) + generation (8) + flags (1)
pub const HEADER_LEN: usize = 4 + 8 + 1;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CorruptRecord;

pub fn encode_record(generation: u64, present: bool, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&generation.to_le_bytes());
    out.push(if present { FLAG_PRESENT } else { 0 });
    out.extend_from_slice(payload);
    out
}

/// `(generation, present, payload)`; any unknown flag bit or a bad header is
/// corruption.
pub fn decode_record(raw: &[u8]) -> Result<(u64, bool, &[u8]), CorruptRecord> {
    if raw.len() < HEADER_LEN || raw[..4] != MAGIC {
        return Err(CorruptRecord);
    }
    let mut generation = [0u8; 8];
    generation.copy_from_slice(&raw[4..12]);
    let generation = u64::from_le_bytes(generation);
    let flags = raw[12];
    if flags & !FLAG_PRESENT != 0 {
        return Err(CorruptRecord);
    }
    Ok((generation, flags & FLAG_PRESENT != 0, &raw[HEADER_LEN..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_golden_bytes_and_round_trip() {
        let raw = encode_record(0x0102_0304_0506_0708, true, b"hi");
        assert_eq!(raw, b"CSM1\x08\x07\x06\x05\x04\x03\x02\x01\x01hi");
        assert_eq!(
            decode_record(&raw),
            Ok((0x0102_0304_0506_0708, true, &b"hi"[..]))
        );
        let cleared = encode_record(9, false, &[]);
        assert_eq!(cleared, b"CSM1\x09\0\0\0\0\0\0\0\x00");
        assert_eq!(decode_record(&cleared), Ok((9, false, &[][..])));
    }

    #[test]
    fn corrupt_records_are_rejected() {
        assert_eq!(decode_record(b"CSM1"), Err(CorruptRecord));
        assert_eq!(
            decode_record(b"XXM1\0\0\0\0\0\0\0\0\x01"),
            Err(CorruptRecord)
        );
        assert_eq!(
            decode_record(b"CSM1\0\0\0\0\0\0\0\0\x02"),
            Err(CorruptRecord),
            "unknown flag bit"
        );
        assert_eq!(
            decode_record(b"CSM1\0\0\0\0\0\0\0\0\x81"),
            Err(CorruptRecord)
        );
    }
}
