#![no_std]
//! Host-testable request admission of the reference firmware, not a library API.

#[cfg(test)]
extern crate std;

pub fn valid_deployment_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

/// Product tag a signed deployment must carry (policy: pinned at build time).
pub const PRODUCT: &str = "iobewi-ota-sys";
/// Hardware variant of this reference firmware.
pub const HW_VARIANT: u16 = 0;

/// Decodes the `X-Image-Manifest` header (unpadded base64url of an IAM1
/// envelope) into `out`; `None` when absent, malformed or not canonical.
pub fn decode_manifest_header(
    text: &str,
    out: &mut [u8; iobewi_firmware_auth::MAX_ENVELOPE],
) -> Option<usize> {
    iobewi_firmware_auth::decode_b64url(text.as_bytes(), out).ok()
}

/// Public key (SEC1 uncompressed, 130 hex digits) from the build environment.
/// A malformed value stops the build: there is no unsigned fallback.
pub const fn parse_public_key(hex: &str) -> [u8; 65] {
    let bytes = hex.as_bytes();
    assert!(
        bytes.len() == 130,
        "IOBEWI_OTA_SIGNING_PUBKEY must be 130 hex digits"
    );
    let mut out = [0u8; 65];
    let mut i = 0;
    while i < 65 {
        out[i] = (nibble(bytes[2 * i]) << 4) | nibble(bytes[2 * i + 1]);
        i += 1;
    }
    assert!(out[0] == 0x04, "uncompressed SEC1 point required");
    out
}

/// RSA-3072 public key in Espressif's encoding (776 bytes: `n`, `e`, `rinv`, `mdash`, little-endian),
/// as 1552 hex digits, from the build environment. A malformed value stops the build: there is no
/// unsigned fallback.
pub const fn parse_rsa_key(hex: &str) -> [u8; 776] {
    let bytes = hex.as_bytes();
    assert!(
        bytes.len() == 1552,
        "IOBEWI_OTA_SIGNING_RSA_KEY must be 1552 hex digits"
    );
    let mut out = [0u8; 776];
    let mut i = 0;
    while i < 776 {
        out[i] = (nibble(bytes[2 * i]) << 4) | nibble(bytes[2 * i + 1]);
        i += 1;
    }
    // A genuine 3072-bit modulus (odd, top bit set) and the exponent 65537.
    assert!(
        out[0] & 1 == 1 && out[383] & 0x80 != 0,
        "not a 3072-bit modulus"
    );
    assert!(
        out[384] == 0x01 && out[385] == 0x00 && out[386] == 0x01 && out[387] == 0x00,
        "public exponent must be 65537"
    );
    out
}

/// Decimal `u32` from the build environment (the static security-version floor).
pub const fn parse_u32(text: &str) -> u32 {
    let bytes = text.as_bytes();
    assert!(!bytes.is_empty(), "empty number");
    let mut value = 0u32;
    let mut i = 0;
    while i < bytes.len() {
        assert!(bytes[i].is_ascii_digit(), "not a decimal digit");
        value = match value.checked_mul(10) {
            Some(v) => match v.checked_add((bytes[i] - b'0') as u32) {
                Some(v) => v,
                None => panic!("u32 overflow"),
            },
            None => panic!("u32 overflow"),
        };
        i += 1;
    }
    value
}

const fn nibble(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        b'A'..=b'F' => c - b'A' + 10,
        _ => panic!("not a hex digit"),
    }
}

pub fn valid_upload(id: &str, digest: &str, length: usize, capacity: usize) -> bool {
    valid_deployment_id(id)
        && length > 0
        && length <= capacity
        && length <= u32::MAX as usize
        && iobewi_firmware_image::parse_digest(digest).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    const DIGEST: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    #[test]
    fn manifest_header_decoding_is_strict() {
        let mut out = [0u8; iobewi_firmware_auth::MAX_ENVELOPE];
        assert_eq!(decode_manifest_header("AAAA", &mut out), Some(3));
        assert_eq!(decode_manifest_header("", &mut out), Some(0));
        assert_eq!(decode_manifest_header("AAA=", &mut out), None);
        assert_eq!(decode_manifest_header("AA AA", &mut out), None);
        assert_eq!(decode_manifest_header("A", &mut out), None);
        assert_eq!(decode_manifest_header(&"A".repeat(700), &mut out), None);
    }

    #[test]
    fn an_rsa_envelope_fits_the_request_header_path() {
        // 76-byte manifest + 384-byte RSA-PSS signature = 460 bytes, 614 base64url characters:
        // inside the envelope buffer, and well inside the 2048-byte HTTP request buffer even
        // next to the other headers of an upload.
        let raw = [0x5au8; 76 + 384];
        let mut text = [0u8; iobewi_firmware_auth::MAX_ENVELOPE_TEXT];
        let n = iobewi_firmware_auth::encode_b64url(&raw, &mut text).unwrap();
        assert_eq!(n, 614);
        let mut out = [0u8; iobewi_firmware_auth::MAX_ENVELOPE];
        let decoded = decode_manifest_header(core::str::from_utf8(&text[..n]).unwrap(), &mut out);
        assert_eq!(decoded, Some(460));
        assert_eq!(&out[..460], &raw[..]);
        let headers = "Authorization: Bearer ".len()
            + 64
            + "X-Deployment-ID: ".len()
            + 64
            + "X-Image-SHA256: ".len()
            + 71
            + "X-Image-Manifest: ".len()
            + n
            + 128;
        assert!(headers < iobewi_http_server_buffer(), "{headers}");
    }

    /// `iobewi_http_server::HTTP_BUFFER_LEN`, restated: this host-only library does not link the
    /// server. The firmware build asserts the real constant is at least this.
    fn iobewi_http_server_buffer() -> usize {
        2048
    }

    #[test]
    fn rsa_key_parsing_is_const_and_strict() {
        let mut good = [0u8; 776];
        good[0] = 1;
        good[383] = 0x80;
        good[384..388].copy_from_slice(&65537u32.to_le_bytes());
        let hex: std::string::String = good.iter().map(|b| std::format!("{b:02x}")).collect();
        assert_eq!(parse_rsa_key(&hex), good);
        for (what, edit) in [
            ("even modulus", (0usize, 0u8)),
            ("short modulus", (383, 0x00)),
            ("exponent", (384, 0x03)),
        ] {
            let mut bad = good;
            bad[edit.0] = edit.1;
            let hex: std::string::String = bad.iter().map(|b| std::format!("{b:02x}")).collect();
            assert!(
                std::panic::catch_unwind(|| parse_rsa_key(&hex)).is_err(),
                "{what}"
            );
        }
        assert!(std::panic::catch_unwind(|| parse_rsa_key("00")).is_err());
    }

    #[test]
    fn public_key_parsing_is_const_and_strict() {
        const KEY: [u8; 65] = parse_public_key(
            "04000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f\
             202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f",
        );
        assert_eq!(KEY[0], 0x04);
        assert_eq!(KEY[1], 0x00);
        assert_eq!(KEY[64], 0x3f);
        assert!(std::panic::catch_unwind(|| parse_public_key("04")).is_err());
        assert!(std::panic::catch_unwind(|| parse_public_key(&"zz".repeat(65))).is_err());
        assert!(std::panic::catch_unwind(|| parse_public_key(&"02".repeat(65))).is_err());
    }

    #[test]
    fn rejects_invalid_request_before_superseding_or_erasing() {
        assert!(valid_upload("deploy-1", DIGEST, 1024, 1024));
        assert!(!valid_upload("deploy-1", DIGEST, 1025, 1024));
        assert!(!valid_upload("deploy-1", DIGEST, 0, 1024));
        assert!(!valid_upload("deploy-1", "sha256:zz", 1, 1024));
        assert!(!valid_upload("", DIGEST, 1, 1024));
        assert!(!valid_upload("foreign\r\nheader", DIGEST, 1, 1024));
        assert!(!valid_deployment_id("é"));
        assert!(!valid_deployment_id(
            core::str::from_utf8(&[b'a'; 65]).unwrap()
        ));
        assert!(valid_deployment_id(
            core::str::from_utf8(&[b'a'; 64]).unwrap()
        ));
    }
}
