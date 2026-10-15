#![no_std]
#![doc = include_str!("../README.md")]

use core::mem::MaybeUninit;

use embassy_time::{Duration, Instant, Timer};
use embassy_usb::{
    Builder, Handler,
    control::{self, InResponse, OutResponse, Recipient, RequestType},
    driver::{Driver, Endpoint, EndpointError, EndpointIn, EndpointOut},
    types::InterfaceNumber,
};
use iobewi_block::{ReadOnlyBlockDevice, ReadStatus, SECTOR_SIZE};

const USB_CLASS_MASS_STORAGE: u8 = 0x08;
const MSC_SUBCLASS_SCSI_TRANSPARENT: u8 = 0x06;
const MSC_PROTOCOL_BULK_ONLY: u8 = 0x50;

const MSC_REQ_GET_MAX_LUN: u8 = 0xFE;
const MSC_REQ_BULK_ONLY_RESET: u8 = 0xFF;

const CBW_SIGNATURE: u32 = 0x4342_5355;
const CSW_SIGNATURE: u32 = 0x5342_5355;

const SCSI_TEST_UNIT_READY: u8 = 0x00;
const SCSI_REQUEST_SENSE: u8 = 0x03;
const SCSI_INQUIRY: u8 = 0x12;
const SCSI_MODE_SENSE_6: u8 = 0x1A;
const SCSI_START_STOP_UNIT: u8 = 0x1B;
const SCSI_PREVENT_ALLOW_MEDIUM_REMOVAL: u8 = 0x1E;
const SCSI_READ_FORMAT_CAPACITIES: u8 = 0x23;
const SCSI_READ_CAPACITY_10: u8 = 0x25;
const SCSI_READ_10: u8 = 0x28;

/// Fixed-width SCSI inquiry identification supplied by the composition root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InquiryIdentity {
    pub vendor: [u8; 8],
    pub product: [u8; 16],
    pub revision: [u8; 4],
}

/// Consumer decision when a sector is unavailable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadAction {
    RetryAfter(Duration),
    /// Send the zero-filled sector required by the block contract as a successful read.
    ZeroFill,
}

/// Product-owned retry, timeout and stale-data policy.
pub trait ReadPolicy {
    /// Called at the start of every READ(10) command, before its first sector.
    ///
    /// A policy that must bound the wait of a whole command (the host aborts a command after
    /// its own timeout, whatever the number of sectors it asked for) records the instant here
    /// and compares it with the clock in [`unavailable`](Self::unavailable). The default does
    /// nothing, so existing per-sector policies are unaffected.
    fn command_started(&mut self) {}

    /// Called only for Pending or Expired; elapsed is measured from the first read of this sector.
    fn unavailable(&mut self, status: ReadStatus, elapsed: Duration) -> ReadAction;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// READ FORMAT CAPACITIES cannot represent last_lba + 1 in a u32.
    UnsupportedCapacity,
    Endpoint(EndpointError),
}

impl From<EndpointError> for Error {
    fn from(error: EndpointError) -> Self {
        Self::Endpoint(error)
    }
}

const SENSE_ILLEGAL_REQUEST: u8 = 0x05;
const ASC_INVALID_OPCODE: u8 = 0x20;
const ASC_LBA_OUT_OF_RANGE: u8 = 0x21;

pub struct State<'a> {
    control: MaybeUninit<Control>,
    _lifetime: core::marker::PhantomData<&'a ()>,
}

impl<'a> State<'a> {
    pub const fn new() -> Self {
        Self {
            control: MaybeUninit::uninit(),
            _lifetime: core::marker::PhantomData,
        }
    }
}

impl Default for State<'_> {
    fn default() -> Self {
        Self::new()
    }
}

struct Control {
    interface: InterfaceNumber,
}

impl Handler for Control {
    fn control_out(&mut self, req: control::Request, _data: &[u8]) -> Option<OutResponse> {
        if (req.request_type, req.recipient, req.index)
            != (
                RequestType::Class,
                Recipient::Interface,
                self.interface.0 as u16,
            )
        {
            return None;
        }

        match req.request {
            MSC_REQ_BULK_ONLY_RESET if req.value == 0 && req.length == 0 => {
                log::info!("msc: bulk-only reset");
                Some(OutResponse::Accepted)
            }
            _ => Some(OutResponse::Rejected),
        }
    }

    fn control_in<'a>(
        &'a mut self,
        req: control::Request,
        buf: &'a mut [u8],
    ) -> Option<InResponse<'a>> {
        if (req.request_type, req.recipient, req.index)
            != (
                RequestType::Class,
                Recipient::Interface,
                self.interface.0 as u16,
            )
        {
            return None;
        }

        match req.request {
            MSC_REQ_GET_MAX_LUN if req.value == 0 && req.length == 1 => {
                #[cfg(feature = "usb-debug")]
                log::info!("msc: get max lun");
                buf[0] = 0; // one LUN
                Some(InResponse::Accepted(&buf[..1]))
            }
            _ => Some(InResponse::Rejected),
        }
    }
}

#[derive(Clone, Copy)]
struct ReadStats {
    commands: u32,
    blocks: u64,
    min_lba: u32,
    max_lba: u32,
}

impl ReadStats {
    const fn new() -> Self {
        Self {
            commands: 0,
            blocks: 0,
            min_lba: u32::MAX,
            max_lba: 0,
        }
    }

    fn reset(&mut self) {
        *self = Self::new();
    }

    fn record(&mut self, lba: u32, blocks: u32) -> bool {
        self.commands = self.commands.saturating_add(1);
        self.blocks = self.blocks.saturating_add(blocks as u64);
        self.min_lba = core::cmp::min(self.min_lba, lba);
        self.max_lba = core::cmp::max(self.max_lba, lba.saturating_add(blocks.saturating_sub(1)));
        self.commands.is_multiple_of(256)
    }

    fn log(&self, prefix: &str) {
        if self.commands == 0 {
            log::info!("msc: {} reads=0", prefix);
        } else {
            log::info!(
                "msc: {} reads={} blocks={} bytes={} lba={}..{}",
                prefix,
                self.commands,
                self.blocks,
                self.blocks * SECTOR_SIZE as u64,
                self.min_lba,
                self.max_lba
            );
        }
    }
}

pub struct MscClass<'d, D: Driver<'d>> {
    read_ep: D::EndpointOut,
    write_ep: D::EndpointIn,
    stats: ReadStats,
    identity: InquiryIdentity,
}

impl<'d, D: Driver<'d>> MscClass<'d, D> {
    pub fn new(
        builder: &mut Builder<'d, D>,
        state: &'d mut State<'d>,
        identity: InquiryIdentity,
    ) -> Self {
        let mut function = builder.function(
            USB_CLASS_MASS_STORAGE,
            MSC_SUBCLASS_SCSI_TRANSPARENT,
            MSC_PROTOCOL_BULK_ONLY,
        );
        let mut interface = function.interface();
        let interface_number = interface.interface_number();
        let mut alt = interface.alt_setting(
            USB_CLASS_MASS_STORAGE,
            MSC_SUBCLASS_SCSI_TRANSPARENT,
            MSC_PROTOCOL_BULK_ONLY,
            None,
        );

        let read_ep = alt.endpoint_bulk_out(None, 64);
        let write_ep = alt.endpoint_bulk_in(None, 64);

        drop(function);

        let control = state.control.write(Control {
            interface: interface_number,
        });
        builder.handler(control);

        Self {
            read_ep,
            write_ep,
            stats: ReadStats::new(),
            identity,
        }
    }

    pub async fn run<B: ReadOnlyBlockDevice, P: ReadPolicy>(
        &mut self,
        disk: &mut B,
        policy: &mut P,
    ) -> Result<(), Error> {
        let total_sectors = total_sectors(disk.last_lba())?;
        loop {
            self.read_ep.wait_enabled().await;
            self.stats.reset();
            disk.begin_session();
            log::info!("msc: connected");

            match self.run_connected(disk, policy, total_sectors).await {
                Ok(()) => {
                    self.stats.log("session end");
                    disk.end_session();
                }
                Err(EndpointError::Disabled) => {
                    self.stats.log("session end");
                    disk.end_session();
                    log::info!("msc: disconnected");
                }
                Err(e) => {
                    self.stats.log("session error");
                    disk.end_session();
                    return Err(e.into());
                }
            }
        }
    }

    async fn run_connected<B: ReadOnlyBlockDevice, P: ReadPolicy>(
        &mut self,
        disk: &mut B,
        policy: &mut P,
        total_sectors: u32,
    ) -> Result<(), EndpointError> {
        let mut packet = [0u8; 64];
        // (sense key, ASC) of the last failed command, reported once by REQUEST SENSE.
        let mut sense = (0u8, 0u8);
        // Diagnostics: non-READ(10) commands are not otherwise visible in the log.
        let mut cmds_logged = 0u32;

        loop {
            let n = self.read_ep.read(&mut packet).await?;
            if n != 31 {
                log::info!("msc: ignoring non-CBW packet len={}", n);
                continue;
            }

            let Some(cbw) = Cbw::parse(&packet[..31]) else {
                log::info!("msc: invalid CBW");
                continue;
            };

            if cfg!(feature = "usb-debug") && cbw.command[0] != SCSI_READ_10 && cmds_logged < 24 {
                cmds_logged += 1;
                log::info!(
                    "msc: cmd op=0x{:02x} xfer={} tag={}",
                    cbw.command[0],
                    cbw.transfer_len,
                    cbw.tag
                );
            }

            let mut residue = cbw.transfer_len;
            let mut status = 0u8;

            match cbw.command[0] {
                SCSI_TEST_UNIT_READY => {}

                SCSI_INQUIRY => {
                    let data = inquiry(self.identity);
                    let sent = self.send_limited(&data, cbw.transfer_len).await?;
                    residue = residue.saturating_sub(sent as u32);
                }

                SCSI_REQUEST_SENSE => {
                    let data = request_sense(sense.0, sense.1);
                    sense = (0, 0);
                    let sent = self.send_limited(&data, cbw.transfer_len).await?;
                    residue = residue.saturating_sub(sent as u32);
                }

                SCSI_MODE_SENSE_6 => {
                    // Write-protected direct-access removable block device.
                    let data = [3u8, 0, 0x80, 0];
                    let sent = self.send_limited(&data, cbw.transfer_len).await?;
                    residue = residue.saturating_sub(sent as u32);
                }

                SCSI_READ_CAPACITY_10 => {
                    let mut data = [0u8; 8];
                    data[..4].copy_from_slice(&(total_sectors - 1).to_be_bytes());
                    data[4..].copy_from_slice(&(SECTOR_SIZE as u32).to_be_bytes());
                    let sent = self.send_limited(&data, cbw.transfer_len).await?;
                    residue = residue.saturating_sub(sent as u32);
                }

                SCSI_READ_FORMAT_CAPACITIES => {
                    let mut data = [0u8; 12];
                    data[3] = 8; // one capacity descriptor
                    data[4..8].copy_from_slice(&total_sectors.to_be_bytes());
                    data[8] = 0x02; // formatted media
                    let block_len = SECTOR_SIZE as u32;
                    data[9] = ((block_len >> 16) & 0xff) as u8;
                    data[10] = ((block_len >> 8) & 0xff) as u8;
                    data[11] = (block_len & 0xff) as u8;
                    let sent = self.send_limited(&data, cbw.transfer_len).await?;
                    residue = residue.saturating_sub(sent as u32);
                }

                SCSI_START_STOP_UNIT | SCSI_PREVENT_ALLOW_MEDIUM_REMOVAL => {}

                SCSI_READ_10 => {
                    let lba = u32::from_be_bytes([
                        cbw.command[2],
                        cbw.command[3],
                        cbw.command[4],
                        cbw.command[5],
                    ]);
                    let blocks = u16::from_be_bytes([cbw.command[7], cbw.command[8]]) as u32;

                    if self.stats.record(lba, blocks) {
                        self.stats.log("progress");
                    }

                    if lba >= total_sectors || blocks > total_sectors - lba {
                        sense = (SENSE_ILLEGAL_REQUEST, ASC_LBA_OUT_OF_RANGE);
                        status = 1;
                    } else {
                        let mut sector = [0u8; SECTOR_SIZE];
                        let mut sent_total = 0u32;
                        policy.command_started();

                        for block in 0..blocks {
                            let block_lba = lba + block;
                            let mut pending_logged = false;
                            let waiting_since = Instant::now();

                            loop {
                                match disk.read_sector(block_lba, &mut sector) {
                                    ReadStatus::Ready => break,
                                    status => {
                                        if !pending_logged {
                                            log::info!(
                                                "msc: unavailable sector lba={} status={:?}",
                                                block_lba,
                                                status
                                            );
                                            pending_logged = true;
                                        }
                                        match policy.unavailable(status, waiting_since.elapsed()) {
                                            ReadAction::RetryAfter(delay) => {
                                                Timer::after(delay).await
                                            }
                                            ReadAction::ZeroFill => break,
                                        }
                                    }
                                }
                            }

                            self.write_bytes(&sector).await?;
                            sent_total += SECTOR_SIZE as u32;
                        }

                        residue = residue.saturating_sub(sent_total);
                    }
                }

                other => {
                    log::info!(
                        "msc: unsupported SCSI opcode=0x{:02x} xfer={}",
                        other,
                        cbw.transfer_len
                    );
                    sense = (SENSE_ILLEGAL_REQUEST, ASC_INVALID_OPCODE);
                    status = 1;
                }
            }

            self.send_csw(cbw.tag, residue, status).await?;
        }
    }

    async fn send_limited(
        &mut self,
        data: &[u8],
        transfer_len: u32,
    ) -> Result<usize, EndpointError> {
        let len = core::cmp::min(data.len(), transfer_len as usize);
        self.write_bytes(&data[..len]).await?;
        Ok(len)
    }

    async fn write_bytes(&mut self, mut data: &[u8]) -> Result<(), EndpointError> {
        while !data.is_empty() {
            let n = core::cmp::min(64, data.len());
            self.write_ep.write(&data[..n]).await?;
            data = &data[n..];
        }
        Ok(())
    }

    async fn send_csw(&mut self, tag: u32, residue: u32, status: u8) -> Result<(), EndpointError> {
        self.write_ep
            .write(&command_status(tag, residue, status))
            .await
    }
}

struct Cbw {
    tag: u32,
    transfer_len: u32,
    command: [u8; 16],
}

impl Cbw {
    fn parse(data: &[u8]) -> Option<Self> {
        if data.len() != 31 {
            return None;
        }

        if u32::from_le_bytes(data[0..4].try_into().ok()?) != CBW_SIGNATURE {
            return None;
        }

        if data[13] > 15 || data[14] == 0 || data[14] > 16 {
            return None;
        }

        let mut command = [0u8; 16];
        command.copy_from_slice(&data[15..31]);

        Some(Self {
            tag: u32::from_le_bytes(data[4..8].try_into().ok()?),
            transfer_len: u32::from_le_bytes(data[8..12].try_into().ok()?),
            command,
        })
    }
}

fn inquiry(identity: InquiryIdentity) -> [u8; 36] {
    let mut data = [0u8; 36];
    data[0] = 0x00; // direct-access block device
    data[1] = 0x80; // removable
    data[2] = 0x04; // SPC-2-ish
    data[3] = 0x02;
    data[4] = 31;
    data[8..16].copy_from_slice(&identity.vendor);
    data[16..32].copy_from_slice(&identity.product);
    data[32..36].copy_from_slice(&identity.revision);
    data
}

fn request_sense(key: u8, asc: u8) -> [u8; 18] {
    let mut data = [0u8; 18];
    data[0] = 0x70;
    data[2] = key;
    data[7] = 10;
    data[12] = asc;
    data
}

fn total_sectors(last_lba: u32) -> Result<u32, Error> {
    last_lba.checked_add(1).ok_or(Error::UnsupportedCapacity)
}

fn command_status(tag: u32, residue: u32, status: u8) -> [u8; 13] {
    let mut csw = [0u8; 13];
    csw[0..4].copy_from_slice(&CSW_SIGNATURE.to_le_bytes());
    csw[4..8].copy_from_slice(&tag.to_le_bytes());
    csw[8..12].copy_from_slice(&residue.to_le_bytes());
    csw[12] = status;
    csw
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cbw() -> [u8; 31] {
        let mut packet = [0u8; 31];
        packet[..4].copy_from_slice(&CBW_SIGNATURE.to_le_bytes());
        packet[4..8].copy_from_slice(&0x8765_4321u32.to_le_bytes());
        packet[8..12].copy_from_slice(&1024u32.to_le_bytes());
        packet[12] = 0x80;
        packet[14] = 10;
        packet[15] = SCSI_READ_10;
        packet[17..21].copy_from_slice(&123u32.to_be_bytes());
        packet[22..24].copy_from_slice(&2u16.to_be_bytes());
        packet
    }

    #[test]
    fn golden_cbw_fields_and_rejections() {
        let packet = cbw();
        let parsed = Cbw::parse(&packet).unwrap();
        assert_eq!(parsed.tag, 0x8765_4321);
        assert_eq!(parsed.transfer_len, 1024);
        assert_eq!(parsed.command, packet[15..31]);
        assert!(Cbw::parse(&packet[..30]).is_none());
        for (index, value) in [(0, 0), (13, 16), (14, 0), (14, 17)] {
            let mut invalid = packet;
            invalid[index] = value;
            assert!(Cbw::parse(&invalid).is_none());
        }
    }

    #[test]
    fn parser_preserves_reference_lun_flags_and_cdb_len_limits() {
        let mut packet = cbw();
        packet[12] = 0xff; // flags deliberately ignored by the golden implementation
        packet[13] = 15; // one LUN advertised, but parser accepts these values
        for length in 1..=16 {
            packet[14] = length;
            assert!(Cbw::parse(&packet).is_some());
        }
    }

    #[test]
    fn inquiry_preserves_golden_structure_and_injected_identity() {
        let identity = InquiryIdentity {
            vendor: *b"IOBEWI  ",
            product: *b"USB RADIO POC   ",
            revision: *b"0001",
        };
        let data = inquiry(identity);
        assert_eq!(&data[..8], &[0, 0x80, 4, 2, 31, 0, 0, 0]);
        assert_eq!(&data[8..16], &identity.vendor);
        assert_eq!(&data[16..32], &identity.product);
        assert_eq!(&data[32..], &identity.revision);
        let other = InquiryIdentity {
            vendor: *b"OTHER   ",
            ..identity
        };
        assert_eq!(&inquiry(other)[8..16], b"OTHER   ");
    }

    #[test]
    fn fixed_sense_and_csw_bytes_match_reference() {
        let mut expected = [0u8; 18];
        expected[0] = 0x70;
        expected[2] = 5;
        expected[7] = 10;
        expected[12] = 0x21;
        assert_eq!(request_sense(5, 0x21), expected);
        assert_eq!(
            command_status(0x1234_5678, 0x90ab_cdef, 1),
            [
                0x55, 0x53, 0x42, 0x53, 0x78, 0x56, 0x34, 0x12, 0xef, 0xcd, 0xab, 0x90, 1,
            ]
        );
    }

    #[test]
    fn capacity_rejects_only_unrepresentable_sector_count() {
        assert_eq!(total_sectors(0), Ok(1));
        assert_eq!(total_sectors(65535), Ok(65536));
        assert_eq!(total_sectors(u32::MAX - 1), Ok(u32::MAX));
        assert_eq!(total_sectors(u32::MAX), Err(Error::UnsupportedCapacity));
    }
}
