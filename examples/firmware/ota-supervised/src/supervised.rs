//! Concrete S3 composition. OTM2 policy, execution, and write reservation share one owner.
extern crate alloc;
use alloc::string::String;
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, mutex::Mutex};
use embassy_time::{Duration, Timer};
use embedded_storage::nor_flash::ReadNorFlash;
use esp_hal::system::CpuControl;
use iobewi_esp_firmware_supervisor::{
    DomainState, ExecutionState, SlotLayout,
    metadata::NorMetadata,
    native::{EspNativeBackend, NativeRuntime},
    storage::{self, Reader},
};
use iobewi_esp_flash::{EspFlash, SharedFlash};
use iobewi_firmware_layout::domain_b::WorkloadLayout;
use iobewi_firmware_slots::{Domain, Slot};
use iobewi_firmware_update::{
    Digest,
    compat::otm2::{Record, RuntimeApi, Side, SlotMeta, State, slot_index},
    supervised::{Recovery, SupervisedUpdate},
};
use iobewi_http_server::{
    ResponseSent,
    io::Read,
    request::Request,
    response::{IntoResponse, ResponseWriter, StatusCode},
    routing::RequestHandlerService,
};
use iobewi_workload_native::{
    boot_guard::{self, BootDecision, GuardRecord, ProvenHealth},
    runtime::{Health, Identity, WorkloadRuntime},
};
const API: RuntimeApi = RuntimeApi::new(1, 0);

#[esp_hal::ram(unstable(rtc_fast, persistent))]
static mut GUARD: [u32; 4] = [0; 4];
fn guard_update<R>(f: impl FnOnce(&mut GuardRecord) -> R) -> R {
    // SAFETY: all accesses are on cpu0 and synchronous on one executor. No references
    // to retained memory escape; arbitrary boot words are decoded and checksummed.
    let pointer = core::ptr::addr_of_mut!(GUARD);
    let mut record = GuardRecord::from_words(unsafe { pointer.read_volatile() });
    let result = f(&mut record);
    unsafe { pointer.write_volatile(record.to_words()) };
    result
}
pub fn clean_reboot() {
    guard_update(boot_guard::mark_clean_reboot);
}
pub struct Component {
    flash: &'static SharedFlash,
    layout: WorkloadLayout,
    execution: DomainState,
    runtime: NativeRuntime<EspNativeBackend>,
    health: ProvenHealth,
    pending_since: Option<u64>,
}
pub type SharedComponent = Mutex<NoopRawMutex, Component>;
fn slot(side: Side) -> Slot {
    match side {
        Side::A => Slot::Zero,
        Side::B => Slot::One,
    }
}
fn identity(record: &Record, side: Side) -> Identity {
    let meta = record.meta[slot_index(side)];
    Identity {
        slot: side,
        id: String::from(meta.id()),
        version: String::from(meta.version()),
        digest: meta.digest,
        size: meta.size,
        requires: meta.requires,
    }
}
impl Component {
    pub async fn new(flash: &'static SharedFlash, cpu: CpuControl<'static>) -> Result<Self, ()> {
        let layout = storage::discover(flash).await?;
        unsafe extern "C" {
            static _native_region_start: u8;
            static _native_region_end: u8;
        }
        let region_ok = core::ptr::addr_of!(_native_region_start) as u32 == 0x3fce3700
            && core::ptr::addr_of!(_native_region_end) as u32 == 0x3fceb700;
        if !region_ok {
            return Err(());
        }
        Ok(Self {
            flash,
            layout,
            execution: DomainState::new(Domain::B),
            runtime: NativeRuntime::new(
                // SAFETY: native-region.x reserves the full fixed region and
                // asserts non-overlap. CPU_CTRL has one owner; authenticated
                // uploads are operator-trusted IWNI images gated by NativeRuntime.
                unsafe { EspNativeBackend::new(cpu, API, region_ok) },
            ),
            health: ProvenHealth::new(),
            pending_since: None,
        })
    }
    // Synchronous OTM2 store borrowed only inside the physical lock. No awaits in f.
    async fn policy<R>(
        &self,
        f: impl FnOnce(&mut SupervisedUpdate<NorMetadata<'_, EspFlash>>) -> Result<R, ()>,
    ) -> Result<R, ()> {
        let mut flash = self.flash.lock().await;
        let backend = NorMetadata::new(&mut *flash, &self.layout).map_err(|_| ())?;
        f(&mut SupervisedUpdate::new(backend))
    }
    async fn record(&self) -> Result<Record, ()> {
        self.policy(|u| u.record().map_err(|_| ())?.ok_or(())).await
    }
    fn reader(&self, side: Side, size: u32) -> Reader<'_> {
        Reader {
            flash: self.flash,
            region: storage::region(&self.layout, side),
            size,
        }
    }
    async fn launch(&self, record: &Record, side: Side) -> Result<(), ()> {
        let image = identity(record, side);
        let reader = self.reader(side, image.size);
        reader.verify(&image.digest).await.map_err(|_| ())?;
        self.runtime.start(&image, &reader).await.map_err(|_| ())
    }
    async fn stop(&mut self) {
        self.runtime.stop().await;
        self.execution.observe(ExecutionState::Stopped);
        log::info!("B: real stop {:?}", self.runtime.last_stop());
    }
    pub async fn recover(&mut self) -> Result<(), ()> {
        self.stop().await;
        let recovery = self.policy(|u| u.recover().map_err(|_| ())).await?;
        match recovery {
            Recovery::NoWorkload => Ok(()),
            Recovery::CorruptedMetadata => Err(()), // No invented state or automatic reset.
            Recovery::RollbackRequired { .. } | Recovery::PendingConfirmation(_) => {
                self.rollback().await
            }
            Recovery::Staged { active: None, .. } => Ok(()),
            Recovery::Valid(side)
            | Recovery::Staged {
                active: Some(side), ..
            } => {
                if matches!(
                    guard_update(boot_guard::on_boot),
                    BootDecision::Suppressed { .. }
                ) {
                    log::warn!("B: auto-start suppressed after repeated unclean boots");
                    return Ok(());
                }
                self.launch(&self.record().await?, side).await?;
                self.execution.observe(ExecutionState::Running(slot(side)));
                Ok(())
            }
        }
    }
    async fn rollback(&mut self) -> Result<(), ()> {
        let plan = match self.policy(|u| u.begin_rollback().map_err(|_| ())).await {
            Ok(plan) => plan,
            Err(()) => {
                self.stop().await;
                return Err(());
            }
        };
        self.stop().await;
        if let Some(side) = plan.restore {
            self.launch(&self.record().await?, side).await?;
            self.execution.observe(ExecutionState::Running(slot(side)));
        }
        self.policy(|u| u.complete_rollback().map_err(|_| ())).await
    }
    async fn activate(&mut self, id: &str) -> Result<(), ()> {
        let record = self
            .policy(|u| u.preflight_activate(API).map_err(|_| ()))
            .await?;
        let side = record.candidate.ok_or(())?;
        let image = identity(&record, side);
        if image.id != id {
            return Err(());
        }
        let reader = self.reader(side, image.size);
        reader.verify(&image.digest).await.map_err(|_| ())?;
        self.runtime
            .preflight(&image, &reader)
            .await
            .map_err(|_| ())?;
        self.policy(|u| u.begin_activation(API).map(|_| ()).map_err(|_| ()))
            .await?;
        self.stop().await;
        if self.launch(&record, side).await.is_err() {
            let _ = self.rollback().await;
            return Err(());
        }
        self.execution.observe(ExecutionState::Running(slot(side)));
        self.health = ProvenHealth::new();
        // A torn completion is recovered through the persisted Activating intention.
        if self
            .policy(|u| u.complete_activation().map_err(|_| ()))
            .await
            .is_err()
        {
            let _ = self.rollback().await;
            return Err(());
        }
        self.pending_since = Some(self.runtime.backend().now_ms());
        log::info!("B: candidate running PendingConfirmation");
        Ok(())
    }
    async fn upload<R: Read>(
        &mut self,
        request: &mut Request<'_, R>,
        meta: SlotMeta,
    ) -> Result<(), ()> {
        let prepared = self
            .policy(|u| {
                u.prepare(meta, self.layout.max_artifact_size() as u64)
                    .map_err(|_| ())
            })
            .await?;
        let running = self.runtime.running().await;
        if let Some(ref image) = running {
            self.execution
                .observe(ExecutionState::Running(slot(image.slot)))
        } else {
            self.stop().await;
        }
        let layout = SlotLayout::for_b(&self.layout, self.flash.lock().await.capacity() as u32)
            .map_err(|_| ())?;
        let flash = self.flash;
        // Supersession is durable before erase; an interrupted upload never leaves an
        // old Staged record pointing at partially overwritten bytes.
        let mut reservation = self
            .execution
            .reserve(&layout, slot(prepared.slot()))
            .map_err(|_| ())?;
        {
            let mut locked = flash.lock().await;
            let backend = NorMetadata::new(&mut *locked, &self.layout).map_err(|_| ())?;
            SupervisedUpdate::new(backend)
                .begin_overwrite()
                .map_err(|_| ())?;
        }
        let mut session = iobewi_esp_firmware_supervisor::begin_update(
            &mut reservation,
            meta.size as u64,
            Digest(meta.digest),
        )
        .map_err(|_| ())?;
        let partition = session.partition();
        let mut buffer = [0; 2048];
        let mut remaining = meta.size as usize;
        {
            let mut body = request.body_connection.body().reader();
            while remaining > 0 {
                let n = body
                    .read(&mut buffer[..remaining.min(2048)])
                    .await
                    .map_err(|_| ())?;
                if n == 0 || !session.append(flash, &buffer[..n]).await {
                    return Err(());
                }
                remaining -= n;
            }
        }
        let committed = session.finish(flash).await.map_err(|_| ())?;
        let reader = Reader {
            flash,
            region: iobewi_firmware_layout::domain_b::Region::new(
                partition.offset,
                partition.size as u32,
            ),
            size: meta.size,
        };
        reader.verify(&meta.digest).await.map_err(|_| ())?;
        let image = Identity {
            slot: prepared.slot(),
            id: String::from(meta.id()),
            version: String::from(meta.version()),
            digest: meta.digest,
            size: meta.size,
            requires: meta.requires,
        };
        self.runtime
            .preflight(&image, &reader)
            .await
            .map_err(|_| ())?;
        // Borrow other fields directly: reservation remains live through publication.
        let mut locked = flash.lock().await;
        let backend = NorMetadata::new(&mut *locked, &self.layout).map_err(|_| ())?;
        SupervisedUpdate::new(backend)
            .commit_staged(&prepared, &committed)
            .map_err(|_| ())?;
        drop(locked);
        drop(reservation);
        log::info!("B: staged {}", meta.id());
        Ok(())
    }
    async fn poll(&mut self) {
        self.runtime.sample();
        let Ok(record) = self.record().await else {
            return;
        };
        let running = self.runtime.running().await;
        let health = self.runtime.health().await;
        if record.state == State::PendingConfirmation {
            let matches = record
                .active
                .is_some_and(|side| running.as_ref() == Some(&identity(&record, side)));
            if matches && health == Health::Healthy {
                // RUNNING alone gives an initial healthy window. Keep probation
                // longer than the 3 s progress deadline so a never-progressing
                // candidate cannot be confirmed from that initial grace period.
                let now = self.runtime.backend().now_ms();
                let since = *self.pending_since.get_or_insert(now);
                if now.saturating_sub(since) >= 5_000
                    && self.policy(|u| u.confirm().map_err(|_| ())).await.is_ok()
                {
                    self.pending_since = None;
                    log::info!("B: confirmed")
                }
            } else if self.rollback().await.is_err() {
                log::error!("B: rollback incomplete; A remains available")
            }
        }
        if self
            .health
            .observe(health == Health::Healthy, self.runtime.backend().now_ms())
        {
            guard_update(boot_guard::mark_proven_healthy);
        }
        iobewi_esp_firmware_supervisor::native::drain_logs(4, |level, line| {
            if let Ok(text) = core::str::from_utf8(line) {
                log::info!("B[{level}]: {text}")
            }
        });
    }
}
use iobewi_workload_native::NativeBackend;
#[embassy_executor::task]
pub async fn monitor(component: &'static SharedComponent) {
    loop {
        Timer::after(Duration::from_millis(200)).await;
        component.lock().await.poll().await;
    }
}
pub struct Handler {
    pub component: Option<&'static SharedComponent>,
    pub action: u8,
    pub token: &'static str,
}
impl RequestHandlerService for Handler {
    async fn call_request_handler_service<R: Read, W: ResponseWriter<Error = R::Error>>(
        &self,
        _: &(),
        (): (),
        mut request: Request<'_, R>,
        writer: W,
    ) -> Result<ResponseSent, W::Error> {
        let headers = request.parts.headers();
        let header = |name| {
            headers
                .get(name)
                .and_then(|v| v.as_str().ok())
                .unwrap_or("")
        };
        let authorized = !self.token.is_empty()
            && iobewi_http_server::auth::bearer_token(Some(header("authorization"))) == self.token;
        let id = String::from(header("x-deployment-id"));
        let version = String::from(header("x-component-version"));
        let metadata = example_firmware_ota_supervised::upload_metadata(
            &id,
            &version,
            header("x-image-sha256"),
            header("x-runtime-api"),
            request.body_connection.content_length(),
        );
        let chunked = headers.get("transfer-encoding").is_some();
        let len = request.body_connection.content_length();
        let mut body = String::new();
        let (status, message) = if !authorized {
            (StatusCode::UNAUTHORIZED, "unauthorized")
        } else if let Some(shared) = self.component {
            let mut component = shared.lock().await;
            let result = match self.action {
                0 => {
                    if chunked {
                        Err(())
                    } else if let Some(meta) = metadata {
                        component.upload(&mut request, meta).await
                    } else {
                        Err(())
                    }
                }
                1 => {
                    if len == 0 && !chunked {
                        component.activate(&id).await
                    } else {
                        Err(())
                    }
                }
                2 => {
                    let record = component.policy(|u| u.record().map_err(|_| ())).await;
                    let running = component.runtime.running().await;
                    let health = component.runtime.health().await;
                    body = alloc::format!(
                        "metadata={record:?}; running={running:?}; health={health:?}"
                    );
                    Ok(())
                }
                3 => {
                    if len == 0 && !chunked {
                        component.stop().await;
                        Ok(())
                    } else {
                        Err(())
                    }
                }
                _ => Err(()),
            };
            if result.is_ok() {
                (
                    StatusCode::OK,
                    match self.action {
                        0 => "staged",
                        1 => "pending",
                        3 => "stopped",
                        _ => "status",
                    },
                )
            } else {
                (StatusCode::CONFLICT, "component refused")
            }
        } else {
            (
                StatusCode::CONFLICT,
                "component unavailable; system A operational",
            )
        };
        let connection = request.body_connection.finalize().await?;
        if body.is_empty() {
            (status, message).write_to(connection, writer).await
        } else {
            (status, body.as_str()).write_to(connection, writer).await
        }
    }
}
