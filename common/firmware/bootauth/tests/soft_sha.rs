//! The portable software SHA-256 against known vectors and the reference implementation.
#![cfg(feature = "soft-sha256")]
#![forbid(unsafe_code)]

use iobewi_firmware_bootauth::{Sha256Stream, SoftSha256};
use sha2::{Digest, Sha256};

fn hex(d: [u8; 32]) -> String {
    d.iter().map(|b| format!("{b:02x}")).collect()
}

fn pseudo(len: usize, seed: u32) -> Vec<u8> {
    let mut x = seed;
    (0..len)
        .map(|_| {
            x = x.wrapping_mul(1664525).wrapping_add(1013904223);
            (x >> 24) as u8
        })
        .collect()
}

#[test]
fn nist_vectors() {
    let cases: [(&[u8], &str); 3] = [
        (
            b"",
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ),
        (
            b"abc",
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        ),
        (
            b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
        ),
    ];
    for (msg, want) in cases {
        assert_eq!(hex(SoftSha256::digest(msg)), want);
    }
}

#[test]
fn a_million_a() {
    let mut h = SoftSha256::new();
    let buf = [b'a'; 1000];
    for _ in 0..1000 {
        h.update(&buf);
    }
    assert_eq!(
        hex(h.finish()),
        "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
    );
}

#[test]
fn every_length_around_the_block_boundaries_and_any_chunking() {
    for len in (0..=200).chain([255, 256, 257, 4095, 4096, 4097]) {
        let data = pseudo(len, len as u32 + 1);
        let want: [u8; 32] = Sha256::digest(&data).into();
        assert_eq!(SoftSha256::digest(&data), want, "length {len}");
        for chunk in [1, 3, 63, 64, 65, 97] {
            let mut h = SoftSha256::new();
            for piece in data.chunks(chunk) {
                h.update(piece);
            }
            assert_eq!(h.finish(), want, "length {len}, chunks of {chunk}");
        }
    }
}

#[test]
fn a_slot_sized_message_read_256_bytes_at_a_time() {
    let data = pseudo(1_049_840, 7);
    let mut h = SoftSha256::new();
    for piece in data.chunks(256) {
        h.update(piece);
    }
    assert_eq!(h.finish(), <[u8; 32]>::from(Sha256::digest(&data)));
}
