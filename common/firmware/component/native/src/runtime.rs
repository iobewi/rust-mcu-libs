//! Native runtime execution contracts reused from the historical supervisor.
use alloc::string::String;
use iobewi_firmware_update::compat::otm2::{RuntimeApi, Side};
/// Which artifact is (or should be) running: everything the runtime and the
/// confirmation need to recognise it. No slot is ever exposed outside the device;
/// `slot` is local.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub slot: Side,
    pub id: String,
    pub version: String,
    pub digest: [u8; 32],
    pub size: u32,
    pub requires: RuntimeApi,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    Healthy,
    Unhealthy,
    /// Nothing to judge (no Workload running, or the runtime cannot tell).
    Unknown,
}

impl Health {
    pub const fn as_str(self) -> &'static str {
        match self {
            Health::Healthy => "healthy",
            Health::Unhealthy => "unhealthy",
            Health::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeError {
    pub reason: &'static str,
}

/// Read access to the running artifact's bytes, without loading it in RAM and
/// without holding the flash: each read takes the lock for that read only.
#[allow(async_fn_in_trait)]
pub trait ArtifactReader {
    async fn read(&self, offset: u64, buf: &mut [u8]) -> Result<(), RuntimeError>;
}

/// The execution backend. One Workload at a time; `start` only returns `Ok` once
/// the Workload is running, `stop` only returns once it has really stopped (no
/// zombie task), and both are idempotent.
#[allow(async_fn_in_trait)]
pub trait WorkloadRuntime {
    /// Structural and compatibility gate, with no load, stop, or execution effect.
    async fn preflight<R: ArtifactReader>(
        &self,
        artifact: &Identity,
        reader: &R,
    ) -> Result<(), RuntimeError>;
    async fn start<R: ArtifactReader>(
        &self,
        artifact: &Identity,
        reader: &R,
    ) -> Result<(), RuntimeError>;
    async fn stop(&self);
    async fn health(&self) -> Health;
    /// What is running right now, if anything.
    async fn running(&self) -> Option<Identity>;
}
