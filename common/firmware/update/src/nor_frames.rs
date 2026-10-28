//! [`FrameBackend`] over any `embedded-storage` NOR flash: the two copies are
//! the first [`MAX_FRAME`] bytes of two **consecutive erase units** of a
//! dedicated region, so erasing or tearing one never touches the other.
//!
//! The caller borrows the sole physical owner for the duration of one store
//! operation (a lock held by the platform adapter); no flash, mutex or
//! partition table is constructed here.

use crate::frame_store::{FrameBackend, MAX_FRAME};
use embedded_storage::nor_flash::{NorFlash, ReadNorFlash};

#[derive(Debug, PartialEq, Eq)]
pub enum Error<E> {
    /// The region or the flash geometry cannot host two copies.
    Geometry,
    Copy,
    /// An erase left a non-blank copy, or a program did not read back.
    Readback,
    Flash(E),
}

pub struct NorFrames<'a, F> {
    flash: &'a mut F,
    offset: u32,
}

impl<'a, F: NorFlash> NorFrames<'a, F> {
    /// `offset`/`size` describe the dedicated region (partition).
    pub fn new(flash: &'a mut F, offset: u32, size: u32) -> Result<Self, Error<F::Error>> {
        let end = u64::from(offset) + u64::from(size);
        if F::ERASE_SIZE == 0
            || F::WRITE_SIZE == 0
            || F::READ_SIZE == 0
            || MAX_FRAME > F::ERASE_SIZE
            || !MAX_FRAME.is_multiple_of(F::WRITE_SIZE)
            || !MAX_FRAME.is_multiple_of(F::READ_SIZE)
            || !(offset as usize).is_multiple_of(F::ERASE_SIZE)
            || (size as usize) < 2 * F::ERASE_SIZE
            || end > flash.capacity() as u64
        {
            return Err(Error::Geometry);
        }
        Ok(Self { flash, offset })
    }

    fn address(&self, copy: usize) -> Result<u32, Error<F::Error>> {
        if copy >= 2 {
            return Err(Error::Copy);
        }
        self.offset
            .checked_add((copy * F::ERASE_SIZE) as u32)
            .ok_or(Error::Geometry)
    }

    fn read_copy(&mut self, copy: usize) -> Result<[u8; MAX_FRAME], Error<F::Error>> {
        let address = self.address(copy)?;
        let mut bytes = [0u8; MAX_FRAME];
        self.flash.read(address, &mut bytes).map_err(Error::Flash)?;
        Ok(bytes)
    }

    fn erase_copy(&mut self, copy: usize) -> Result<u32, Error<F::Error>> {
        let address = self.address(copy)?;
        self.flash
            .erase(address, address + F::ERASE_SIZE as u32)
            .map_err(Error::Flash)?;
        if self.read_copy(copy)? != [0xFF; MAX_FRAME] {
            return Err(Error::Readback);
        }
        Ok(address)
    }
}

impl<F: NorFlash> FrameBackend for NorFrames<'_, F> {
    type Error = Error<F::Error>;

    fn read(&mut self, copy: usize, out: &mut [u8; MAX_FRAME]) -> Result<(), Self::Error> {
        *out = self.read_copy(copy)?;
        Ok(())
    }

    fn write(&mut self, copy: usize, frame: &[u8]) -> Result<(), Self::Error> {
        // Pad with the erased value up to the programming granularity.
        let mut padded = [0xFFu8; MAX_FRAME];
        padded[..frame.len()].copy_from_slice(frame);
        let len = frame.len().next_multiple_of(F::WRITE_SIZE);
        if len > MAX_FRAME {
            return Err(Error::Geometry);
        }
        let address = self.erase_copy(copy)?;
        self.flash
            .write(address, &padded[..len])
            .map_err(Error::Flash)?;
        if self.read_copy(copy)? != padded {
            return Err(Error::Readback);
        }
        Ok(())
    }

    fn erase(&mut self, copy: usize) -> Result<(), Self::Error> {
        self.erase_copy(copy).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use crate::Digest;
    use crate::frame_store::{FrameStore, Loaded, MAX_FRAME, MAX_PAYLOAD, StoreError};
    use crate::metadata::{
        Metadata, MetadataStore, Stage, Staged, finish_validation, load_metadata, save_metadata,
    };
    use crate::service::{self, BootActivation};
    use alloc::string::String;
    use alloc::vec;
    use alloc::vec::Vec;
    use core::cell::RefCell;
    use embedded_storage::nor_flash::{ErrorType, NorFlashError, NorFlashErrorKind};
    use std::future::Future;
    use std::task::{Context, Poll, Waker};

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct Cut;
    impl NorFlashError for Cut {
        fn kind(&self) -> NorFlashErrorKind {
            NorFlashErrorKind::Other
        }
    }

    const SECTOR: usize = 4096;
    /// Region under test sits between two guard sectors that must never change.
    const REGION: u32 = SECTOR as u32;

    #[derive(Clone)]
    struct Flash {
        bytes: Vec<u8>,
        /// Remaining byte operations before power is lost (None = never).
        budget: Option<usize>,
        /// An interrupted erase leaves the interrupted byte as garbage.
        garbage: bool,
        ops: usize,
    }
    impl Flash {
        fn new() -> Self {
            Self {
                bytes: vec![0xFF; 4 * SECTOR],
                budget: None,
                garbage: false,
                ops: 0,
            }
        }
        fn tick(&mut self) -> bool {
            self.ops += 1;
            match self.budget.as_mut() {
                Some(0) => false,
                Some(n) => {
                    *n -= 1;
                    true
                }
                None => true,
            }
        }
    }
    impl ErrorType for Flash {
        type Error = Cut;
    }
    impl ReadNorFlash for Flash {
        const READ_SIZE: usize = 1;
        fn capacity(&self) -> usize {
            self.bytes.len()
        }
        fn read(&mut self, at: u32, out: &mut [u8]) -> Result<(), Cut> {
            out.copy_from_slice(&self.bytes[at as usize..at as usize + out.len()]);
            Ok(())
        }
    }
    impl NorFlash for Flash {
        const WRITE_SIZE: usize = 4;
        const ERASE_SIZE: usize = SECTOR;
        fn erase(&mut self, from: u32, to: u32) -> Result<(), Cut> {
            for i in from as usize..to as usize {
                if !self.tick() {
                    if self.garbage {
                        self.bytes[i] ^= 0x5A;
                    }
                    return Err(Cut);
                }
                self.bytes[i] = 0xFF;
            }
            Ok(())
        }
        fn write(&mut self, at: u32, bytes: &[u8]) -> Result<(), Cut> {
            for (i, b) in bytes.iter().enumerate() {
                if !self.tick() {
                    return Err(Cut);
                }
                self.bytes[at as usize + i] &= b;
            }
            Ok(())
        }
    }

    fn store(flash: &mut Flash) -> FrameStore<NorFrames<'_, Flash>> {
        FrameStore::new(NorFrames::new(flash, REGION, 2 * SECTOR as u32).unwrap())
    }
    fn loaded_bytes(flash: &mut Flash) -> Option<Vec<u8>> {
        match store(flash).load().unwrap() {
            Loaded::Blank => None,
            Loaded::Payload { bytes, .. } => Some(bytes),
            Loaded::Corrupted => panic!("corrupted"),
        }
    }
    fn assert_guards_untouched(flash: &Flash, reference: &Flash) {
        assert_eq!(flash.bytes[..SECTOR], reference.bytes[..SECTOR]);
        assert_eq!(flash.bytes[3 * SECTOR..], reference.bytes[3 * SECTOR..]);
    }

    #[test]
    fn geometry_is_checked_before_any_access() {
        let mut flash = Flash::new();
        for (offset, size) in [(1, 8192), (4096, 4096), (4096, 1 << 20), (4097, 8192)] {
            assert!(matches!(
                NorFrames::new(&mut flash, offset, size),
                Err(Error::Geometry)
            ));
        }
    }

    #[test]
    fn an_erased_region_is_blank_and_commits_stay_inside_the_two_sectors() {
        let mut flash = Flash::new();
        let reference = flash.clone();
        assert_eq!(loaded_bytes(&mut flash), None);
        for n in 0..7u8 {
            store(&mut flash).commit(&[n; 200]).unwrap();
            assert_eq!(loaded_bytes(&mut flash), Some(vec![n; 200]));
        }
        assert_guards_untouched(&flash, &reference);
    }

    /// Every erase byte and every programmed byte is a possible power-loss
    /// instant, in both erase-failure models. After each one the region reads
    /// as the previous record or the new one, never anything else, and the
    /// guard sectors are untouched.
    #[test]
    fn a_power_cut_at_any_byte_of_a_commit_yields_the_old_or_the_new_record() {
        for garbage in [false, true] {
            for prior in 1..4u8 {
                let mut base = Flash::new();
                for n in 0..prior {
                    store(&mut base).commit(&[n; 100]).unwrap();
                }
                let old = loaded_bytes(&mut base);
                let new = vec![0xA5; 300];
                let mut probe = base.clone();
                probe.ops = 0;
                store(&mut probe).commit(&new).unwrap();
                let total = probe.ops;
                let reference = base.clone();
                for cut in 0..total {
                    let mut flash = base.clone();
                    flash.ops = 0;
                    flash.budget = Some(cut);
                    flash.garbage = garbage;
                    assert!(store(&mut flash).commit(&new).is_err(), "cut {cut}");
                    flash.budget = None;
                    let seen = loaded_bytes(&mut flash);
                    assert!(
                        seen == old || seen.as_deref() == Some(&new[..]),
                        "garbage={garbage} prior={prior} cut={cut}: unexpected {seen:?}"
                    );
                    assert_guards_untouched(&flash, &reference);
                    // The interrupted store still accepts the next commit.
                    store(&mut flash).commit(b"after").unwrap();
                    assert_eq!(loaded_bytes(&mut flash), Some(b"after".to_vec()));
                }
            }
        }
    }

    #[test]
    fn a_first_commit_cut_leaves_blank_or_corrupted_and_is_not_reformatted() {
        let mut base = Flash::new();
        let mut probe = base.clone();
        store(&mut probe).commit(b"first").unwrap();
        let total = probe.ops;
        for cut in 0..total {
            let mut flash = base.clone();
            flash.ops = 0;
            flash.budget = Some(cut);
            assert!(store(&mut flash).commit(b"first").is_err());
            flash.budget = None;
            match store(&mut flash).load().unwrap() {
                Loaded::Blank => store(&mut flash).commit(b"first").unwrap(),
                Loaded::Payload { bytes, .. } => assert_eq!(bytes, b"first"),
                Loaded::Corrupted => {
                    let snapshot = flash.bytes.clone();
                    assert_eq!(
                        store(&mut flash).commit(b"first"),
                        Err(StoreError::Corrupted)
                    );
                    assert_eq!(flash.bytes, snapshot);
                    store(&mut flash).erase_all().unwrap();
                    assert_eq!(store(&mut flash).load().unwrap(), Loaded::Blank);
                }
            }
        }
        base.bytes.fill(0xFF);
    }

    #[test]
    fn a_payload_over_the_limit_is_refused_without_touching_the_flash() {
        let mut flash = Flash::new();
        let reference = flash.clone();
        assert_eq!(
            store(&mut flash).commit(&vec![0; MAX_PAYLOAD + 1]),
            Err(StoreError::TooLarge)
        );
        assert_eq!(flash.bytes, reference.bytes);
    }

    // --- the engine on top of the store, power lost at every flash byte ---

    struct FlashStore<'a>(&'a RefCell<Flash>);
    impl MetadataStore for FlashStore<'_> {
        type Error = ();
        async fn load_raw(&self) -> Result<Option<Vec<u8>>, ()> {
            let mut flash = self.0.borrow_mut();
            match store(&mut flash).load().map_err(|_| ())? {
                Loaded::Blank => Ok(None),
                Loaded::Payload { bytes, .. } => Ok(Some(bytes)),
                Loaded::Corrupted => Err(()),
            }
        }
        async fn commit_raw(&self, bytes: &[u8]) -> Result<(), ()> {
            let mut flash = self.0.borrow_mut();
            store(&mut flash).commit(bytes).map_err(|_| ())
        }
    }

    struct Boot;
    impl BootActivation for Boot {
        fn valid_target(&self, target: &str) -> bool {
            target == "ota_1"
        }
        async fn activate(&self, _target: &str) -> Result<(), ()> {
            Ok(())
        }
    }

    fn ready<F: Future>(future: F) -> F::Output {
        let mut future = std::pin::pin!(future);
        match future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("in-memory flash is immediately ready"),
        }
    }

    /// publish -> activate -> confirm, then a second deployment.
    fn lifecycle(cell: &RefCell<Flash>) {
        let flash_store = FlashStore(cell);
        for (id, byte) in [("deploy-1", 1u8), ("deploy-2", 2)] {
            let artifact = crate::Committed {
                size: 4096,
                digest: Digest([byte; 32]),
            };
            if ready(service::publish(
                &flash_store,
                String::from(id),
                artifact,
                String::from("ota_1"),
            ))
            .is_err()
            {
                return;
            }
            if ready(service::activate_staged(&flash_store, &Boot, id)).is_err() {
                return;
            }
            let staged = match ready(load_metadata(&flash_store)) {
                Ok(metadata) => metadata.staged,
                Err(_) => return,
            };
            if ready(finish_validation(&flash_store, &staged)).is_err() {
                return;
            }
        }
    }

    #[test]
    fn the_ota_lifecycle_survives_a_power_cut_at_every_flash_byte() {
        let mut clean = Flash::new();
        let cell = RefCell::new(clean.clone());
        lifecycle(&cell);
        clean = cell.into_inner();
        let total = clean.ops;
        // Cost of the very first commit: until it completes there is no
        // previous record to fall back on, and a torn region is refused.
        let first_commit = SECTOR + MAX_FRAME;
        assert!(
            total > 4 * SECTOR,
            "the lifecycle really commits many times"
        );
        let mut final_state = None;
        {
            let mut f = clean.clone();
            final_state = final_state.or(loaded_bytes(&mut f));
        }
        let final_state = Metadata::decode(&final_state.unwrap()).unwrap();
        assert_eq!(final_state.active_deployment_id, "deploy-2");
        assert!(final_state.staged.stage == Stage::None);

        for cut in 0..total {
            let mut flash = Flash::new();
            flash.budget = Some(cut);
            let cell = RefCell::new(flash);
            lifecycle(&cell);
            // Power is back: the store must answer, with a coherent record.
            let mut flash = cell.into_inner();
            flash.budget = None;
            let cell = RefCell::new(flash);
            let flash_store = FlashStore(&cell);
            let metadata = match ready(load_metadata(&flash_store)) {
                Ok(metadata) => metadata,
                Err(error) => {
                    assert!(
                        cut < first_commit,
                        "cut {cut}: store unreadable after the first commit: {error:?}"
                    );
                    continue;
                }
            };
            match metadata.staged.stage {
                Stage::None => {}
                Stage::Written | Stage::Activating => {
                    assert_eq!(metadata.staged.slot, "ota_1", "cut {cut}");
                    assert!(
                        metadata.staged.deployment_id.starts_with("deploy-"),
                        "cut {cut}"
                    );
                }
            }
            // The engine can keep working on the recovered store.
            let staged = Staged::default();
            ready(save_metadata(&flash_store, &{
                let mut m = metadata.clone();
                m.set_staged(staged);
                m
            }))
            .unwrap_or_else(|e| panic!("cut {cut}: next commit failed: {e:?}"));
        }
    }

    // --- factory provisioning: an initial OTSF frame holding the empty OTM1 ---

    fn initial_payload() -> Vec<u8> {
        Metadata::default().encode().unwrap()
    }

    fn provisioned_flash() -> Flash {
        let mut flash = Flash::new();
        let image = crate::frame_store::provisioned_image(SECTOR, &initial_payload()).unwrap();
        flash.bytes[REGION as usize..REGION as usize + image.len()].copy_from_slice(&image);
        flash
    }

    #[test]
    fn the_provisioned_image_is_what_provision_programs() {
        let mut programmed = Flash::new();
        store(&mut programmed)
            .provision(&initial_payload())
            .unwrap();
        assert_eq!(programmed.bytes, provisioned_flash().bytes);
        // Copy 1 and the guard sectors stay erased.
        let flash = provisioned_flash();
        assert!(
            flash.bytes[REGION as usize + SECTOR..]
                .iter()
                .all(|b| *b == 0xFF)
        );
        assert!(flash.bytes[..SECTOR].iter().all(|b| *b == 0xFF));
    }

    #[test]
    fn the_empty_record_means_no_ota_transaction_exactly_like_a_blank_region() {
        let mut flash = provisioned_flash();
        match store(&mut flash).load().unwrap() {
            Loaded::Payload {
                sequence,
                copy,
                degraded,
                ..
            } => assert_eq!((sequence, copy, degraded), (1, 0, true)),
            other => panic!("{other:?}"),
        }
        let provisioned = ready(load_metadata(&FlashStore(&RefCell::new(flash)))).unwrap();
        let blank = ready(load_metadata(&FlashStore(&RefCell::new(Flash::new())))).unwrap();
        assert_eq!(provisioned.encode().unwrap(), blank.encode().unwrap());
        assert!(provisioned.staged.stage == Stage::None);
        assert!(provisioned.staged.slot.is_empty());
        assert!(provisioned.staged.digest.is_empty());
        assert!(provisioned.staged.deployment_id.is_empty());
        assert_eq!(provisioned.staged.size, 0);
        // Neither a confirmed image nor a candidate.
        assert!(provisioned.active_digest.is_empty());
        assert!(provisioned.active_deployment_id.is_empty());
    }

    #[test]
    fn provision_never_overwrites_a_record_or_a_corrupted_region() {
        let mut flash = provisioned_flash();
        assert_eq!(
            store(&mut flash).provision(&initial_payload()),
            Err(StoreError::NotBlank)
        );
        flash.bytes[REGION as usize + 20] ^= 1;
        let before = flash.bytes.clone();
        assert_eq!(
            store(&mut flash).provision(&initial_payload()),
            Err(StoreError::Corrupted)
        );
        assert_eq!(store(&mut flash).load().unwrap(), Loaded::Corrupted);
        assert_eq!(store(&mut flash).commit(b"x"), Err(StoreError::Corrupted));
        assert_eq!(flash.bytes, before, "a degraded region is never rewritten");
    }

    #[test]
    fn a_corrupted_byte_of_the_only_provisioned_copy_stays_fail_closed() {
        let len = {
            let mut frame = [0u8; MAX_FRAME];
            let image = crate::frame_store::provisioned_image(SECTOR, &initial_payload()).unwrap();
            frame.copy_from_slice(&image[..MAX_FRAME]);
            image
                .iter()
                .take(MAX_FRAME)
                .rposition(|b| *b != 0xFF)
                .unwrap()
                + 1
        };
        for byte in 0..len {
            for pattern in [0x01u8, 0x80, 0xFF] {
                let mut flash = provisioned_flash();
                flash.bytes[REGION as usize + byte] ^= pattern;
                assert_eq!(
                    store(&mut flash).load().unwrap(),
                    Loaded::Corrupted,
                    "byte {byte} ^ {pattern:#x}"
                );
            }
        }
    }

    #[test]
    fn a_power_cut_at_any_byte_of_the_first_commit_after_provisioning_keeps_a_record() {
        let initial = initial_payload();
        // Exact flash cost of the first operational commit (publish).
        let first_commit = {
            let cell = RefCell::new(provisioned_flash());
            ready(service::publish(
                &FlashStore(&cell),
                String::from("deploy-1"),
                crate::Committed {
                    size: 4096,
                    digest: Digest([1; 32]),
                },
                String::from("ota_1"),
            ))
            .unwrap();
            let ops = cell.into_inner().ops;
            assert!(ops > SECTOR && ops < SECTOR + MAX_FRAME);
            ops
        };
        for garbage in [false, true] {
            let mut clean = provisioned_flash();
            clean.garbage = garbage;
            let cell = RefCell::new(clean);
            lifecycle(&cell);
            let total = cell.into_inner().ops;
            assert!(total > first_commit);

            for cut in 0..total {
                let mut flash = provisioned_flash();
                flash.garbage = garbage;
                flash.budget = Some(cut);
                let cell = RefCell::new(flash);
                lifecycle(&cell);
                let mut flash = cell.into_inner();
                flash.budget = None;
                let copy0_before =
                    provisioned_flash().bytes[REGION as usize..REGION as usize + SECTOR].to_vec();
                if cut < first_commit {
                    // Only copy 1 was being written: copy 0 is untouched and
                    // the answer is the initial record, never an error.
                    assert_eq!(
                        flash.bytes[REGION as usize..REGION as usize + SECTOR],
                        copy0_before[..],
                        "cut {cut}"
                    );
                }
                let cell = RefCell::new(flash);
                let flash_store = FlashStore(&cell);
                let metadata = ready(load_metadata(&flash_store))
                    .unwrap_or_else(|e| panic!("cut {cut} garbage {garbage}: {e:?}"));
                if cut < first_commit {
                    // The initial record, or the new one once its frame is whole.
                    let is_initial = metadata.encode().unwrap() == initial;
                    let is_new = metadata.staged.stage == Stage::Written
                        && metadata.staged.deployment_id == "deploy-1";
                    assert!(is_initial || is_new, "cut {cut}");
                    if cut <= SECTOR {
                        assert!(is_initial, "cut {cut}: still erasing copy 1");
                    }
                } else {
                    match metadata.staged.stage {
                        Stage::None => {}
                        Stage::Written | Stage::Activating => {
                            assert_eq!(metadata.staged.slot, "ota_1", "cut {cut}")
                        }
                    }
                }
                let mut next = metadata.clone();
                next.set_staged(Staged::default());
                ready(save_metadata(&flash_store, &next))
                    .unwrap_or_else(|e| panic!("cut {cut}: next commit failed: {e:?}"));
            }
        }
    }

    #[test]
    fn a_provisioning_image_needs_a_sector_that_holds_the_frame() {
        assert!(crate::frame_store::provisioned_image(16, &initial_payload()).is_none());
        assert!(crate::frame_store::provisioned_image(SECTOR, &[0u8; MAX_PAYLOAD + 1]).is_none());
    }
}
