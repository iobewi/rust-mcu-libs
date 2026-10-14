#![no_std]
//! Request admission used by the concrete B route before any persistence or erase.
pub use example_firmware_ota_system::{valid_deployment_id, valid_upload};
use iobewi_firmware_update::compat::otm2::{RuntimeApi, SlotMeta};

pub fn upload_metadata(
    id: &str,
    version: &str,
    digest: &str,
    api: &str,
    length: usize,
) -> Option<SlotMeta> {
    if !valid_deployment_id(id) || version.is_empty() || length < iobewi_workload_image::HEADER_LEN
    {
        return None;
    }
    let digest = iobewi_firmware_image::parse_digest(digest)?;
    let (major, minor) = api.split_once('.')?;
    let api = RuntimeApi::new(major.parse().ok()?, minor.parse().ok()?);
    SlotMeta::new(id, version, digest.0, length as u64, api).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    const DIGEST: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    #[test]
    fn invalid_candidate_is_rejected_before_supersession_or_erase() {
        assert!(upload_metadata("hello", "1", DIGEST, "1.0", 128).is_some());
        for id in ["", "foreign\r\nheader", "component-name-too-long-25"] {
            assert!(upload_metadata(id, "1", DIGEST, "1.0", 128).is_none());
        }
        for api in ["", "1", "1.0.1", "65536.0", "-1.0"] {
            assert!(upload_metadata("hello", "1", DIGEST, api, 128).is_none());
        }
        assert!(upload_metadata("hello", "", DIGEST, "1.0", 128).is_none());
        assert!(upload_metadata("hello", "1\0", DIGEST, "1.0", 128).is_none());
        assert!(upload_metadata("hello", "1", "sha256:zz", "1.0", 128).is_none());
        assert!(upload_metadata("hello", "1", DIGEST, "1.0", 63).is_none());
    }
}
