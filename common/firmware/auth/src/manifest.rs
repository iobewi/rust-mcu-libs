//! IAM1 v1: the canonical, fixed-layout manifest that a signature covers.
//!
//! Fixed offsets and no optional field leave no second way to encode the same
//! manifest: decoding rejects every deviation instead of normalizing it.
//!
//! | offset | size | field |
//! |---|---|---|
//! | 0 | 4 | magic `IAM1` |
//! | 4 | 1 | version, `1` |
//! | 5 | 1 | algorithm: `1` ECDSA P-256/SHA-256; `2`, `3` reserved |
//! | 6 | 1 | image format: `1` ESP application image |
//! | 7 | 1 | MCU family: `1` ESP32-C3, `2` ESP32-S3, `3` RP2350, `4` i.MX RT1062 |
//! | 8 | 8 | key id: first 8 bytes of SHA-256 of the public key |
//! | 16 | 16 | product tag: printable ASCII, zero padded, not empty |
//! | 32 | 2 | hardware variant |
//! | 34 | 2 | reserved, `0` |
//! | 36 | 4 | security version |
//! | 40 | 4 | image size in bytes |
//! | 44 | 32 | SHA-256 of the image |
//!
//! The signature covers [`DOMAIN`] followed by the 76 manifest bytes. The
//! envelope is the manifest followed by the signature (64 bytes for ECDSA).

use crate::error::DecodeError;
use iobewi_firmware_image::Digest;
use sha2::{Digest as _, Sha256};

pub const MAGIC: [u8; 4] = *b"IAM1";
pub const VERSION: u8 = 1;
pub const MANIFEST_LEN: usize = 76;
/// Domain separation: a signature over a manifest is never valid in another protocol.
pub const DOMAIN: &[u8; 20] = b"iobewi.ota.image.v1\0";
pub const SIGNED_LEN: usize = DOMAIN.len() + MANIFEST_LEN;
/// Largest accepted envelope (room for a future RSA-PSS 3072 signature).
pub const MAX_ENVELOPE: usize = 512;
/// Largest base64url text of [`MAX_ENVELOPE`] bytes.
pub const MAX_ENVELOPE_TEXT: usize = MAX_ENVELOPE.div_ceil(3) * 4;
pub const PRODUCT_LEN: usize = 16;
pub const ECDSA_P256_SIGNATURE_LEN: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Algorithm {
    EcdsaP256Sha256,
}

impl Algorithm {
    pub const fn id(self) -> u8 {
        match self {
            Self::EcdsaP256Sha256 => 1,
        }
    }

    /// Length of the public key in its pinned encoding.
    pub const fn public_key_len(self) -> usize {
        match self {
            // SEC1 uncompressed point: 0x04 | X | Y.
            Self::EcdsaP256Sha256 => 65,
        }
    }

    pub const fn signature_len(self) -> usize {
        match self {
            Self::EcdsaP256Sha256 => ECDSA_P256_SIGNATURE_LEN,
        }
    }

    fn from_id(id: u8) -> Result<Self, DecodeError> {
        match id {
            1 => Ok(Self::EcdsaP256Sha256),
            2 | 3 => Err(DecodeError::UnsupportedAlgorithm),
            _ => Err(DecodeError::Algorithm),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    EspApp,
}

impl ImageFormat {
    const fn id(self) -> u8 {
        match self {
            Self::EspApp => 1,
        }
    }

    fn from_id(id: u8) -> Result<Self, DecodeError> {
        match id {
            1 => Ok(Self::EspApp),
            _ => Err(DecodeError::ImageFormat),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mcu {
    Esp32C3,
    Esp32S3,
    Rp2350,
    Imxrt1062,
}

impl Mcu {
    const fn id(self) -> u8 {
        match self {
            Self::Esp32C3 => 1,
            Self::Esp32S3 => 2,
            Self::Rp2350 => 3,
            Self::Imxrt1062 => 4,
        }
    }

    fn from_id(id: u8) -> Result<Self, DecodeError> {
        match id {
            1 => Ok(Self::Esp32C3),
            2 => Ok(Self::Esp32S3),
            3 => Ok(Self::Rp2350),
            4 => Ok(Self::Imxrt1062),
            _ => Err(DecodeError::Mcu),
        }
    }
}

/// First 8 bytes of the SHA-256 of the public key, in its pinned encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyId(pub [u8; 8]);

impl KeyId {
    pub fn of(public_key: &[u8]) -> Self {
        let hash = Sha256::digest(public_key);
        let mut id = [0u8; 8];
        id.copy_from_slice(&hash[..8]);
        Self(id)
    }
}

/// Printable ASCII (`0x21..=0x7e`), zero padded to 16 bytes, never empty.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Product([u8; PRODUCT_LEN]);

impl core::fmt::Debug for Product {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Product {
    pub fn new(tag: &str) -> Option<Self> {
        let bytes = tag.as_bytes();
        if bytes.is_empty() || bytes.len() > PRODUCT_LEN {
            return None;
        }
        let mut out = [0u8; PRODUCT_LEN];
        out[..bytes.len()].copy_from_slice(bytes);
        Self::from_bytes(out).ok()
    }

    fn from_bytes(bytes: [u8; PRODUCT_LEN]) -> Result<Self, DecodeError> {
        let used = bytes.iter().position(|b| *b == 0).unwrap_or(PRODUCT_LEN);
        if used == 0
            || !bytes[..used].iter().all(|b| (0x21..=0x7e).contains(b))
            || bytes[used..].iter().any(|b| *b != 0)
        {
            return Err(DecodeError::Product);
        }
        Ok(Self(bytes))
    }

    pub fn as_str(&self) -> &str {
        let used = self.0.iter().position(|b| *b == 0).unwrap_or(PRODUCT_LEN);
        // Validated as printable ASCII at construction.
        core::str::from_utf8(&self.0[..used]).unwrap_or("")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Manifest {
    pub algorithm: Algorithm,
    pub image_format: ImageFormat,
    pub mcu: Mcu,
    pub key_id: KeyId,
    pub product: Product,
    pub hw_variant: u16,
    pub security_version: u32,
    pub size: u32,
    pub digest: Digest,
}

impl Manifest {
    pub fn encode(&self) -> [u8; MANIFEST_LEN] {
        let mut out = [0u8; MANIFEST_LEN];
        out[..4].copy_from_slice(&MAGIC);
        out[4] = VERSION;
        out[5] = self.algorithm.id();
        out[6] = self.image_format.id();
        out[7] = self.mcu.id();
        out[8..16].copy_from_slice(&self.key_id.0);
        out[16..32].copy_from_slice(&self.product.0);
        out[32..34].copy_from_slice(&self.hw_variant.to_le_bytes());
        // 34..36 reserved, zero.
        out[36..40].copy_from_slice(&self.security_version.to_le_bytes());
        out[40..44].copy_from_slice(&self.size.to_le_bytes());
        out[44..76].copy_from_slice(&self.digest.0);
        out
    }

    pub fn decode(raw: &[u8]) -> Result<Self, DecodeError> {
        let raw: &[u8; MANIFEST_LEN] = raw.try_into().map_err(|_| DecodeError::Length)?;
        if raw[..4] != MAGIC {
            return Err(DecodeError::Magic);
        }
        if raw[4] != VERSION {
            return Err(DecodeError::Version);
        }
        if raw[34] != 0 || raw[35] != 0 {
            return Err(DecodeError::Reserved);
        }
        let mut key_id = [0u8; 8];
        key_id.copy_from_slice(&raw[8..16]);
        let mut product = [0u8; PRODUCT_LEN];
        product.copy_from_slice(&raw[16..32]);
        let mut digest = [0u8; 32];
        digest.copy_from_slice(&raw[44..76]);
        Ok(Self {
            algorithm: Algorithm::from_id(raw[5])?,
            image_format: ImageFormat::from_id(raw[6])?,
            mcu: Mcu::from_id(raw[7])?,
            key_id: KeyId(key_id),
            product: Product::from_bytes(product)?,
            hw_variant: u16::from_le_bytes([raw[32], raw[33]]),
            security_version: u32::from_le_bytes([raw[36], raw[37], raw[38], raw[39]]),
            size: u32::from_le_bytes([raw[40], raw[41], raw[42], raw[43]]),
            digest: Digest(digest),
        })
    }
}

/// The exact bytes a signature covers: [`DOMAIN`] then the manifest.
pub fn signed_bytes(manifest: &[u8; MANIFEST_LEN]) -> [u8; SIGNED_LEN] {
    let mut out = [0u8; SIGNED_LEN];
    out[..DOMAIN.len()].copy_from_slice(DOMAIN);
    out[DOMAIN.len()..].copy_from_slice(manifest);
    out
}

/// A canonical envelope: manifest bytes, their decoding, and the signature.
#[derive(Debug, Clone, Copy)]
pub struct Envelope<'a> {
    pub manifest: Manifest,
    pub manifest_bytes: [u8; MANIFEST_LEN],
    pub signature: &'a [u8],
}

impl<'a> Envelope<'a> {
    pub fn decode(raw: &'a [u8]) -> Result<Self, DecodeError> {
        if raw.len() < MANIFEST_LEN || raw.len() > MAX_ENVELOPE {
            return Err(DecodeError::Length);
        }
        let manifest = Manifest::decode(&raw[..MANIFEST_LEN])?;
        if raw.len() != MANIFEST_LEN + manifest.algorithm.signature_len() {
            return Err(DecodeError::Length);
        }
        let mut manifest_bytes = [0u8; MANIFEST_LEN];
        manifest_bytes.copy_from_slice(&raw[..MANIFEST_LEN]);
        Ok(Self {
            manifest,
            manifest_bytes,
            signature: &raw[MANIFEST_LEN..],
        })
    }

    /// Manifest followed by signature, written into `out`; `None` if it does not fit.
    pub fn encode(manifest: &Manifest, signature: &[u8], out: &mut [u8]) -> Option<usize> {
        let total = MANIFEST_LEN + signature.len();
        if out.len() < total || signature.len() != manifest.algorithm.signature_len() {
            return None;
        }
        out[..MANIFEST_LEN].copy_from_slice(&manifest.encode());
        out[MANIFEST_LEN..total].copy_from_slice(signature);
        Some(total)
    }
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Strict unpadded base64url: no padding, no whitespace, no other alphabet,
/// and zero trailing bits, so every byte string has exactly one text form.
pub fn decode_b64url(text: &[u8], out: &mut [u8]) -> Result<usize, DecodeError> {
    if text.len() % 4 == 1 {
        return Err(DecodeError::Base64);
    }
    let decoded = text.len() / 4 * 3 + [0, 0, 1, 2][text.len() % 4];
    if decoded > out.len() {
        return Err(DecodeError::Length);
    }
    let value = |c: u8| -> Result<u32, DecodeError> {
        B64.iter()
            .position(|x| *x == c)
            .map(|v| v as u32)
            .ok_or(DecodeError::Base64)
    };
    let mut written = 0;
    for chunk in text.chunks(4) {
        let mut acc = 0u32;
        for (i, c) in chunk.iter().enumerate() {
            acc |= value(*c)? << (18 - 6 * i as u32);
        }
        let bytes = acc.to_be_bytes();
        let n = chunk.len() - 1;
        if n < 3 && (acc & (0x00ff_ffff >> (8 * n))) != 0 {
            return Err(DecodeError::Base64);
        }
        out[written..written + n].copy_from_slice(&bytes[1..1 + n]);
        written += n;
    }
    Ok(written)
}

/// Unpadded base64url of `input` into `out`; `None` if `out` is too small.
pub fn encode_b64url(input: &[u8], out: &mut [u8]) -> Option<usize> {
    let needed = input.len() / 3 * 4 + [0, 2, 3][input.len() % 3];
    if out.len() < needed {
        return None;
    }
    let mut at = 0;
    for chunk in input.chunks(3) {
        let mut acc = 0u32;
        for (i, b) in chunk.iter().enumerate() {
            acc |= (*b as u32) << (16 - 8 * i as u32);
        }
        for i in 0..=chunk.len() {
            out[at] = B64[((acc >> (18 - 6 * i as u32)) & 63) as usize];
            at += 1;
        }
    }
    Some(at)
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn sample() -> Manifest {
        Manifest {
            algorithm: Algorithm::EcdsaP256Sha256,
            image_format: ImageFormat::EspApp,
            mcu: Mcu::Esp32S3,
            key_id: KeyId([1, 2, 3, 4, 5, 6, 7, 8]),
            product: Product::new("iobewi-ota").unwrap(),
            hw_variant: 7,
            security_version: 9,
            size: 1_025_744,
            digest: Digest([0xab; 32]),
        }
    }

    #[test]
    fn golden_layout_is_fixed() {
        let bytes = sample().encode();
        assert_eq!(&bytes[..8], b"IAM1\x01\x01\x01\x02");
        assert_eq!(&bytes[8..16], &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(&bytes[16..26], b"iobewi-ota");
        assert_eq!(&bytes[26..32], &[0; 6]);
        assert_eq!(&bytes[32..36], &[7, 0, 0, 0]);
        assert_eq!(&bytes[36..40], &[9, 0, 0, 0]);
        assert_eq!(&bytes[40..44], &1_025_744u32.to_le_bytes());
        assert_eq!(&bytes[44..], &[0xab; 32]);
        assert_eq!(Manifest::decode(&bytes), Ok(sample()));
    }

    #[test]
    fn every_non_canonical_manifest_is_rejected() {
        let good = sample().encode();
        let flip = |at: usize, value: u8| {
            let mut b = good;
            b[at] = value;
            Manifest::decode(&b)
        };
        assert_eq!(flip(0, b'X'), Err(DecodeError::Magic));
        assert_eq!(flip(4, 2), Err(DecodeError::Version));
        assert_eq!(flip(5, 2), Err(DecodeError::UnsupportedAlgorithm));
        assert_eq!(flip(5, 3), Err(DecodeError::UnsupportedAlgorithm));
        assert_eq!(flip(5, 0), Err(DecodeError::Algorithm));
        assert_eq!(flip(5, 9), Err(DecodeError::Algorithm));
        assert_eq!(flip(6, 2), Err(DecodeError::ImageFormat));
        assert_eq!(flip(7, 0), Err(DecodeError::Mcu));
        assert_eq!(flip(7, 5), Err(DecodeError::Mcu));
        assert_eq!(flip(34, 1), Err(DecodeError::Reserved));
        assert_eq!(flip(35, 1), Err(DecodeError::Reserved));
        // Product: empty, non printable, garbage after the padding.
        assert_eq!(flip(16, 0), Err(DecodeError::Product));
        assert_eq!(flip(16, b' '), Err(DecodeError::Product));
        assert_eq!(flip(20, 0x7f), Err(DecodeError::Product));
        assert_eq!(flip(31, b'x'), Err(DecodeError::Product));
        assert_eq!(Manifest::decode(&good[..75]), Err(DecodeError::Length));
        assert_eq!(Manifest::decode(&[0u8; 77]), Err(DecodeError::Length));
    }

    #[test]
    fn product_tag_rules() {
        assert!(Product::new("").is_none());
        assert!(Product::new("a b").is_none());
        assert!(Product::new("é").is_none());
        assert!(Product::new("seventeen-chars-x").is_none());
        assert_eq!(Product::new("0123456789abcdef").unwrap().as_str().len(), 16);
        assert_eq!(Product::new("iobewi-ota").unwrap().as_str(), "iobewi-ota");
    }

    #[test]
    fn envelope_length_follows_the_algorithm_exactly() {
        let manifest = sample();
        let mut buf = [0u8; 200];
        let n = Envelope::encode(&manifest, &[7u8; 64], &mut buf).unwrap();
        assert_eq!(n, 140);
        assert_eq!(Envelope::decode(&buf[..n]).unwrap().signature, &[7u8; 64]);
        assert_eq!(
            Envelope::decode(&buf[..n - 1]).unwrap_err(),
            DecodeError::Length
        );
        assert_eq!(
            Envelope::decode(&buf[..n + 1]).unwrap_err(),
            DecodeError::Length
        );
        assert!(Envelope::encode(&manifest, &[7u8; 63], &mut buf).is_none());
        assert_eq!(
            Envelope::decode(&[0u8; 513]).unwrap_err(),
            DecodeError::Length
        );
        assert_eq!(Envelope::decode(&[]).unwrap_err(), DecodeError::Length);
    }

    #[test]
    fn signed_bytes_carry_the_domain_prefix() {
        let m = sample().encode();
        let signed = signed_bytes(&m);
        assert_eq!(&signed[..20], b"iobewi.ota.image.v1\0");
        assert_eq!(&signed[20..], &m);
    }

    #[test]
    fn base64url_is_strict_and_round_trips() {
        let mut text = [0u8; 700];
        let mut back = [0u8; 520];
        for len in 0..=64usize {
            let data: [u8; 64] = core::array::from_fn(|i| (i * 37 + len) as u8);
            let n = encode_b64url(&data[..len], &mut text).unwrap();
            assert_eq!(n, (len * 4).div_ceil(3));
            let m = decode_b64url(&text[..n], &mut back).unwrap();
            assert_eq!(&back[..m], &data[..len]);
        }
        let mut out = [0u8; 8];
        for bad in ["a", "AAAAA", "AB==", "AA=", "A B ", "AA+/", "AAA\n", "é"] {
            assert!(decode_b64url(bad.as_bytes(), &mut out).is_err(), "{bad:?}");
        }
        // Non-zero trailing bits: "AB" would be 0x00 with spare bits set.
        assert_eq!(decode_b64url(b"AB", &mut out), Err(DecodeError::Base64));
        assert_eq!(decode_b64url(b"AAB", &mut out), Err(DecodeError::Base64));
        assert_eq!(decode_b64url(b"AA", &mut out), Ok(1));
        assert_eq!(
            decode_b64url(b"AAAAAAAAAAAA", &mut out),
            Err(DecodeError::Length)
        );
    }
}
