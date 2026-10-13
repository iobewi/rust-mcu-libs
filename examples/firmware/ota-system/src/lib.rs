#![no_std]
//! Host-testable request admission of the reference firmware, not a library API.

pub fn valid_deployment_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
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
