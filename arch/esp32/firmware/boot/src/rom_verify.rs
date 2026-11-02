//! SHA-256 and RSA-PSS 3072 through the ESP32-C3/S3 mask ROM, for the verified boot profile.
//!
//! The ROM exports `ets_sha_*` (hardware SHA) and `ets_rsa_pss_verify` (RSA accelerator);
//! `esp-rom-sys` already provides their addresses to the linker. Using them costs about a
//! kilobyte of code where a software implementation would not fit in the 32 KiB second stage.
//!
//! # The SHA contract
//!
//! Established on the ROM images themselves (Espressif's `esp-rom-elfs` 20260528: ESP32-S3 rev 0,
//! ESP32-C3 rev 0, 101 and 3), not only on the headers (`esp_rom/<chip>/include/<chip>/rom/sha.h`):
//!
//! * the context is the ROM's `SHA_CTX`: **216 bytes, 4-byte aligned**: `start` (u8) at 0,
//!   `in_hardware` (u8) at 1, `type` (u32) at 4, `state[16]` at 8, `buffer[128]` at 72,
//!   `total_bits[4]` at 200. `ets_sha_init` clears all 216 bytes and sets `type`;
//!   `ets_sha_update` keeps the bit counter at 200..216 and a partial block at 72; `ets_sha_finish`
//!   reads the counter to pad, then copies `sha_result_bytes[type]` (32 for SHA-256) from `state`;
//! * a context declared shorter than that is overrun by the ROM (the first version declared 80
//!   bytes: `ets_sha_init` cleared 136 bytes of its caller's frame, and a copy of the 80 bytes lost
//!   the bit counter, so the length that was hashed was the one of the caller's memory);
//! * no routine keeps the address of the context: moving a whole context is safe, truncating it
//!   is not. The hash *state* lives in the SHA peripheral while `in_hardware` is set (every
//!   `update` here passes `update_ctx = false`), so nothing else may use the peripheral between
//!   `new` and `finish`. `ets_rsa_pss_verify` does (it enables, uses and disables it), so no hash
//!   of ours is ever alive across it: `RomRsaPss3072::verify` finishes its own before the call;
//! * `ets_sha_enable` / `ets_sha_disable` only set the clock and the reset of the peripheral,
//!   with no reference count;
//! * `ets_rsa_pss_verify(key, signature, digest, verified_digest)`: the key is the 776-byte
//!   `ets_rsa_pubkey_t`, `verified_digest` is written for exactly 32 bytes.
//!
//! All pointers handed to the ROM are word aligned. Unsafe entry points require the sole boot
//! phase (no concurrent users of the SHA or RSA peripherals), like the rest of this crate.

use core::mem::{align_of, offset_of, size_of};

use iobewi_firmware_auth::{Algorithm, SignatureVerifier, VerifyError};
use iobewi_firmware_bootauth::{Aligned, Sha256Stream};

/// The ROM's `SHA_CTX`, field for field (see the module documentation).
#[repr(C)]
struct ShaContext {
    start: u8,
    in_hardware: u8,
    kind: u32,
    state: [u32; 16],
    buffer: [u8; 128],
    total_bits: [u32; 4],
}

// The layout is the ROM's: a mismatch must not build.
const _: () = {
    assert!(size_of::<ShaContext>() == 216);
    assert!(align_of::<ShaContext>() == 4);
    assert!(offset_of!(ShaContext, start) == 0);
    assert!(offset_of!(ShaContext, in_hardware) == 1);
    assert!(offset_of!(ShaContext, kind) == 4);
    assert!(offset_of!(ShaContext, state) == 8);
    assert!(offset_of!(ShaContext, buffer) == 72);
    assert!(offset_of!(ShaContext, total_bits) == 200);
};

impl ShaContext {
    const fn zeroed() -> Self {
        Self {
            start: 0,
            in_hardware: 0,
            kind: 0,
            state: [0; 16],
            buffer: [0; 128],
            total_bits: [0; 4],
        }
    }
}

/// Words that follow the context in memory. The ROM never writes there; if a ROM revision with a
/// larger context ever did, this is where it would show, before it reached anything of ours.
const CANARY: [u32; 2] = [0x5348_4143, 0x414E_4152];

#[repr(C)]
struct GuardedContext {
    context: ShaContext,
    canary: [u32; 2],
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

/// Streaming SHA-256 on the ROM and the SHA peripheral.
///
/// Created cheaply and **started on first use**: the ROM context is initialised where it will
/// live for the whole hash, never in a temporary that is then moved.
pub struct RomSha256 {
    ctx: GuardedContext,
    started: bool,
}

impl RomSha256 {
    fn start(&mut self) {
        if !self.started {
            // SAFETY: sole boot phase; `ctx.context` is the ROM's 216-byte, word-aligned context.
            unsafe {
                ets_sha_enable();
                ets_sha_init(&mut self.ctx.context, SHA2_256);
            }
            self.started = true;
            self.check_canary();
        }
    }

    fn check_canary(&self) {
        // A write past the context would have clobbered whatever follows it: refuse to go on
        // with a hash of unknown memory state. (Never taken on the verified ROM revisions.)
        assert!(self.ctx.canary == CANARY, "ROM SHA overran its context");
    }
}

impl Sha256Stream for RomSha256 {
    fn new() -> Self {
        Self {
            ctx: GuardedContext {
                context: ShaContext::zeroed(),
                canary: CANARY,
            },
            started: false,
        }
    }

    fn update(&mut self, data: &[u8]) {
        self.start();
        if (data.as_ptr() as usize).is_multiple_of(4) {
            // SAFETY: pointer and length come from a live slice; the ROM only reads it.
            unsafe {
                ets_sha_update(
                    &mut self.ctx.context,
                    data.as_ptr(),
                    data.len() as u32,
                    false,
                )
            };
            return;
        }
        // The ROM reads words: bounce an unaligned slice through an aligned buffer.
        let mut block = Aligned([0u8; 64]);
        for piece in data.chunks(block.0.len()) {
            block.0[..piece.len()].copy_from_slice(piece);
            // SAFETY: as above, on the aligned copy.
            unsafe {
                ets_sha_update(
                    &mut self.ctx.context,
                    block.0.as_ptr(),
                    piece.len() as u32,
                    false,
                )
            };
        }
    }

    fn finish(mut self) -> [u8; 32] {
        self.start();
        let mut out = Aligned([0u8; 32]);
        // SAFETY: `out` is a word-aligned 32-byte buffer; the ROM writes `sha_result_bytes[SHA2_256]`
        // = 32 bytes there.
        unsafe { ets_sha_finish(&mut self.ctx.context, out.0.as_mut_ptr()) };
        self.check_canary();
        out.0
    }
}

impl Drop for RomSha256 {
    fn drop(&mut self) {
        if self.started {
            // The peripheral is left gated and in reset, as the ROM leaves it, also when a hash is abandoned.
            // SAFETY: sole boot phase.
            unsafe { ets_sha_disable() };
        }
    }
}

#[cfg(all(feature = "sha-hal", feature = "sha-soft"))]
compile_error!("choose one SHA-256 backend: `sha-hal` or `sha-soft` (default: the ROM's)");

/// The SHA-256 of the verified boot profile: one choice per build, shared by the policy and by the
/// message digest of the RSA-PSS check below.
#[cfg(not(any(feature = "sha-hal", feature = "sha-soft")))]
pub type Sha256Backend = RomSha256;
#[cfg(feature = "sha-hal")]
pub type Sha256Backend = crate::hal_sha::HalSha256;
#[cfg(feature = "sha-soft")]
pub type Sha256Backend = iobewi_firmware_bootauth::SoftSha256;

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
        let digest = Aligned(Sha256Backend::digest(signed));
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
