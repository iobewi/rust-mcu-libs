//! The ROM SHA-256 adapter against a model of the ROM (issue #91, after the first hardware try).
//!
//! The first adapter declared the ROM's `SHA_CTX` as 80 bytes; it is 216. On the board
//! `ets_sha_init` cleared 136 bytes of its caller's stack frame and the verified second stage
//! died on a null pointer. A correct digest on a test vector does not show that kind of fault, so
//! these tests do two things: they check the digests (known vectors, every length around a block
//! boundary, a long message, unaligned input, a hash moved half way), **and** they check what the
//! ROM touches. The model below is the ROM as its disassembly shows it (ESP32-S3 rev 0 and
//! ESP32-C3 rev 0, 101, 3, see the module documentation of `rom_verify`): `ets_sha_init` clears
//! 216 bytes and sets the type, `ets_sha_update` keeps the 128-bit bit counter at 200..216 and
//! the partial block at 72, `ets_sha_finish` reads the counter to pad and copies 32 bytes of the
//! state, the hash state lives in the peripheral (not in the context) while `in_hardware` is
//! set, and nothing remembers the context's address. Every access to the context goes through a
//! helper that records the highest offset used; the adapter's object sits between two guard
//! bands that must come out untouched.
//!
//! The model is only as good as that reading of the ROM: it does not replace the hardware
//! qualification, it makes the memory contract executable.
#![cfg(feature = "rom-verify")]
#![allow(clippy::missing_safety_doc)]

#[path = "../src/rom_verify.rs"]
#[allow(dead_code)]
mod rom_verify;

use std::sync::Mutex;

use iobewi_firmware_bootauth::Sha256Stream;
use rom_verify::RomSha256;
use sha2::{Digest, Sha256};

/// Size of the ROM's `SHA_CTX`.
const CTX_SIZE: usize = 216;
const OFF_START: usize = 0;
const OFF_IN_HW: usize = 1;
const OFF_TYPE: usize = 4;
const OFF_STATE: usize = 8;
const OFF_BUFFER: usize = 72;
const OFF_BITS: usize = 200;

const IV: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

/// The ROM, as global state: the SHA peripheral and what the context accesses reached.
struct Rom {
    /// Peripheral clock enabled (`ets_sha_enable` / `ets_sha_disable`).
    enabled: bool,
    /// The peripheral's hash state.
    hw: [u32; 8],
    /// Highest context offset (exclusive) read or written since `reset`.
    reach: usize,
    /// A call that the ROM would refuse or that breaks its rules.
    violations: Vec<String>,
}

static ROM: Mutex<Rom> = Mutex::new(Rom {
    enabled: false,
    hw: IV,
    reach: 0,
    violations: Vec::new(),
});

/// One test at a time: the ROM symbols are global.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

fn rom() -> std::sync::MutexGuard<'static, Rom> {
    ROM.lock().unwrap_or_else(|e| e.into_inner())
}

/// Record that the ROM used the context up to `end`.
fn touch(end: usize) {
    let mut r = rom();
    r.reach = r.reach.max(end);
}

struct Ctx(*mut u8);

impl Ctx {
    fn rd8(&self, off: usize) -> u8 {
        touch(off + 1);
        unsafe { *self.0.add(off) }
    }
    fn wr8(&self, off: usize, v: u8) {
        touch(off + 1);
        unsafe { *self.0.add(off) = v }
    }
    fn rd32(&self, off: usize) -> u32 {
        touch(off + 4);
        unsafe { (self.0.add(off) as *const u32).read() }
    }
    fn wr32(&self, off: usize, v: u32) {
        touch(off + 4);
        unsafe { (self.0.add(off) as *mut u32).write(v) }
    }
    fn clear(&self, len: usize) {
        touch(len);
        unsafe { std::ptr::write_bytes(self.0, 0, len) }
    }
    fn aligned(&self) -> bool {
        (self.0 as usize).is_multiple_of(4)
    }
}

fn compress(state: &mut [u32; 8], block: &[u8]) {
    let mut b = sha2::digest::generic_array::GenericArray::default();
    b.copy_from_slice(block);
    sha2::compress256(state, &[b]);
}

/// `ets_sha_process`: one 64-byte block through the peripheral.
fn process(ctx: &Ctx, block: &[u8]) {
    let mut r = rom();
    if !r.enabled {
        r.violations
            .push("block processed with the peripheral off".into());
    }
    drop(r);
    // Entering: first block starts from the IV; otherwise the state must be in the peripheral,
    // or be reloaded from the context.
    if ctx.rd8(OFF_START) == 0 {
        rom().hw = IV;
        ctx.wr8(OFF_START, 1);
    } else if ctx.rd8(OFF_IN_HW) == 0 {
        let mut s = [0u32; 8];
        for (i, w) in s.iter_mut().enumerate() {
            *w = ctx.rd32(OFF_STATE + 4 * i);
        }
        rom().hw = s;
    }
    let mut s = rom().hw;
    compress(&mut s, block);
    rom().hw = s;
    ctx.wr8(OFF_IN_HW, 1);
}

#[unsafe(no_mangle)]
pub extern "C" fn ets_sha_enable() {
    rom().enabled = true;
}

#[unsafe(no_mangle)]
pub extern "C" fn ets_sha_disable() {
    rom().enabled = false;
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ets_sha_init(ctx: *mut u8, kind: u32) -> i32 {
    if ctx.is_null() || kind >= 8 {
        return 0x102;
    }
    let c = Ctx(ctx);
    if !c.aligned() {
        rom().violations.push("context not word aligned".into());
    }
    c.clear(CTX_SIZE);
    c.wr32(OFF_TYPE, kind);
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ets_sha_update(
    ctx: *mut u8,
    input: *const u8,
    len: u32,
    update_ctx: bool,
) {
    let c = Ctx(ctx);
    if !(input as usize).is_multiple_of(4) {
        rom().violations.push("input not word aligned".into());
    }
    let data = unsafe { std::slice::from_raw_parts(input, len as usize) };
    // 128-bit bit counter at 200..216 (little-endian words, low first).
    let add = (len as u64) * 8;
    let before = c.rd32(OFF_BITS);
    let sum = before.wrapping_add(add as u32);
    c.wr32(OFF_BITS, sum);
    let mut carry = (sum < before) as u32 + (add >> 32) as u32;
    for k in 1..4 {
        if carry == 0 {
            break;
        }
        let w = c.rd32(OFF_BITS + 4 * k);
        c.wr32(OFF_BITS + 4 * k, w.wrapping_add(carry));
        carry = (w.wrapping_add(carry) < w) as u32;
    }
    let mut filled = (before as usize / 8) % 64;
    let mut rest = data;
    while !rest.is_empty() {
        let take = (64 - filled).min(rest.len());
        for (i, &b) in rest[..take].iter().enumerate() {
            c.wr8(OFF_BUFFER + filled + i, b);
        }
        filled += take;
        rest = &rest[take..];
        if filled == 64 {
            let block: Vec<u8> = (0..64).map(|i| c.rd8(OFF_BUFFER + i)).collect();
            process(&c, &block);
            filled = 0;
        }
    }
    if update_ctx {
        let s = rom().hw;
        for (i, w) in s.iter().enumerate() {
            c.wr32(OFF_STATE + 4 * i, *w);
        }
        c.wr8(OFF_IN_HW, 0);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ets_sha_finish(ctx: *mut u8, out: *mut u8) -> i32 {
    let c = Ctx(ctx);
    let bits = [c.rd32(OFF_BITS), c.rd32(OFF_BITS + 4)];
    let used = (bits[0] as usize / 8) % 64;
    let pad_len = if used < 56 { 56 - used } else { 120 - used };
    let mut pad = vec![0u8; pad_len];
    pad[0] = 0x80;
    let pad = pad;
    // The ROM pads and appends the length (big-endian, 8 bytes for SHA-256), the last
    // update pulling the state back into the context.
    let aligned_pad = Box::new(AlignedBlock([0; 128]));
    let mut tmp = aligned_pad;
    tmp.0[..pad_len].copy_from_slice(&pad);
    unsafe { ets_sha_update(ctx, tmp.0.as_ptr(), pad_len as u32, false) };
    let mut len = AlignedBlock([0; 128]);
    len.0[..4].copy_from_slice(&bits[1].to_be_bytes());
    len.0[4..8].copy_from_slice(&bits[0].to_be_bytes());
    unsafe { ets_sha_update(ctx, len.0.as_ptr(), 8, true) };
    for i in 0..8 {
        let w = c.rd32(OFF_STATE + 4 * i);
        unsafe { std::ptr::copy_nonoverlapping(w.to_be_bytes().as_ptr(), out.add(4 * i), 4) };
    }
    0
}

#[repr(C, align(4))]
struct AlignedBlock([u8; 128]);

/// The RSA entry point is not exercised here; it must exist for the module to link.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ets_rsa_pss_verify(
    _: *const u8,
    _: *const u8,
    _: *const u8,
    _: *mut u8,
) -> u32 {
    0
}

fn reset_rom() {
    let mut r = rom();
    r.enabled = false;
    r.hw = IV;
    r.reach = 0;
    r.violations.clear();
}

/// The adapter between two guard bands that must stay as they were.
#[repr(C)]
struct Frame {
    before: [u8; 512],
    hash: RomSha256,
    after: [u8; 512],
}

const BAND: u8 = 0xA5;

fn frame() -> Box<Frame> {
    Box::new(Frame {
        before: [BAND; 512],
        hash: RomSha256::new(),
        after: [BAND; 512],
    })
}

fn reference(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
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

fn hex(d: [u8; 32]) -> String {
    d.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn the_model_and_the_context_have_the_same_size() {
    // The adapter's object holds the 216-byte context plus its guard; it is not smaller.
    assert!(std::mem::size_of::<RomSha256>() >= CTX_SIZE);
    assert_eq!(std::mem::align_of::<RomSha256>(), 4);
}

#[test]
fn nist_vectors() {
    let _g = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    reset_rom();
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
        assert_eq!(hex(RomSha256::digest(msg)), want, "{:?}", msg);
    }
    assert!(rom().violations.is_empty(), "{:?}", rom().violations);
}

#[test]
fn a_million_a_in_odd_sized_chunks() {
    let _g = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    reset_rom();
    let mut h = RomSha256::new();
    let mut left = 1_000_000usize;
    let mut size = 1usize;
    let buf = vec![b'a'; 4096];
    while left > 0 {
        let n = size.min(left).min(buf.len());
        h.update(&buf[..n]);
        left -= n;
        size = size * 7 % 4093 + 1;
    }
    assert_eq!(
        hex(h.finish()),
        "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
    );
    assert!(rom().violations.is_empty(), "{:?}", rom().violations);
}

#[test]
fn every_length_around_the_block_boundaries() {
    let _g = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    for len in (0..=200).chain([255, 256, 257, 511, 512, 513, 4095, 4096, 4097]) {
        reset_rom();
        let data = pseudo(len, len as u32 + 1);
        assert_eq!(RomSha256::digest(&data), reference(&data), "length {len}");
        assert!(
            rom().violations.is_empty(),
            "length {len}: {:?}",
            rom().violations
        );
    }
}

#[test]
fn a_large_message_in_firmware_sized_reads() {
    // The slot hash: a megabyte read 256 bytes at a time, as `VerifiedBoot::admit` does.
    let _g = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    reset_rom();
    let data = pseudo(1_049_840, 7);
    let mut h = RomSha256::new();
    for piece in data.chunks(256) {
        h.update(piece);
    }
    assert_eq!(h.finish(), reference(&data));
    assert!(rom().violations.is_empty(), "{:?}", rom().violations);
}

#[test]
fn unaligned_input_is_bounced_through_an_aligned_buffer() {
    let _g = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let backing = pseudo(5000, 3);
    for skew in 0..4 {
        reset_rom();
        let data = &backing[skew..skew + 3001];
        assert_eq!(RomSha256::digest(data), reference(data), "skew {skew}");
        assert!(
            rom().violations.is_empty(),
            "skew {skew}: {:?}",
            rom().violations
        );
    }
}

#[test]
fn the_rom_never_goes_past_its_216_bytes_and_the_neighbours_are_untouched() {
    let _g = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    for len in [0usize, 1, 55, 56, 63, 64, 65, 1000, 70_000] {
        reset_rom();
        let data = pseudo(len, 11);
        let mut f = frame();
        for piece in data.chunks(97) {
            f.hash.update(piece);
        }
        let want = reference(&data);
        // `finish` consumes the hash: keep the bands by moving it out and back.
        let Frame {
            before,
            hash,
            after,
        } = *f;
        let out = hash.finish();
        assert_eq!(out, want, "length {len}");
        assert!(
            before.iter().all(|&b| b == BAND),
            "length {len}: band before"
        );
        assert!(after.iter().all(|&b| b == BAND), "length {len}: band after");
        let reach = rom().reach;
        assert!(
            reach <= CTX_SIZE,
            "length {len}: the ROM reached offset {reach}, past its 216 bytes"
        );
        assert!(
            reach >= OFF_BITS + 16,
            "length {len}: the model saw the bit counter (reach {reach})"
        );
    }
}

#[test]
fn a_hash_moved_half_way_is_still_right() {
    // The context may move between calls (nothing in the ROM keeps its address): a whole copy.
    let _g = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    reset_rom();
    let data = pseudo(5000, 5);
    let mut a = RomSha256::new();
    a.update(&data[..1234]);
    let mut b = Box::new(a); // moved to the heap, then moved again below
    b.update(&data[1234..3000]);
    let mut c = *b;
    c.update(&data[3000..]);
    assert_eq!(c.finish(), reference(&data));
    assert!(rom().violations.is_empty(), "{:?}", rom().violations);
}

#[test]
fn the_peripheral_is_released_by_finish_and_by_an_abandoned_hash() {
    let _g = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    reset_rom();
    let _ = RomSha256::digest(b"abc");
    assert!(!rom().enabled, "finish leaves the peripheral gated");
    let mut h = RomSha256::new();
    h.update(b"abc");
    assert!(rom().enabled);
    drop(h);
    assert!(
        !rom().enabled,
        "an abandoned hash must not leave the peripheral on"
    );
    // A hash that was never used does not touch the peripheral at all.
    reset_rom();
    drop(RomSha256::new());
    assert!(!rom().enabled);
}

/// The first adapter, as it was: an 80-byte context handed to the ROM. The model is the proof
/// that these tests would have caught it.
#[test]
fn the_80_byte_context_of_the_first_adapter_is_caught() {
    let _g = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    reset_rom();
    #[repr(C)]
    struct Old {
        state: [u32; 16],
        total_bits: [u32; 4],
    }
    #[repr(C)]
    struct Stack {
        ctx: Old,
        neighbour: [u8; 512],
    }
    let mut s = Box::new(Stack {
        ctx: Old {
            state: [0; 16],
            total_bits: [0; 4],
        },
        neighbour: [BAND; 512],
    });
    ets_sha_enable();
    unsafe { ets_sha_init(&mut s.ctx as *mut Old as *mut u8, 2) };
    assert!(
        s.neighbour.iter().take(136).all(|&b| b == 0),
        "ets_sha_init cleared 136 bytes past the 80 declared"
    );
    assert!(s.neighbour[136..].iter().all(|&b| b == BAND));
    assert!(rom().reach > std::mem::size_of::<Old>());
    ets_sha_disable();
}
