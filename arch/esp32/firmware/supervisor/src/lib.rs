#![no_std]

//! Domain B: supervisor-owned workload state and inactive write authorization.
//! This is NOT a bootloader and never uses otadata.
//! The supervisor must own this value exclusively for the full write transaction.
use iobewi_firmware_layout::authorization::{
    AuthorizationError, InactiveWriteTarget, authorize_b,
};
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
            permit: Some(permit),
        })
    }
}

pub struct WriteGuard<'a> {
    owner: &'a mut SupervisorState,
    permit: Option<InactiveWriteTarget>,
}

impl WriteGuard<'_> {
    /// Consume the permit once. Keep this guard alive while writing.
    pub fn take_permit(&mut self) -> Option<InactiveWriteTarget> {
        self.permit.take()
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
            Err(SupervisorError::Authorization(AuthorizationError::ActiveTarget))
        ));
        let mut guard = s.authorize_inactive(&layout(), Slot::One).unwrap();
        let permit = guard.take_permit().unwrap();
        assert_eq!(permit.region().offset, 0x60000);
        assert!(guard.take_permit().is_none());
        drop(guard);
        s.set_running(Some(Slot::One)).unwrap();
    }
}
