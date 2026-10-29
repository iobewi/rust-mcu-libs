//! Shared fixtures: an in-memory flash with fault injection, real RSA keys, signed slots.
#![allow(dead_code)]

use std::sync::OnceLock;

use iobewi_firmware_auth::{
    Algorithm, ImageFormat, KeyId, Manifest, Mcu, Product, Target, manifest::MANIFEST_LEN,
};
use iobewi_firmware_boot::transaction::Flash;
use iobewi_firmware_bootauth::host::{HostKey, HostRsaVerifier, HostSha256};
use iobewi_firmware_bootauth::{IoError, SlotRead, VerifiedBoot, iap1, writer};
use iobewi_firmware_image::Digest;
use iobewi_firmware_layout::domain_a::Partition;
use sha2::{Digest as _, Sha256};

pub const BASE: u32 = 0x20000;
pub const SLOT_SIZE: u32 = 0x8000;
pub const SLOTS: [Partition; 2] = [
    Partition {
        offset: BASE,
        size: SLOT_SIZE,
    },
    Partition {
        offset: BASE + SLOT_SIZE,
        size: SLOT_SIZE,
    },
];

pub fn key_a() -> &'static HostKey {
    static KEY: OnceLock<HostKey> = OnceLock::new();
    KEY.get_or_init(HostKey::generate)
}

pub fn key_b() -> &'static HostKey {
    static KEY: OnceLock<HostKey> = OnceLock::new();
    KEY.get_or_init(HostKey::generate)
}

pub const VARIANTS: [u16; 2] = [0, 1];

pub fn target() -> Target<'static> {
    Target {
        product: Product::new("iobewi-ota-sys").unwrap(),
        mcu: Mcu::Esp32S3,
        image_format: ImageFormat::EspApp,
        hw_variants: &VARIANTS,
        min_security_version: 1,
    }
}

pub fn pins() -> &'static [[u8; 32]; 1] {
    static PINS: OnceLock<[[u8; 32]; 1]> = OnceLock::new();
    PINS.get_or_init(|| [key_a().fingerprint()])
}

pub type Verified = VerifiedBoot<'static, HostRsaVerifier, HostSha256>;

pub fn verified() -> Verified {
    VerifiedBoot::new(target(), pins(), HostRsaVerifier).unwrap()
}

/// Flash covering the two test slots, with read failures and power cuts on demand.
#[derive(Clone)]
pub struct MemFlash {
    pub data: Vec<u8>,
    pub reads: Vec<(u32, usize)>,
    /// Any read touching `[start, end)` fails.
    pub fail_reads: Option<(u32, u32)>,
    /// Commands (erase/program) still allowed before power is lost.
    pub budget: Option<usize>,
    /// Bytes the command interrupted by the power cut still manages to apply.
    pub torn: usize,
}

impl MemFlash {
    pub fn new() -> Self {
        Self {
            data: vec![0xFF; (2 * SLOT_SIZE) as usize],
            reads: Vec::new(),
            fail_reads: None,
            budget: None,
            torn: 0,
        }
    }

    fn at(&self, absolute: u32) -> usize {
        (absolute - BASE) as usize
    }

    pub fn poke(&mut self, absolute: u32, bytes: &[u8]) {
        let at = self.at(absolute);
        self.data[at..at + bytes.len()].copy_from_slice(bytes);
    }

    pub fn peek(&self, absolute: u32, len: usize) -> &[u8] {
        let at = self.at(absolute);
        &self.data[at..at + len]
    }

    pub fn flip(&mut self, absolute: u32, mask: u8) {
        let at = self.at(absolute);
        self.data[at] ^= mask;
    }

    fn spend(&mut self) -> bool {
        match &mut self.budget {
            Some(0) => false,
            Some(n) => {
                *n -= 1;
                true
            }
            None => true,
        }
    }

    pub fn proof_reads(&self, slot: Partition) -> usize {
        self.reads
            .iter()
            .filter(|(at, _)| *at >= slot.proof_offset() && *at < slot.offset + slot.size)
            .count()
    }
}

impl SlotRead for MemFlash {
    fn read(&mut self, offset: u32, out: &mut [u8]) -> Result<(), IoError> {
        self.reads.push((offset, out.len()));
        if let Some((start, end)) = self.fail_reads
            && offset < end
            && offset + out.len() as u32 > start
        {
            return Err(IoError);
        }
        out.copy_from_slice(self.peek(offset, out.len()));
        Ok(())
    }
}

impl Flash for MemFlash {
    type Error = ();
    fn read(&mut self, absolute: u32, out: &mut [u8]) -> Result<(), ()> {
        out.copy_from_slice(self.peek(absolute, out.len()));
        Ok(())
    }
    fn erase(&mut self, absolute: u32, len: u32) -> Result<(), ()> {
        let at = self.at(absolute);
        if !self.spend() {
            // A torn erase leaves the first bytes erased and the rest as it was.
            let k = self.torn.min(len as usize);
            self.data[at..at + k].fill(0xFF);
            return Err(());
        }
        self.data[at..at + len as usize].fill(0xFF);
        Ok(())
    }
    fn program(&mut self, absolute: u32, bytes: &[u8]) -> Result<(), ()> {
        let at = self.at(absolute);
        let n = if self.spend() {
            bytes.len()
        } else {
            self.torn.min(bytes.len())
        };
        // NOR flash: programming can only clear bits.
        for (cell, byte) in self.data[at..at + n].iter_mut().zip(bytes) {
            *cell &= byte;
        }
        if n == bytes.len() { Ok(()) } else { Err(()) }
    }
}

/// A deterministic "image" of `len` bytes.
pub fn image(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
        .collect()
}

pub fn manifest(key: &HostKey, image: &[u8]) -> Manifest {
    Manifest {
        algorithm: Algorithm::RsaPss3072Sha256,
        image_format: ImageFormat::EspApp,
        mcu: Mcu::Esp32S3,
        key_id: KeyId::of(&key.public_struct),
        product: Product::new("iobewi-ota-sys").unwrap(),
        hw_variant: 0,
        security_version: 1,
        size: image.len() as u32,
        digest: Digest(Sha256::digest(image).into()),
    }
}

/// Writes `image` at the start of `slot` and a proof for `manifest` signed by `signer`,
/// through the real proof writer.
pub fn install(
    flash: &mut MemFlash,
    slot: Partition,
    image: &[u8],
    manifest: &Manifest,
    signer: &HostKey,
    key: &[u8],
) {
    flash.poke(slot.offset, image);
    let envelope = signer.envelope(manifest);
    assert_eq!(envelope.len(), MANIFEST_LEN + 384);
    let mut scratch = [0u8; iap1::MAX_PAYLOAD];
    let write =
        iap1::Write::new(Algorithm::RsaPss3072Sha256, &envelope, key, &mut scratch).unwrap();
    writer::execute(flash, slot, &write).unwrap();
}

/// A slot holding `image` with a valid proof by key A.
pub fn good_slot(flash: &mut MemFlash, slot: Partition, image: &[u8]) {
    let signer = key_a();
    install(
        flash,
        slot,
        image,
        &manifest(signer, image),
        signer,
        &signer.public_struct,
    );
}
