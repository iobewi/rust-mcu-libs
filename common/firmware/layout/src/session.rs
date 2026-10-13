//! Exclusive reservation shared by system and supervised firmware domains.
//! The application owns exactly one state per physical domain and must route
//! every start/stop/activation through it. This is a borrow boundary, not a
//! runtime detector, persistence engine, or global service manager.
use crate::authorization::{AuthorizationError, InactiveWriteTarget, SlotLayout};
use iobewi_firmware_slots::{Domain, Slot};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionState {
    /// A failed observation must never be treated as a stopped component.
    Unknown,
    /// The supervisor verified that no component is executing.
    Stopped,
    Running(Slot),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionError {
    UnknownExecution,
    WrongDomain,
    Authorization(AuthorizationError),
}

/// One owner per physical domain. Activation cannot mutate its observation
/// while a reservation exists:
///
/// ```compile_fail
/// use iobewi_firmware_layout::{Region, authorization::SlotLayout, session::{DomainState, ExecutionState}};
/// use iobewi_firmware_slots::{Domain, Slot};
/// let layout = SlotLayout::new(Domain::B,
///     [Region { offset: 0x20000, size: 0x40000 }, Region { offset: 0x60000, size: 0x40000 }],
///     Region { offset: 0x9000, size: 0x2000 }, 0x1000000, 0x1000).unwrap();
/// let mut state = DomainState::new(Domain::B);
/// state.observe(ExecutionState::Running(Slot::Zero));
/// let mut guard = state.reserve(&layout, Slot::One).unwrap();
/// state.observe(ExecutionState::Running(Slot::One));
/// let _ = guard.permit();
/// ```
pub struct DomainState {
    domain: Domain,
    execution: ExecutionState,
}

impl DomainState {
    pub const fn new(domain: Domain) -> Self {
        Self {
            domain,
            execution: ExecutionState::Unknown,
        }
    }

    /// Supply the real platform/supervisor observation. The mutable borrow
    /// makes this unavailable while a write guard owns the state.
    pub fn observe(&mut self, execution: ExecutionState) {
        self.execution = execution;
    }

    pub const fn execution(&self) -> ExecutionState {
        self.execution
    }

    /// Reserve a target until verification/publication completes. A stopped
    /// supervised domain permits first installation; unknown execution does not.
    /// A system adapter must separately require its actual running-image observation.
    pub fn reserve<'a>(
        &'a mut self,
        layout: &SlotLayout,
        requested: Slot,
    ) -> Result<WriteGuard<'a>, SessionError> {
        if layout.domain() != self.domain {
            return Err(SessionError::WrongDomain);
        }
        let active = match self.execution {
            ExecutionState::Unknown => return Err(SessionError::UnknownExecution),
            ExecutionState::Stopped => None,
            ExecutionState::Running(slot) => Some(slot),
        };
        let permit = layout
            .authorize(active, requested)
            .map_err(SessionError::Authorization)?;
        Ok(WriteGuard {
            _owner: self,
            permit,
        })
    }
}

/// Holds the domain's mutable borrow across the transaction. No activation or
/// second reservation through this owner is possible until the guard is dropped.
pub struct WriteGuard<'a> {
    _owner: &'a mut DomainState,
    permit: InactiveWriteTarget,
}

impl WriteGuard<'_> {
    /// An exclusive writer borrow also prevents creating a second writer from
    /// this guard. Keep the guard until publication finishes.
    /// Two concurrent writers cannot borrow the same permit:
    ///
    /// ```compile_fail
    /// use iobewi_firmware_layout::{Region, authorization::SlotLayout, session::{DomainState, ExecutionState}};
    /// use iobewi_firmware_slots::{Domain, Slot};
    /// let layout = SlotLayout::new(Domain::B,
    ///     [Region { offset: 0x20000, size: 0x40000 }, Region { offset: 0x60000, size: 0x40000 }],
    ///     Region { offset: 0x9000, size: 0x2000 }, 0x1000000, 0x1000).unwrap();
    /// let mut state = DomainState::new(Domain::B);
    /// state.observe(ExecutionState::Stopped);
    /// let mut guard = state.reserve(&layout, Slot::Zero).unwrap();
    /// let first = guard.permit();
    /// let second = guard.permit();
    /// assert_eq!(first.region(), second.region());
    /// ```
    pub fn permit(&mut self) -> &mut InactiveWriteTarget {
        &mut self.permit
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Region;
    fn layout(domain: Domain) -> SlotLayout {
        SlotLayout::new(
            domain,
            [
                Region {
                    offset: 0x20000,
                    size: 0x40000,
                },
                Region {
                    offset: 0x60000,
                    size: 0x40000,
                },
            ],
            Region {
                offset: 0x9000,
                size: 0x2000,
            },
            0x1000000,
            0x1000,
        )
        .unwrap()
    }
    #[test]
    fn unknown_is_not_stopped_and_verified_stopped_allows_first_install() {
        for domain in [Domain::A, Domain::B, Domain::new(2)] {
            let mut state = DomainState::new(domain);
            assert!(matches!(
                state.reserve(&layout(domain), Slot::Zero),
                Err(SessionError::UnknownExecution)
            ));
            state.observe(ExecutionState::Stopped);
            assert!(state.reserve(&layout(domain), Slot::Zero).is_ok());
        }
    }
    #[test]
    fn all_domains_share_inactive_selection_and_release_on_abort() {
        for domain in [Domain::A, Domain::B, Domain::new(2)] {
            let mut state = DomainState::new(domain);
            state.observe(ExecutionState::Running(Slot::Zero));
            assert!(matches!(
                state.reserve(&layout(domain), Slot::Zero),
                Err(SessionError::Authorization(
                    AuthorizationError::ActiveTarget
                ))
            ));
            {
                let mut guard = state.reserve(&layout(domain), Slot::One).unwrap();
                assert_eq!(guard.permit().slot().domain, domain);
                assert_eq!(guard.permit().region().offset, 0x60000);
            }
            assert_eq!(state.execution(), ExecutionState::Running(Slot::Zero));
            state.observe(ExecutionState::Running(Slot::One));
            assert!(state.reserve(&layout(domain), Slot::Zero).is_ok());
        }
    }
    #[test]
    fn rejects_another_domains_layout() {
        let mut state = DomainState::new(Domain::B);
        state.observe(ExecutionState::Stopped);
        assert!(matches!(
            state.reserve(&layout(Domain::new(2)), Slot::Zero),
            Err(SessionError::WrongDomain)
        ));
    }
}
