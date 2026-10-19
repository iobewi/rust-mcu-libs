extern crate std;

use super::*;
use base64::Engine;
use sha2::{Digest, Sha256};

fn der(pem: &str) -> std::vec::Vec<u8> {
    let body: std::string::String = pem
        .lines()
        .filter(|l| !l.starts_with("-----"))
        .collect();
    base64::engine::general_purpose::STANDARD
        .decode(body)
        .expect("PEM body is base64")
}

#[test]
fn every_root_is_one_pem_certificate_matching_its_published_fingerprint() {
    for root in ALL {
        assert_eq!(root.pem.matches("-----BEGIN CERTIFICATE-----").count(), 1, "{}", root.name);
        assert_eq!(root.pem.matches("-----END CERTIFICATE-----").count(), 1, "{}", root.name);
        assert!(root.pem.ends_with('\n'), "{}: PEM must end with a newline", root.name);
        let digest: [u8; 32] = Sha256::digest(der(root.pem)).into();
        assert_eq!(digest, root.sha256, "{}", root.name);
    }
}

#[test]
fn the_bundle_is_the_roots_concatenated_in_order() {
    let joined: std::string::String = ALL.iter().map(|r| r.pem).collect();
    assert_eq!(BUNDLE, joined);
}

#[test]
fn names_are_unique_and_documented() {
    for (i, a) in ALL.iter().enumerate() {
        assert!(!a.reason.is_empty());
        for b in &ALL[i + 1..] {
            assert_ne!(a.name, b.name);
        }
    }
}
