//! SHA-256 and RSA-PSS 3072 through the ESP32-C3/S3 mask ROM, for the verified boot profile.
//!
//! The ROM exports `ets_sha_*` (hardware SHA) and `ets_rsa_pss_verify` (RSA accelerator);
//! `esp-rom-sys` already provides their addresses to the linker. Using them costs about a
//! kilobyte of code where a software implementation would not fit in the 32 KiB second stage.
//!
//! Contract, from ESP-IDF's ROM headers and `bootloader_sha.c`, checked on an ESP32-S3
//! (issue #91, phase A3): the SHA sequence is enable, init(SHA2_256), update(.., false),
//! finish; `ets_rsa_pss_verify(key, signature, digest, out)` takes Espressif's 776-byte
//! little-endian key structure, a little-endian signature and the 32-byte SHA-256 of the
//! message, accepts only a salt of 32 bytes, and needs no other peripheral initialisation.
//! All pointers handed to the ROM are word aligned.
//!
//! Unsafe entry points require the sole boot phase (no concurrent users of the SHA or RSA
//! peripherals), like the rest of this crate.

use iobewi_firmware_auth::{Algorithm, SignatureVerifier, VerifyError};
use iobewi_firmware_bootauth::{Aligned, Sha256Stream};

#[repr(C)]
struct ShaContext {
    state: [u32; 16],
    total_bits: [u32; 4],
}

unsafe extern "C" {
    fn ets_sha_enable();
    fn ets_sha_disable();
    fn ets_sha_init(context: *mut ShaContext, kind: u32) -> i32;
    fn ets_sha_update(context: *mut ShaContext, input: *const u8, len: u32, update_context: bool);
    fn ets_sha_finish(context: *mut ShaContext, output: *mut u8) -> i32;
    fn ets_rsa_pss_verify(
        key: *const u8,
        signature: *const u8,
        digest: *const u8,
        verified_digest: *mut u8,
    ) -> u32;
}

/// `SHA_TYPE::SHA2_256` in the ROM header.
const SHA2_256: u32 = 2;

pub struct RomSha256 {
    context: ShaContext,
}

impl Sha256Stream for RomSha256 {
    fn new() -> Self {
        let mut context = ShaContext {
            state: [0; 16],
            total_bits: [0; 4],
        };
        // SAFETY: sole boot phase; `context` is a valid, word-aligned ROM SHA context.
        unsafe {
            ets_sha_enable();
            ets_sha_init(&mut context, SHA2_256);
        }
        Self { context }
    }

    fn update(&mut self, data: &[u8]) {
        if (data.as_ptr() as usize).is_multiple_of(4) {
            // SAFETY: pointer and length come from a live slice; the ROM only reads it.
            unsafe { ets_sha_update(&mut self.context, data.as_ptr(), data.len() as u32, false) };
            return;
        }
        // The ROM reads words: bounce an unaligned slice through an aligned buffer.
        let mut block = Aligned([0u8; 64]);
        for piece in data.chunks(block.0.len()) {
            block.0[..piece.len()].copy_from_slice(piece);
            // SAFETY: as above, on the aligned copy.
            unsafe {
                ets_sha_update(
                    &mut self.context,
                    block.0.as_ptr(),
                    piece.len() as u32,
                    false,
                )
            };
        }
    }

    fn finish(mut self) -> [u8; 32] {
        let mut out = Aligned([0u8; 32]);
        // SAFETY: `out` is a word-aligned 32-byte buffer, as the ROM expects.
        unsafe {
            ets_sha_finish(&mut self.context, out.0.as_mut_ptr());
            ets_sha_disable();
        }
        out.0
    }
}

/// RSASSA-PSS, 3072-bit, SHA-256, salt 32, public exponent 65537.
#[derive(Debug, Clone, Copy, Default)]
pub struct RomRsaPss3072;

impl SignatureVerifier for RomRsaPss3072 {
    const ALGORITHM: Algorithm = Algorithm::RsaPss3072Sha256;

    fn verify(&self, key: &[u8], signed: &[u8], signature: &[u8]) -> Result<(), VerifyError> {
        if key.len() != 776 {
            return Err(VerifyError::BadKey);
        }
        if signature.len() != 384 {
            return Err(VerifyError::NonCanonical);
        }
        let mut key_copy = Aligned([0u8; 776]);
        let mut signature_copy = Aligned([0u8; 384]);
        key_copy.0.copy_from_slice(key);
        signature_copy.0.copy_from_slice(signature);
        let digest = Aligned(RomSha256::digest(signed));
        let mut verified = Aligned([0u8; 32]);
        // SAFETY: four word-aligned buffers of the sizes the ROM function reads and writes.
        let accepted = unsafe {
            ets_rsa_pss_verify(
                key_copy.0.as_ptr(),
                signature_copy.0.as_ptr(),
                digest.0.as_ptr(),
                verified.0.as_mut_ptr(),
            )
        } & 0xFF
            != 0;
        if accepted {
            Ok(())
        } else {
            Err(VerifyError::BadSignature)
        }
    }
}
