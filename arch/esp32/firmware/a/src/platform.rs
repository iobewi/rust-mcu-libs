//! System-domain composition over the historical boot observation and protocol.
//! Own one SystemBoot per A domain and route activation through it. The returned
//! write guard prevents activation while upload/verification/publication runs.
use crate::otadata_esp::LockedFlash;
use crate::{Layout, Partition, otadata, running, selection};
use embedded_storage::nor_flash::{NorFlash, ReadNorFlash};
use iobewi_esp_flash::{EspFlash, SharedFlash};
use iobewi_firmware_boot::{self as boot, Raw, SECTOR_COUNT};
use iobewi_firmware_layout::{
    Requirement,
    session::{DomainState, WriteGuard},
};
use iobewi_firmware_slots::{Domain, Slot};

#[derive(Debug)]
pub enum Error {
    Discovery,
    Observation,
    Selection(selection::SelectionError),
    Reservation(running::ReservationError),
    Metadata,
    NoTransition,
}

pub struct SystemBoot {
    layout: Layout,
    state: DomainState,
    flash_size: u32,
}

impl SystemBoot {
    /// Discover all physical regions and reject overlapping/out-of-flash tables.
    pub async fn discover(flash: &SharedFlash) -> Result<Self, Error> {
        let mut guard = flash.lock().await;
        let flash_size = u32::try_from(guard.capacity()).map_err(|_| Error::Discovery)?;
        let required = [
            Requirement {
                label: crate::OTA0,
                kind: crate::APP_TYPE,
                subtype: crate::OTA0_SUBTYPE,
            },
            Requirement {
                label: crate::OTA1,
                kind: crate::APP_TYPE,
                subtype: crate::OTA1_SUBTYPE,
            },
            Requirement {
                label: crate::OTADATA,
                kind: crate::DATA_TYPE,
                subtype: crate::OTADATA_SUBTYPE,
            },
        ];
        let [a0, a1, meta] = iobewi_esp_firmware_storage::partitions::discover(
            guard.storage(),
            &mut [0; iobewi_esp_partitions::TABLE_BUFFER_SIZE],
            &required,
            flash_size,
            <EspFlash as NorFlash>::ERASE_SIZE as u32,
        )
        .map_err(|_| Error::Discovery)?;
        let partition = |p: iobewi_esp_partitions::PartitionRange| Partition {
            offset: p.offset,
            size: p.size as u32,
        };
        let layout = Layout {
            images: [partition(a0), partition(a1)],
            otadata: partition(meta),
        };
        layout.validate(flash_size).map_err(|_| Error::Discovery)?;
        Ok(Self {
            layout,
            state: DomainState::new(Domain::A),
            flash_size,
        })
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    fn snapshot_locked(
        &self,
        flash: &mut EspFlash,
    ) -> Result<(running::RunningSlot, [Raw; SECTOR_COUNT]), Error> {
        let observed = iobewi_esp_partitions::booted_partition(
            flash.storage(),
            &mut [0; iobewi_esp_partitions::TABLE_BUFFER_SIZE],
        )
        .map_err(|_| Error::Observation)?;
        let running = running::from_running_partition(
            &self.layout,
            Partition {
                offset: observed.offset,
                size: observed.size as u32,
            },
        )
        .ok_or(Error::Observation)?;
        let records = otadata::read_entries(&mut LockedFlash(flash), self.layout.otadata)
            .map_err(|_| Error::Metadata)?;
        Ok((running, records))
    }

    async fn snapshot(
        &self,
        flash: &SharedFlash,
    ) -> Result<(running::RunningSlot, [Raw; SECTOR_COUNT]), Error> {
        let mut guard = flash.lock().await;
        self.snapshot_locked(&mut guard)
    }

    /// Check the historical write target against the actual mapped image, then
    /// reserve the same target through the shared domain reservation mechanism.
    pub async fn reserve<'a>(&'a mut self, flash: &SharedFlash) -> Result<WriteGuard<'a>, Error> {
        let (running, records) = self.snapshot(flash).await?;
        let observed = self.layout.images[usize::from(running.index())];
        running::reserve_inactive(
            &mut self.state,
            &self.layout,
            records,
            Some(observed),
            self.flash_size,
        )
        .map_err(Error::Reservation)
    }

    /// Activation is deliberately unavailable while reserve's guard exists.
    pub async fn activate(&mut self, flash: &SharedFlash, target: Slot) -> Result<(), Error> {
        let mut guard = flash.lock().await;
        let (running, records) = self.snapshot_locked(&mut guard)?;
        if boot::has_entry_in_state(records, boot::state::PENDING_VERIFY) {
            return Err(Error::Reservation(
                running::ReservationError::PendingConfirmation,
            ));
        }
        let chosen = selection::inactive(&self.layout, records, Some(running.index()))
            .map_err(Error::Selection)?;
        if chosen != self.layout.images[usize::from(target.index())] {
            return Err(Error::Observation);
        }
        let write = boot::activate(records, 2, target.index()).map_err(|_| Error::NoTransition)?;
        otadata::execute(&mut LockedFlash(&mut guard), self.layout.otadata, write)
            .map_err(|_| Error::Metadata)
    }

    pub async fn confirm(&mut self, flash: &SharedFlash) -> Result<(), Error> {
        self.transition(flash, boot::confirm).await
    }

    pub async fn reject(&mut self, flash: &SharedFlash) -> Result<(), Error> {
        self.transition(flash, boot::reject).await
    }

    async fn transition(
        &mut self,
        flash: &SharedFlash,
        plan: fn([Raw; SECTOR_COUNT]) -> Option<boot::Write>,
    ) -> Result<(), Error> {
        let mut guard = flash.lock().await;
        let (running, records) = self.snapshot_locked(&mut guard)?;
        let write = plan(records).ok_or(Error::NoTransition)?;
        if boot::slot_of(write.entry.seq, 2) != running.index() {
            return Err(Error::Observation);
        }
        otadata::execute(&mut LockedFlash(&mut guard), self.layout.otadata, write)
            .map_err(|_| Error::Metadata)
    }
}
