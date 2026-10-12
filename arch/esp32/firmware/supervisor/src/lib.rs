#![no_std]

//! Domain B: supervisor-owned workload state and inactive write authorization.
//! This is NOT a bootloader and never uses otadata.
//! The supervisor must own this value exclusively for the full write transaction.
use iobewi_firmware_layout::authorization::{AuthorizationError, InactiveWriteTarget, authorize_b};
use iobewi_firmware_layout::domain_b::WorkloadLayout;
use iobewi_firmware_slots::{Domain, FirmwareSlot, Slot};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupervisorError {
    NotRunning,
    Busy,
    InvalidTarget,
    Authorization(AuthorizationError),
}

/// Exclusive owner of the currently running workload identity.
/// Construct only after the supervisor has actually activated that workload.
pub struct SupervisorState {
    active: Option<Slot>,
    writing: bool,
}

impl SupervisorState {
    pub const fn stopped() -> Self {
        Self {
            active: None,
            writing: false,
        }
    }

    /// The supervisor calls this when activation finishes, never from OTM2.
    /// Switching while a writer owns the state is denied.
    pub fn set_running(&mut self, active: Option<Slot>) -> Result<(), SupervisorError> {
        if self.writing {
            return Err(SupervisorError::Busy);
        }
        self.active = active;
        Ok(())
    }

    /// Hold the returned guard across the *entire* update transaction.
    /// Dropping it releases the supervisor's logical write exclusion.
    pub fn authorize_inactive<'a>(
        &'a mut self,
        layout: &WorkloadLayout,
        requested: Slot,
    ) -> Result<WriteGuard<'a>, SupervisorError> {
        if self.writing {
            return Err(SupervisorError::Busy);
        }
        let active = self.active.ok_or(SupervisorError::NotRunning)?;
        let permit = authorize_b(
            layout,
            Some(FirmwareSlot::new(Domain::B, active)),
            FirmwareSlot::new(Domain::B, requested),
        )
        .map_err(SupervisorError::Authorization)?;
        self.writing = true;
        Ok(WriteGuard {
            owner: self,
            permit,
        })
    }
}

pub struct WriteGuard<'a> {
    owner: &'a mut SupervisorState,
    permit: InactiveWriteTarget,
}

impl WriteGuard<'_> {
    /// Borrow the permit; a writer constructed from it cannot outlive this guard.
    pub fn permit(&self) -> &InactiveWriteTarget {
        &self.permit
    }
}

#[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
impl WriteGuard<'_> {
    /// Create the ESP writer while retaining the supervisor's exclusive
    /// authorization. Rust prevents dropping this guard before the writer.
    /// The caller holds both until verification completes; activation then
    /// remains a separate supervisor decision.
    pub fn writer(&self) -> iobewi_esp_firmware_storage::shared_flash::ArtifactWriter<'_> {
        iobewi_esp_firmware_storage::shared_flash::ArtifactWriter::new(self.permit())
    }
}

/// An ESP OTA write session whose writer cannot outlive the supervisor guard.
/// Dropping an incomplete session never changes the active workload.
#[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
pub struct WorkloadWriteSession<'a> {
    writer: iobewi_esp_firmware_storage::shared_flash::ArtifactWriter<'a>,
    engine: iobewi_firmware_update::WriteSession,
}

#[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
impl WriteGuard<'_> {
    /// Begin a streaming update of this guard's authorized inactive slot.
    pub fn begin_update(
        &self,
        total: u64,
        digest: iobewi_firmware_update::Digest,
    ) -> WorkloadWriteSession<'_> {
        WorkloadWriteSession {
            writer: self.writer(),
            engine: iobewi_firmware_update::WriteSession::begin(total, digest),
        }
    }
}

#[cfg(any(feature = "esp32c3", feature = "esp32s3"))]
impl WorkloadWriteSession<'_> {
    pub async fn append(&mut self, flash: &iobewi_esp_flash::SharedFlash, data: &[u8]) -> bool {
        self.writer.append(flash, &mut self.engine, data).await
    }

    /// Only successful digest verification yields Committed. Activation is
    /// deliberately a distinct supervisor decision, not a side effect here.
    pub async fn finish(
        self,
        flash: &iobewi_esp_flash::SharedFlash,
    ) -> Result<
        iobewi_firmware_update::Committed,
        iobewi_firmware_update::Error<iobewi_esp_firmware_storage::FlashWriteError>,
    > {
        self.writer.finish(flash, self.engine).await
    }
}

impl Drop for WriteGuard<'_> {
    fn drop(&mut self) {
        self.owner.writing = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iobewi_firmware_layout::domain_b::{Region, assemble};

    fn layout() -> WorkloadLayout {
        assemble(
            Some(Region::new(0x9000, 0x2000)),
            Some(Region::new(0x20000, 0x40000)),
            Some(Region::new(0x60000, 0x40000)),
            0x1000,
        )
        .unwrap()
    }

    #[test]
    fn requires_active_workload_and_inactive_target() {
        let mut s = SupervisorState::stopped();
        assert!(matches!(
            s.authorize_inactive(&layout(), Slot::One),
            Err(SupervisorError::NotRunning)
        ));
        s.set_running(Some(Slot::Zero)).unwrap();
        assert!(matches!(
            s.authorize_inactive(&layout(), Slot::Zero),
            Err(SupervisorError::Authorization(
                AuthorizationError::ActiveTarget
            ))
        ));
        let guard = s.authorize_inactive(&layout(), Slot::One).unwrap();
        assert_eq!(guard.permit().region().offset, 0x60000);
        drop(guard);
        s.set_running(Some(Slot::One)).unwrap();
    }
}
