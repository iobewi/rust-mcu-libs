//! The SHA-256 digest of a firmware artifact and its text form.
//!
//! One concrete algorithm rather than a generic hasher trait: it is reused,
//! proven code, not a speculative abstraction.

#[cfg(feature = "alloc")]
extern crate alloc;

#[cfg(feature = "alloc")]
use alloc::string::String;
#[cfg(feature = "alloc")]
use core::fmt::Write as _;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Digest(pub [u8; 32]);

impl core::fmt::Debug for Digest {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "sha256:")?;
        for b in self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

/// `sha256:<64 hex digits>` -> digest. Hex digits are accepted in either case.
pub fn parse_digest(value: &str) -> Option<Digest> {
    let hex = value.strip_prefix("sha256:")?;
    if hex.len() != 64 {
        return None;
    }
    let mut bytes = [0u8; 32];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(Digest(bytes))
}

/// Digest -> `sha256:<lowercase hex>`.
#[cfg(feature = "alloc")]
pub fn format_digest(digest: &Digest) -> String {
    let mut s = String::from("sha256:");
    for b in digest.0 {
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[cfg(all(test, feature = "alloc"))]
mod tests {
    extern crate alloc;
    use super::*;

    #[test]
    fn text_form_round_trips_and_is_lowercase() {
        let digest = Digest([0xab; 32]);
        let text = format_digest(&digest);
        assert_eq!(text, alloc::format!("sha256:{}", "ab".repeat(32)));
        assert_eq!(parse_digest(&text), Some(digest));
        assert_eq!(parse_digest(&text.to_uppercase().replace("SHA256", "sha256")), Some(digest));
    }

    #[test]
    fn malformed_digests_are_rejected() {
        assert_eq!(parse_digest("abab"), None);
        assert_eq!(parse_digest("sha256:abc"), None);
        assert_eq!(parse_digest(&alloc::format!("sha256:{}", "zz".repeat(32))), None);
        assert_eq!(parse_digest(&alloc::format!("sha1:{}", "ab".repeat(32))), None);
    }
}
