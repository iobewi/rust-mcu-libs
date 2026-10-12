//! Streaming upload lifecycle. A platform supplies only physical writes;
//! this manager owns session identity, watermarks and durable publication.

use alloc::string::String;
use core::fmt::Debug;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_time::Instant;

use crate::{Committed, Digest, Error, WriteSession};
use crate::metadata::{MetadataError, MetadataStore, SessionParams, format_digest};
use super::publish;

#[derive(Debug, Clone, Copy, Default)]
pub struct UploadStats {
    pub sectors_flushed: u32,
    pub erase_batches: u32,
    pub erase_batch_kib: usize,
}

#[allow(async_fn_in_trait)]
pub trait UploadWriter {
    type Error: Debug;
    fn slot(&self) -> &'static str;
    fn stats(&self) -> UploadStats { UploadStats::default() }
    async fn append(&mut self, session: &mut WriteSession, data: &[u8]) -> bool;
    async fn finish(&mut self, session: WriteSession) -> Result<Committed, Error<Self::Error>>;
}

struct Upload<W> {
    writer: W,
    engine: WriteSession,
    params: SessionParams,
    started_at: Instant,
}

pub struct UploadManager<W> {
    current: Mutex<CriticalSectionRawMutex, Option<Upload<W>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartError { TooLarge, Busy, Conflict, Storage(MetadataError) }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinishError {
    NotWriting,
    DigestMismatch(Digest),
    Incomplete { durable: u64 },
    Backend,
    Storage(MetadataError),
}

pub struct UploadResult {
    pub written: u32,
    pub digest: String,
    pub slot: &'static str,
    pub elapsed_ms: u64,
    pub stats: UploadStats,
}

impl<W: UploadWriter> UploadManager<W> {
    pub const fn new() -> Self { Self { current: Mutex::new(None) } }

    pub async fn in_progress(&self) -> bool { self.current.lock().await.is_some() }

    pub async fn received(&self) -> u32 {
        self.current.lock().await.as_ref().map_or(0, |s| s.engine.received() as u32)
    }

    pub async fn written(&self) -> u32 {
        self.current.lock().await.as_ref().map_or(0, |s| s.engine.durable() as u32)
    }

    pub async fn params_match(&self, params: &SessionParams) -> bool {
        self.current.lock().await.as_ref().is_some_and(|s| s.params.matches(params))
    }

    /// A new writer is created only after the old staged transaction is
    /// resolved. The caller must select a target without holding flash across
    /// the metadata commit performed here.
    pub async fn begin<S: MetadataStore>(
        &self,
        store: &S,
        params: SessionParams,
        capacity: u64,
        writer: W,
    ) -> Result<(), StartError> {
        if u64::from(params.total) > capacity { return Err(StartError::TooLarge); }
        let expected = super::begin(store, &params).await.map_err(|error| match error {
            super::BeginError::BadDigest => StartError::Busy,
            super::BeginError::Conflict => StartError::Conflict,
            super::BeginError::Storage(error) => StartError::Storage(error),
        })?;
        *self.current.lock().await = Some(Upload {
            writer,
            engine: WriteSession::begin(u64::from(params.total), expected),
            params,
            started_at: Instant::now(),
        });
        Ok(())
    }

    pub async fn chunk(&self, data: &[u8]) -> bool {
        let mut guard = self.current.lock().await;
        let Some(upload) = guard.as_mut() else { return false };
        upload.writer.append(&mut upload.engine, data).await
    }

    pub async fn finish<S: MetadataStore>(&self, store: &S) -> Result<UploadResult, FinishError> {
        let Some(Upload { mut writer, engine, params, started_at }) = self.current.lock().await.take() else {
            return Err(FinishError::NotWriting);
        };
        let slot = writer.slot();
        let committed = writer.finish(engine).await.map_err(|error| match error {
            Error::DigestMismatch(digest) => FinishError::DigestMismatch(digest),
            Error::Incomplete { durable } => FinishError::Incomplete { durable },
            _ => FinishError::Backend,
        })?;
        publish(store, params.deployment_id, committed, String::from(slot))
            .await.map_err(FinishError::Storage)?;
        Ok(UploadResult {
            written: committed.size as u32,
            digest: format_digest(&committed.digest),
            slot,
            elapsed_ms: started_at.elapsed().as_millis(),
            stats: writer.stats(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::{MetadataStore, PrepareRefusal, load_metadata};
    use crate::service::{ActivateError, BootActivation, TargetSelection, activate_staged, prepare};
    use alloc::vec::Vec;
    use core::cell::RefCell;
    use core::future::Future;
    use core::task::{Context, Poll, Waker};
    use sha2::{Digest as _, Sha256};

    fn block_on<F: Future>(future: F) -> F::Output {
        let mut future = core::pin::pin!(future);
        let mut cx = Context::from_waker(Waker::noop());
        loop {
            if let Poll::Ready(v) = future.as_mut().poll(&mut cx) {
                return v;
            }
            std::thread::yield_now();
        }
    }

    struct MemoryStore(RefCell<Option<Vec<u8>>>);

    impl MetadataStore for MemoryStore {
        type Error = ();
        async fn load_raw(&self) -> Result<Option<Vec<u8>>, ()> { Ok(self.0.borrow().clone()) }
        async fn commit_raw(&self, bytes: &[u8]) -> Result<(), ()> {
            *self.0.borrow_mut() = Some(bytes.to_vec());
            Ok(())
        }
    }

    /// Byte store standing in for a flash slot: durable immediately.
    #[derive(Default)]
    struct Slot(Vec<u8>);

    impl crate::ArtifactStorage for Slot {
        type Error = ();
        fn write(&mut self, _offset: u64, pending: &[u8]) -> Result<u64, ()> {
            self.0.extend_from_slice(pending);
            Ok(self.0.len() as u64)
        }
        fn finish(&mut self, _offset: u64, pending: &[u8]) -> Result<u64, ()> {
            self.0.extend_from_slice(pending);
            Ok(self.0.len() as u64)
        }
    }

    struct Writer {
        slot: &'static str,
        storage: Slot,
    }

    impl Writer {
        fn new(slot: &'static str) -> Self { Self { slot, storage: Slot::default() } }
    }

    impl UploadWriter for Writer {
        type Error = ();
        fn slot(&self) -> &'static str { self.slot }
        async fn append(&mut self, session: &mut WriteSession, data: &[u8]) -> bool {
            session.append(&mut self.storage, data).is_ok()
        }
        async fn finish(&mut self, session: WriteSession) -> Result<Committed, Error<()>> {
            session.finish(&mut self.storage)
        }
    }

    fn image(len: usize, seed: u8) -> Vec<u8> {
        (0..len).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect()
    }

    fn params(id: &str, data: &[u8]) -> SessionParams {
        let digest: [u8; 32] = Sha256::digest(data).into();
        SessionParams { deployment_id: String::from(id), digest: format_digest(&Digest(digest)), total: data.len() as u32 }
    }

    struct Platform;

    impl TargetSelection for Platform {
        async fn write_target(&self) -> Result<(&'static str, u64), ()> { Ok(("ota_1", 1024)) }
    }

    struct Boot;

    impl BootActivation for Boot {
        fn valid_target(&self, target: &str) -> bool { target == "ota_0" || target == "ota_1" }
        async fn activate(&self, _target: &str) -> Result<(), ()> { Ok(()) }
    }

    fn upload_all(manager: &UploadManager<Writer>, store: &MemoryStore, id: &str, data: &[u8]) -> UploadResult {
        block_on(manager.begin(store, params(id, data), 1024, Writer::new("ota_1"))).unwrap();
        for chunk in data.chunks(100) {
            assert!(block_on(manager.chunk(chunk)));
        }
        block_on(manager.finish(store)).unwrap()
    }

    #[test]
    fn prepare_write_activate_publishes_in_order_and_activation_is_by_deployment_id() {
        let store = MemoryStore(RefCell::new(None));
        let manager = UploadManager::<Writer>::new();
        let data = image(700, 1);

        let target = block_on(prepare(&store, &Platform, "esp32s3", "embewi-ab-v1", "esp32s3", "embewi-ab-v1", data.len() as u64)).unwrap();
        assert_eq!(target, "ota_1");

        let result = upload_all(&manager, &store, "dep-a", &data);
        assert_eq!((result.written, result.slot), (700, "ota_1"));
        assert_eq!(result.digest, format_digest(&Digest(Sha256::digest(&data).into())));
        let staged = block_on(load_metadata(&store)).unwrap().staged;
        assert_eq!((staged.stage.as_str(), staged.slot.as_str(), staged.deployment_id.as_str()), ("written", "ota_1", "dep-a"));

        // Activation names the deployment that was written, nothing else.
        assert_eq!(block_on(activate_staged(&store, &Boot, "other")), Err(ActivateError::DeploymentMismatch));
        assert_eq!(block_on(activate_staged(&store, &Boot, "dep-a")).unwrap(), "ota_1");
        assert_eq!(block_on(load_metadata(&store)).unwrap().staged.stage.as_str(), "activating");
    }

    #[test]
    fn a_new_upload_supersedes_a_staged_one_but_never_an_activating_one() {
        let store = MemoryStore(RefCell::new(None));
        let manager = UploadManager::<Writer>::new();
        upload_all(&manager, &store, "dep-a", &image(300, 1));

        // Staged A -> begin B: A is superseded.
        let b = image(400, 2);
        upload_all(&manager, &store, "dep-b", &b);
        let staged = block_on(load_metadata(&store)).unwrap().staged;
        assert_eq!((staged.deployment_id.as_str(), staged.size), ("dep-b", 400));

        // Activating B: a further begin is a conflict, and prepare is refused as busy.
        block_on(activate_staged(&store, &Boot, "dep-b")).unwrap();
        let c = image(100, 3);
        assert_eq!(block_on(manager.begin(&store, params("dep-c", &c), 1024, Writer::new("ota_1"))), Err(StartError::Conflict));
        assert!(!block_on(manager.in_progress()));
        assert_eq!(
            block_on(prepare(&store, &Platform, "esp32s3", "embewi-ab-v1", "esp32s3", "embewi-ab-v1", 100)),
            Err(PrepareRefusal::Busy)
        );
        assert_eq!(block_on(load_metadata(&store)).unwrap().staged.deployment_id, "dep-b", "the activating record is intact");
    }

    #[test]
    fn prepare_refuses_foreign_chip_layout_and_oversized_images_before_touching_state() {
        let store = MemoryStore(RefCell::new(None));
        let prep = |chip, layout, size| block_on(prepare(&store, &Platform, chip, layout, "esp32s3", "embewi-ab-v1", size));
        assert_eq!(prep("esp32", "embewi-ab-v1", 10), Err(PrepareRefusal::ChipMismatch));
        assert_eq!(prep("esp32s3", "other", 10), Err(PrepareRefusal::LayoutMismatch));
        assert_eq!(prep("esp32s3", "embewi-ab-v1", 1025), Err(PrepareRefusal::SizeTooLarge));
        assert_eq!(prep("esp32s3", "embewi-ab-v1", 1024), Ok("ota_1"));
        assert!(store.0.borrow().is_none(), "prepare never writes");
    }

    #[test]
    fn write_sequencing_oversize_bad_digest_and_digest_mismatch() {
        let store = MemoryStore(RefCell::new(None));
        let manager = UploadManager::<Writer>::new();
        let data = image(500, 4);

        // No session: chunks and finish are refused.
        assert!(!block_on(manager.chunk(&[1, 2, 3])));
        assert!(matches!(block_on(manager.finish(&store)), Err(FinishError::NotWriting)));

        // Larger than the slot capacity / malformed digest.
        assert_eq!(block_on(manager.begin(&store, params("d", &data), 100, Writer::new("ota_1"))), Err(StartError::TooLarge));
        let mut bad = params("d", &data);
        bad.digest = String::from("sha256:zz");
        assert_eq!(block_on(manager.begin(&store, bad, 1024, Writer::new("ota_1"))), Err(StartError::Busy));

        // Declared digest of different bytes: finish reports the mismatch and publishes nothing.
        let declared = params("d", &image(500, 9));
        block_on(manager.begin(&store, declared, 1024, Writer::new("ota_1"))).unwrap();
        assert!(block_on(manager.params_match(&params("d", &image(500, 9)))));
        for chunk in data.chunks(128) {
            assert!(block_on(manager.chunk(chunk)));
        }
        assert!(matches!(block_on(manager.finish(&store)), Err(FinishError::DigestMismatch(_))));
        assert!(block_on(load_metadata(&store)).unwrap().staged.stage.as_str() == "none");

        // Short of the declared size: incomplete, nothing published.
        block_on(manager.begin(&store, params("e", &data), 1024, Writer::new("ota_1"))).unwrap();
        assert!(block_on(manager.chunk(&data[..300])));
        assert!(matches!(block_on(manager.finish(&store)), Err(FinishError::Incomplete { durable: 300 })));
        assert!(block_on(load_metadata(&store)).unwrap().staged.stage.as_str() == "none");
    }

    // ---- deadlock regression -------------------------------------------------
    //
    // The historical deadlock: `prepare` held the shared flash lock while it read
    // `otadata`, then asked ConfigSpace (which locks the *same* physical flash)
    // for the staged transaction -> a non-reentrant lock taken twice. The rule
    // that prevents it is in the contract: the platform releases its lock before
    // `write_target` returns, and the service only then reads metadata. Here the
    // platform and the metadata store share one lock; the store *try-locks* so a
    // recursive acquisition fails the test instead of hanging it.

    use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
    use embassy_sync::mutex::Mutex as AsyncMutex;

    struct SharedLock(AsyncMutex<CriticalSectionRawMutex, ()>);

    struct LockedTarget<'a>(&'a SharedLock);

    impl TargetSelection for LockedTarget<'_> {
        async fn write_target(&self) -> Result<(&'static str, u64), ()> {
            let guard = self.0 .0.lock().await; // "read otadata under the flash lock"
            drop(guard); // released before returning, as the contract requires
            Ok(("ota_1", 1024))
        }
    }

    struct LockedStore<'a> {
        lock: &'a SharedLock,
        inner: MemoryStore,
    }

    impl MetadataStore for LockedStore<'_> {
        type Error = ();
        async fn load_raw(&self) -> Result<Option<Vec<u8>>, ()> {
            let _g = self
                .lock
                .0
                .try_lock()
                .expect("flash lock still held by the platform: a nested acquisition would deadlock");
            self.inner.load_raw().await
        }
        async fn commit_raw(&self, bytes: &[u8]) -> Result<(), ()> {
            let _g = self.lock.0.try_lock().expect("flash lock still held: nested acquisition");
            self.inner.commit_raw(bytes).await
        }
    }

    #[test]
    fn prepare_never_holds_the_shared_flash_lock_while_it_reads_metadata() {
        let lock = SharedLock(AsyncMutex::new(()));
        let store = LockedStore { lock: &lock, inner: MemoryStore(RefCell::new(None)) };
        let target = block_on(prepare(&store, &LockedTarget(&lock), "esp32s3", "embewi-ab-v1", "esp32s3", "embewi-ab-v1", 100)).unwrap();
        assert_eq!(target, "ota_1");
    }

    #[test]
    fn upload_begin_and_finish_take_the_shared_lock_only_one_operation_at_a_time() {
        let lock = SharedLock(AsyncMutex::new(()));
        let store = LockedStore { lock: &lock, inner: MemoryStore(RefCell::new(None)) };
        let manager = UploadManager::<Writer>::new();
        let data = image(300, 5);
        block_on(manager.begin(&store, params("dep", &data), 1024, Writer::new("ota_1"))).unwrap();
        for chunk in data.chunks(100) {
            assert!(block_on(manager.chunk(chunk)));
        }
        block_on(manager.finish(&store)).unwrap();
        // The platform's lock is free again between operations.
        assert!(lock.0.try_lock().is_ok());
    }
}
