//! Borrowed ESP-IDF entry view for ROM boot and runtime discovery.
//! Upstream's byte-table constructor is private and its public reader requires
//! FlashStorage. Keep the missing ROM integration here, with no flash owner.
//! Matches upstream's non-validation mode: stop at first non-entry magic;
//! MD5/authenticity and encrypted boot are not provided by this view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawEntry<'a> {
    pub label: &'a str,
    pub kind: u8,
    pub subtype: u8,
    pub offset: u32,
    pub size: u32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidTable;
pub const MAX_LEN: usize = 0xc00;
pub fn entries(bytes: &[u8]) -> Result<impl Iterator<Item = RawEntry<'_>>, InvalidTable> {
    if bytes.is_empty()
        || bytes.len() > MAX_LEN
        || !bytes.len().is_multiple_of(32)
        || bytes[..2] != [0xaa, 0x50]
    {
        return Err(InvalidTable);
    }
    let words = bytes
        .as_chunks::<32>()
        .0
        .iter()
        .take_while(|b| b[..2] == [0xaa, 0x50]);
    // Validate labels before returning an infallible iterator. Unknown subtypes
    // remain raw bytes; they never go through upstream enum unwraps.
    for b in words.clone() {
        core::str::from_utf8(label(b)).map_err(|_| InvalidTable)?;
    }
    Ok(words.map(|b| RawEntry {
        label: core::str::from_utf8(label(b)).unwrap(),
        kind: b[2],
        subtype: b[3],
        offset: u32::from_le_bytes(b[4..8].try_into().unwrap()),
        size: u32::from_le_bytes(b[8..12].try_into().unwrap()),
    }))
}
fn label(b: &[u8]) -> &[u8] {
    let b = &b[12..28];
    &b[..b.iter().position(|c| *c == 0).unwrap_or(b.len())]
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn custom_subtype_and_terminators_remain_raw() {
        let mut b = [0xff; 96];
        b[..32].fill(0);
        b[..4].copy_from_slice(&[0xaa, 0x50, 1, 0xfe]);
        b[4..8].copy_from_slice(&0x9000u32.to_le_bytes());
        b[8..12].copy_from_slice(&0x1000u32.to_le_bytes());
        b[12..15].copy_from_slice(b"cfg");
        b[32..34].copy_from_slice(&[0xeb, 0xeb]);
        let e = entries(&b).unwrap().next().unwrap();
        assert_eq!(
            (e.label, e.kind, e.subtype, e.offset, e.size),
            ("cfg", 1, 0xfe, 0x9000, 0x1000)
        );
        assert_eq!(entries(&b).unwrap().count(), 1);
    }
    #[test]
    fn malformed_input_is_rejected_without_panicking() {
        for len in 0..65 {
            assert!(entries(&[0xff; 65][..len]).is_err());
        }
        let mut b = [0; 32];
        b[..2].copy_from_slice(&[0xaa, 0x50]);
        b[12] = 0xff;
        assert!(entries(&b).is_err());
    }
}
