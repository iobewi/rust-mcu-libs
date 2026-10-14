//! `NativeRuntime`: runs a *native* Workload (a binary compiled for the target) under the
//! Workload Supervisor, behind the [`WorkloadRuntime`] trait.
//!
//! What it owns: image validation, loading, start, stop, health and the identity of what is
//! really running. What it does **not** own: OTM2 and slot policy (the Supervisor and the
//! Workload OTA engine), HTTP, Agent OTA/OTM1, and anything platform-specific (the
//! [`NativeBackend`] provides that: where the code goes, how a core starts and is halted,
//! the clock).
//!
//! ```text
//! start : read header -> gate (target, ABI, RuntimeApi, bounds, entry)  -- nothing touched yet
//!         -> stop previous -> clear region -> copy code/data from the slot
//!         -> control block = STARTING -> backend.launch(entry) -> wait RUNNING
//! stop  : stop_requested = 1 -> wait for the Workload to return (cooperative)
//!         -> if it ignores the request: backend.halt() (forced, reported as Forced)
//! health: Healthy = RUNNING and the progress counter advanced within the window
//! ```
//!
//! No Rust type crosses to the Workload: the entry point and the control block are the
//! `iobewi-workload-abi` contract.
#![no_std]

extern crate alloc;

use core::cell::{Cell, RefCell};
use core::sync::atomic::Ordering;

use crate::runtime::{ArtifactReader, Health, Identity, RuntimeError, WorkloadRuntime};
use iobewi_firmware_update::compat::otm2::RuntimeApi;
use iobewi_workload_abi::{ABI_VERSION, ControlBlockV1, state};
use iobewi_workload_image::{HEADER_LEN, ImageError, ImageHeader, TargetLayout};

pub mod boot_guard;
pub mod runtime;
#[cfg(any(test, feature = "test-util"))]
pub mod testing;
#[cfg(test)]
extern crate std;
#[cfg(test)]
mod tests;

/// Why a backend could not start the Workload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaunchError(pub &'static str);

/// The platform side of native execution. All methods take `&self` (single executor,
/// interior mutability inside) and none blocks the async executor except for the bounded,
/// documented memory copies.
#[allow(async_fn_in_trait)]
pub trait NativeBackend {
    /// The fixed-link layout this backend executes (its target, regions, ABI, RuntimeApi).
    fn layout(&self) -> TargetLayout;
    /// Agent-owned shared control block (lives for the whole program).
    fn control(&self) -> &ControlBlockV1;
    /// Zero the whole Workload region (code, data, bss).
    fn clear_region(&self);
    /// Copy `bytes` into the code area at `offset` (relative to the code base).
    fn write_code(&self, offset: u32, bytes: &[u8]);
    /// Copy `bytes` into the data area at `offset` (relative to the data base).
    fn write_data(&self, offset: u32, bytes: &[u8]);
    /// Make the written code visible to instruction fetch and start executing at `entry`
    /// (a link address inside the code area) with the context for `control()`.
    fn launch(&self, entry: u32) -> Result<(), LaunchError>;
    /// Guarantee that no Workload instruction executes any more. Idempotent.
    fn halt(&self);
    /// Monotonic milliseconds.
    fn now_ms(&self) -> u64;
    async fn delay_ms(&self, ms: u32);
}

#[derive(Debug, Clone, Copy)]
pub struct NativeConfig {
    /// Time allowed between `launch` and the Workload announcing RUNNING.
    pub start_timeout_ms: u32,
    /// Time a Workload gets to return after `stop_requested` before it is halted.
    pub stop_grace_ms: u32,
    /// A RUNNING Workload whose progress counter did not move for this long is Unhealthy.
    pub health_window_ms: u32,
    /// A Workload that stays Unhealthy this much longer is quarantined: its core is halted
    /// (it is dead or wedged; it only burns power) while the Agent keeps running.
    pub quarantine_after_ms: u32,
}

impl Default for NativeConfig {
    fn default() -> Self {
        Self {
            start_timeout_ms: 1_000,
            stop_grace_ms: 1_500,
            health_window_ms: 3_000,
            quarantine_after_ms: 10_000,
        }
    }
}

/// How the last `stop` ended (reported, not an error: the Workload is stopped either way).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopOutcome {
    /// Nothing was running.
    NothingRunning,
    /// The Workload returned from its entry point by itself.
    Cooperative,
    /// It ignored the stop request (`StopTimeout`) or had failed; execution was halted.
    Forced,
    /// It had already been halted (quarantined or exited): nothing left to stop.
    AlreadyHalted,
}

pub struct NativeRuntime<B: NativeBackend> {
    backend: B,
    config: NativeConfig,
    current: RefCell<Option<Identity>>,
    /// (last progress value seen, time it last changed)
    progress: Cell<(u32, u64)>,
    last_stop: Cell<StopOutcome>,
    forced_stops: Cell<u32>,
    /// The Workload was halted by the runtime because it died, wedged or exited.
    halted: Cell<bool>,
    quarantines: Cell<u32>,
}

impl<B: NativeBackend> NativeRuntime<B> {
    pub fn new(backend: B) -> Self {
        Self::with_config(backend, NativeConfig::default())
    }

    pub fn with_config(backend: B, config: NativeConfig) -> Self {
        Self {
            backend,
            config,
            current: RefCell::new(None),
            progress: Cell::new((0, 0)),
            last_stop: Cell::new(StopOutcome::NothingRunning),
            forced_stops: Cell::new(0),
            halted: Cell::new(false),
            quarantines: Cell::new(0),
        }
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn last_stop(&self) -> StopOutcome {
        self.last_stop.get()
    }

    /// How many Workloads were quarantined (halted because dead/wedged/exited) since boot.
    pub fn quarantines(&self) -> u32 {
        self.quarantines.get()
    }

    /// How many stops had to be forced (`StopTimeout`) since boot, for the Agent to report.
    pub fn forced_stops(&self) -> u32 {
        self.forced_stops.get()
    }

    /// Observe the progress counter now. Call it periodically (a monitor task every few
    /// hundred ms) so the health window is accurate; `health()` also samples.
    pub fn sample(&self) {
        let control = self.backend.control();
        let now = self.backend.now_ms();
        let value = control.progress.load(Ordering::Acquire);
        let (seen, _) = self.progress.get();
        if value != seen {
            self.progress.set((value, now));
        }
        if self.current.borrow().is_none() || self.halted.get() {
            return;
        }
        match control.state.load(Ordering::Acquire) {
            // A Workload that reported a fault, or whose entry point returned, only burns
            // its core: halt it. The Agent is unaffected.
            state::FAILED | state::STOPPED => self.quarantine(),
            // Wedged or faulted in hardware (the core stops, the state stays RUNNING): after
            // the health window plus the quarantine delay without progress, halt it too.
            state::RUNNING => {
                let (_, changed) = self.progress.get();
                let stalled = now.saturating_sub(changed);
                let limit = u64::from(self.config.health_window_ms)
                    + u64::from(self.config.quarantine_after_ms);
                if stalled > limit {
                    self.quarantine();
                }
            }
            _ => {}
        }
    }

    fn quarantine(&self) {
        self.backend.halt();
        self.halted.set(true);
        self.quarantines.set(self.quarantines.get() + 1);
    }

    async fn read_header<R: ArtifactReader>(
        &self,
        artifact: &Identity,
        reader: &R,
    ) -> Result<ImageHeader, RuntimeError> {
        // Never read a header past the end of what OTA stored.
        if u64::from(artifact.size) < HEADER_LEN as u64 {
            return Err(reject(ImageError::Truncated));
        }
        let mut head = [0u8; HEADER_LEN];
        reader.read(0, &mut head).await?;
        ImageHeader::decode(&head).map_err(reject)
    }

    /// Structural + compatibility gate. No side effect.
    fn gate(&self, header: &ImageHeader, artifact: &Identity) -> Result<(), RuntimeError> {
        if u64::from(artifact.size) < HEADER_LEN as u64 {
            return Err(reject(ImageError::Truncated));
        }
        header
            .validate(&self.backend.layout(), u64::from(artifact.size))
            .map_err(reject)?;
        // What OTM2 recorded at upload must agree with what the image says it needs.
        if header.requires != artifact.requires {
            return Err(RuntimeError {
                reason: "api_declared_mismatch",
            });
        }
        Ok(())
    }

    async fn stop_inner(&self) {
        let control = self.backend.control();
        let had = self.current.borrow_mut().take();
        let st = control.state.load(Ordering::Acquire);
        let alive = (st == state::STARTING || st == state::RUNNING) && !self.halted.get();
        if had.is_some() && !alive {
            // Quarantined / exited / failed earlier: the core is already halted.
            self.backend.halt();
            control.state.store(0, Ordering::Release);
            self.halted.set(false);
            self.last_stop.set(StopOutcome::AlreadyHalted);
            return;
        }
        if had.is_none() && !alive {
            // Nothing to stop (a repeated stop is a no-op); `last_stop` keeps describing the
            // last stop that really ended an execution.
            self.backend.halt();
            return;
        }
        let mut outcome = StopOutcome::Cooperative;
        if alive {
            control.stop_requested.store(1, Ordering::Release);
            let mut waited = 0u32;
            loop {
                let s = control.state.load(Ordering::Acquire);
                if s == state::STOPPED || s == state::FAILED {
                    if s == state::FAILED {
                        outcome = StopOutcome::Forced;
                    }
                    break;
                }
                if waited >= self.config.stop_grace_ms {
                    outcome = StopOutcome::Forced;
                    break;
                }
                self.backend.delay_ms(10).await;
                waited += 10;
            }
        } else if st == state::FAILED {
            outcome = StopOutcome::Forced;
        }
        self.backend.halt();
        control.state.store(0, Ordering::Release);
        if outcome == StopOutcome::Forced {
            self.forced_stops.set(self.forced_stops.get() + 1);
        }
        self.last_stop.set(outcome);
    }

    async fn copy<R: ArtifactReader>(
        reader: &R,
        mut file_offset: u32,
        size: u32,
        mut write: impl FnMut(u32, &[u8]),
    ) -> Result<(), RuntimeError> {
        let mut chunk = [0u8; 256];
        let mut done = 0u32;
        while done < size {
            let n = core::cmp::min(chunk.len() as u32, size - done) as usize;
            reader.read(u64::from(file_offset), &mut chunk[..n]).await?;
            write(done, &chunk[..n]);
            file_offset += n as u32;
            done += n as u32;
        }
        Ok(())
    }
}

fn reject(e: ImageError) -> RuntimeError {
    RuntimeError { reason: e.reason() }
}

impl<B: NativeBackend> WorkloadRuntime for NativeRuntime<B> {
    async fn preflight<R: ArtifactReader>(
        &self,
        artifact: &Identity,
        reader: &R,
    ) -> Result<(), RuntimeError> {
        let header = self.read_header(artifact, reader).await?;
        self.gate(&header, artifact)
    }

    async fn start<R: ArtifactReader>(
        &self,
        artifact: &Identity,
        reader: &R,
    ) -> Result<(), RuntimeError> {
        // 1. Gate first: a bad image never reaches executable memory.
        let header = self.read_header(artifact, reader).await?;
        self.gate(&header, artifact)?;

        // 2. One Workload at a time; the old one is really gone before the region is reused.
        self.stop_inner().await;

        // 3. Load.
        self.backend.clear_region();
        Self::copy(reader, header.code_offset, header.code_size, |off, b| {
            self.backend.write_code(off, b)
        })
        .await?;
        if header.data_size > 0 {
            Self::copy(reader, header.data_offset, header.data_size, |off, b| {
                self.backend.write_data(off, b)
            })
            .await?;
        }

        // 4. Fresh control block, then launch.
        let control = self.backend.control();
        control.size.store(
            core::mem::size_of::<ControlBlockV1>() as u32,
            Ordering::Relaxed,
        );
        control
            .abi_version
            .store(u32::from(ABI_VERSION), Ordering::Relaxed);
        control.stop_requested.store(0, Ordering::Relaxed);
        control.progress.store(0, Ordering::Relaxed);
        control.exit_code.store(0, Ordering::Relaxed);
        control.fault.store(0, Ordering::Relaxed);
        control.state.store(state::STARTING, Ordering::Release);
        self.halted.set(false);
        self.backend
            .launch(header.entry)
            .map_err(|e| RuntimeError { reason: e.0 })?;

        // 5. Running means the Workload itself said so.
        let mut waited = 0u32;
        loop {
            let s = control.state.load(Ordering::Acquire);
            if s == state::RUNNING {
                break;
            }
            if s == state::FAILED || s == state::STOPPED || waited >= self.config.start_timeout_ms {
                self.backend.halt();
                control.state.store(0, Ordering::Release);
                return Err(RuntimeError {
                    reason: "did_not_start",
                });
            }
            self.backend.delay_ms(5).await;
            waited += 5;
        }
        self.progress.set((
            control.progress.load(Ordering::Acquire),
            self.backend.now_ms(),
        ));
        *self.current.borrow_mut() = Some(artifact.clone());
        Ok(())
    }

    async fn stop(&self) {
        self.stop_inner().await;
    }

    async fn health(&self) -> Health {
        if self.current.borrow().is_none() {
            return Health::Unknown;
        }
        self.sample();
        if self.halted.get() {
            return Health::Unhealthy;
        }
        match self.backend.control().state.load(Ordering::Acquire) {
            state::RUNNING => {
                let (_, changed) = self.progress.get();
                if self.backend.now_ms().saturating_sub(changed)
                    <= u64::from(self.config.health_window_ms)
                {
                    Health::Healthy
                } else {
                    Health::Unhealthy
                }
            }
            state::FAILED | state::STOPPED => Health::Unhealthy,
            _ => Health::Unknown,
        }
    }

    async fn running(&self) -> Option<Identity> {
        let st = self.backend.control().state.load(Ordering::Acquire);
        if (st == state::STARTING || st == state::RUNNING) && !self.halted.get() {
            self.current.borrow().clone()
        } else {
            None
        }
    }
}

/// The `RuntimeApi` a layout provides (convenience for glue code).
pub fn provided(layout: &TargetLayout) -> RuntimeApi {
    layout.provides
}
