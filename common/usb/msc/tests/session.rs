//! Public-class dialogues over Embassy driver contracts; no USB hardware is used.
use embassy_time::Duration;
use embassy_usb::{Builder, Config, driver::*};
use iobewi_block::{ReadOnlyBlockDevice, ReadStatus, SECTOR_SIZE};
use iobewi_usb_msc::{Error, InquiryIdentity, MscClass, ReadAction, ReadPolicy, State};
use std::{
    cell::RefCell,
    collections::VecDeque,
    future::Future,
    rc::Rc,
    sync::Arc,
    task::{Context, Poll, Wake, Waker},
};

#[derive(Default)]
struct Traffic {
    input: VecDeque<Result<Vec<u8>, EndpointError>>,
    output: Vec<Vec<u8>>,
    enables: usize,
}
type Shared = Rc<RefCell<Traffic>>;
struct FakeDriver(Shared);
struct FakeEndpoint {
    info: EndpointInfo,
    traffic: Shared,
}
impl Endpoint for FakeEndpoint {
    fn info(&self) -> &EndpointInfo {
        &self.info
    }
    async fn wait_enabled(&mut self) {
        self.traffic.borrow_mut().enables += 1;
    }
}
impl EndpointOut for FakeEndpoint {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, EndpointError> {
        let packet = self
            .traffic
            .borrow_mut()
            .input
            .pop_front()
            .expect("script exhausted")?;
        assert!(packet.len() <= buf.len());
        buf[..packet.len()].copy_from_slice(&packet);
        Ok(packet.len())
    }
}
impl EndpointIn for FakeEndpoint {
    async fn write(&mut self, buf: &[u8]) -> Result<(), EndpointError> {
        assert!(buf.len() <= self.info.max_packet_size as usize);
        self.traffic.borrow_mut().output.push(buf.to_vec());
        Ok(())
    }
}
struct Unused;
impl Bus for Unused {
    async fn enable(&mut self) {
        panic!("bus unused")
    }
    async fn disable(&mut self) {
        panic!("bus unused")
    }
    async fn poll(&mut self) -> Event {
        panic!("bus unused")
    }
    fn endpoint_set_enabled(&mut self, _: EndpointAddress, _: bool) {
        panic!("bus unused")
    }
    fn endpoint_set_stalled(&mut self, _: EndpointAddress, _: bool) {
        panic!("bus unused")
    }
    fn endpoint_is_stalled(&mut self, _: EndpointAddress) -> bool {
        panic!("bus unused")
    }
    async fn remote_wakeup(&mut self) -> Result<(), Unsupported> {
        panic!("bus unused")
    }
}
impl ControlPipe for Unused {
    fn max_packet_size(&self) -> usize {
        64
    }
    async fn setup(&mut self) -> [u8; 8] {
        panic!("control unused")
    }
    async fn data_out(&mut self, _: &mut [u8], _: bool, _: bool) -> Result<usize, EndpointError> {
        panic!("control unused")
    }
    async fn data_in(&mut self, _: &[u8], _: bool, _: bool) -> Result<(), EndpointError> {
        panic!("control unused")
    }
    async fn accept(&mut self) {
        panic!("control unused")
    }
    async fn reject(&mut self) {
        panic!("control unused")
    }
    async fn accept_set_address(&mut self, _: u8) {
        panic!("control unused")
    }
}
impl<'a> Driver<'a> for FakeDriver {
    type EndpointOut = FakeEndpoint;
    type EndpointIn = FakeEndpoint;
    type Bus = Unused;
    type ControlPipe = Unused;
    fn alloc_endpoint_out(
        &mut self,
        t: EndpointType,
        a: Option<EndpointAddress>,
        m: u16,
        i: u8,
    ) -> Result<FakeEndpoint, EndpointAllocError> {
        assert_eq!(t, EndpointType::Bulk);
        assert_eq!(m, 64);
        Ok(FakeEndpoint {
            info: EndpointInfo {
                addr: a.unwrap_or(EndpointAddress::from_parts(1, Direction::Out)),
                ep_type: t,
                max_packet_size: m,
                interval_ms: i,
            },
            traffic: self.0.clone(),
        })
    }
    fn alloc_endpoint_in(
        &mut self,
        t: EndpointType,
        a: Option<EndpointAddress>,
        m: u16,
        i: u8,
    ) -> Result<FakeEndpoint, EndpointAllocError> {
        assert_eq!(t, EndpointType::Bulk);
        assert_eq!(m, 64);
        Ok(FakeEndpoint {
            info: EndpointInfo {
                addr: a.unwrap_or(EndpointAddress::from_parts(1, Direction::In)),
                ep_type: t,
                max_packet_size: m,
                interval_ms: i,
            },
            traffic: self.0.clone(),
        })
    }
    fn start(self, _: u16) -> (Unused, Unused) {
        (Unused, Unused)
    }
}
struct ThreadWake(std::thread::Thread);
impl Wake for ThreadWake {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}
fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(ThreadWake(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        assert!(std::time::Instant::now() < deadline, "dialogue stalled");
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return v,
            Poll::Pending => std::thread::park_timeout(std::time::Duration::from_secs(5)),
        }
    }
}
const ID: InquiryIdentity = InquiryIdentity {
    vendor: *b"TESTVEND",
    product: *b"PUBLIC MSC TEST ",
    revision: *b"0001",
};
#[derive(Default)]
struct Disk {
    last: u32,
    events: Vec<String>,
    statuses: VecDeque<ReadStatus>,
    session: u8,
}
impl ReadOnlyBlockDevice for Disk {
    fn last_lba(&self) -> u32 {
        self.last
    }
    fn begin_session(&mut self) {
        self.session += 1;
        self.events.push("begin".into());
    }
    fn end_session(&mut self) {
        self.events.push("end".into());
    }
    fn read_sector(&mut self, lba: u32, out: &mut [u8; SECTOR_SIZE]) -> ReadStatus {
        self.events.push(format!("read:{lba}"));
        let status = self.statuses.pop_front().unwrap_or(ReadStatus::Ready);
        out.fill(if status == ReadStatus::Ready {
            self.session * 16 + lba as u8
        } else {
            0
        });
        status
    }
}
#[derive(Default)]
struct Policy {
    retry: bool,
    seen: Vec<(ReadStatus, Duration)>,
}
impl ReadPolicy for Policy {
    fn unavailable(&mut self, status: ReadStatus, elapsed: Duration) -> ReadAction {
        self.seen.push((status, elapsed));
        if self.retry {
            ReadAction::RetryAfter(Duration::from_millis(1))
        } else {
            ReadAction::ZeroFill
        }
    }
}
fn command(tag: u32, len: u32, op: u8) -> Vec<u8> {
    let mut b = vec![0; 31];
    b[..4].copy_from_slice(&0x43425355u32.to_le_bytes());
    b[4..8].copy_from_slice(&tag.to_le_bytes());
    b[8..12].copy_from_slice(&len.to_le_bytes());
    b[12] = 0x80;
    b[14] = 16;
    b[15] = op;
    b
}
fn read(tag: u32, len: u32, lba: u32, blocks: u16) -> Vec<u8> {
    let mut b = command(tag, len, 0x28);
    b[17..21].copy_from_slice(&lba.to_be_bytes());
    b[22..24].copy_from_slice(&blocks.to_be_bytes());
    b
}
fn csw(tag: u32, residue: u32, status: u8) -> Vec<u8> {
    let mut b = b"USBS".to_vec();
    b.extend(tag.to_le_bytes());
    b.extend(residue.to_le_bytes());
    b.push(status);
    b
}
fn sense(key: u8, asc: u8) -> Vec<u8> {
    let mut b = vec![0; 18];
    b[0] = 0x70;
    b[2] = key;
    b[7] = 10;
    b[12] = asc;
    b
}
fn run(
    script: Vec<Result<Vec<u8>, EndpointError>>,
    disk: &mut impl ReadOnlyBlockDevice,
    policy: &mut Policy,
) -> (Result<(), Error>, Shared) {
    let shared = Rc::new(RefCell::new(Traffic {
        input: script.into(),
        ..Default::default()
    }));
    let mut state = State::new();
    let mut config = [0; 256];
    let mut bos = [0; 256];
    let mut msos = [0; 256];
    let mut control = [0; 64];
    let mut builder = Builder::new(
        FakeDriver(shared.clone()),
        Config::new(0x1234, 0x5678),
        &mut config,
        &mut bos,
        &mut msos,
        &mut control,
    );
    let mut class = MscClass::new(&mut builder, &mut state, ID);
    let _usb = builder.build();
    (block_on(class.run(disk, policy)), shared)
}
fn finished(commands: Vec<Vec<u8>>, disk: &mut Disk, policy: &mut Policy) -> Shared {
    let mut script: Vec<_> = commands.into_iter().map(Ok).collect();
    script.push(Err(EndpointError::BufferOverflow));
    let (result, traffic) = run(script, disk, policy);
    assert_eq!(result, Err(Error::Endpoint(EndpointError::BufferOverflow)));
    traffic
}

#[test]
fn inquiry_capacity_write_protection_and_consumed_sense() {
    let mut disk = Disk {
        last: 0x1234,
        ..Default::default()
    };
    let mut policy = Policy::default();
    let traffic = finished(
        vec![
            command(1, 40, 0x12),
            command(2, 8, 0x25),
            command(3, 12, 0x23),
            command(4, 4, 0x1a),
            command(5, 99, 0xff),
            command(6, 18, 3),
            command(7, 18, 3),
            command(8, 0, 0),
            command(9, 0, 0x1b),
            command(10, 0, 0x1e),
        ],
        &mut disk,
        &mut policy,
    );
    let mut inquiry = vec![0, 0x80, 4, 2, 31, 0, 0, 0];
    inquiry.extend(ID.vendor);
    inquiry.extend(ID.product);
    inquiry.extend(ID.revision);
    assert_eq!(
        traffic.borrow().output,
        vec![
            inquiry,
            csw(1, 4, 0),
            vec![0, 0, 0x12, 0x34, 0, 0, 2, 0],
            csw(2, 0, 0),
            vec![0, 0, 0, 8, 0, 0, 0x12, 0x35, 2, 0, 2, 0],
            csw(3, 0, 0),
            vec![3, 0, 0x80, 0],
            csw(4, 0, 0),
            csw(5, 99, 1),
            sense(5, 0x20),
            csw(6, 0, 0),
            sense(0, 0),
            csw(7, 0, 0),
            csw(8, 0, 0),
            csw(9, 0, 0),
            csw(10, 0, 0)
        ]
    );
    assert_eq!(disk.events, ["begin", "end"]);
    assert!(policy.seen.is_empty());
}
#[test]
fn read_multiblock_packets_residue_and_invalid_ranges() {
    let mut disk = Disk {
        last: 3,
        ..Default::default()
    };
    let mut policy = Policy::default();
    let traffic = finished(
        vec![
            read(0x12345678, 1100, 1, 2),
            read(2, 1, 3, 1),
            read(3, 512, 4, 1),
            command(4, 18, 3),
            read(5, 1024, 3, 2),
            command(6, 18, 3),
        ],
        &mut disk,
        &mut policy,
    );
    let mut expected = vec![vec![17; 64]; 8];
    expected.extend(vec![vec![18; 64]; 8]);
    expected.push(csw(0x12345678, 76, 0));
    expected.extend(vec![vec![19; 64]; 8]);
    expected.extend([
        csw(2, 0, 0),
        csw(3, 512, 1),
        sense(5, 0x21),
        csw(4, 0, 0),
        csw(5, 1024, 1),
        sense(5, 0x21),
        csw(6, 0, 0),
    ]);
    assert_eq!(traffic.borrow().output, expected);
    assert_eq!(disk.events, ["begin", "read:1", "read:2", "read:3", "end"]);
}
#[test]
fn pending_retries_then_ready_and_unavailable_zero_fills() {
    let mut disk = Disk {
        last: 1,
        statuses: [ReadStatus::Pending, ReadStatus::Ready].into(),
        ..Default::default()
    };
    let mut policy = Policy {
        retry: true,
        ..Default::default()
    };
    let traffic = finished(vec![read(1, 512, 0, 1)], &mut disk, &mut policy);
    let mut expected = vec![vec![16; 64]; 8];
    expected.push(csw(1, 0, 0));
    assert_eq!(traffic.borrow().output, expected);
    assert_eq!(disk.events, ["begin", "read:0", "read:0", "end"]);
    assert_eq!(policy.seen.len(), 1);
    assert_eq!(policy.seen[0].0, ReadStatus::Pending);
    let mut disk = Disk {
        last: 1,
        statuses: [ReadStatus::Pending, ReadStatus::Expired].into(),
        ..Default::default()
    };
    let mut policy = Policy::default();
    let traffic = finished(vec![read(2, 1024, 0, 2)], &mut disk, &mut policy);
    let mut expected = vec![vec![0; 64]; 16];
    expected.push(csw(2, 0, 0));
    assert_eq!(traffic.borrow().output, expected);
    assert_eq!(
        policy.seen.iter().map(|x| x.0).collect::<Vec<_>>(),
        [ReadStatus::Pending, ReadStatus::Expired]
    );
}
#[test]
fn disabled_ends_session_and_reenable_rebases_medium_and_clears_sense() {
    let mut disk = Disk {
        last: 0,
        ..Default::default()
    };
    let mut policy = Policy::default();
    let (result, traffic) = run(
        vec![
            Ok(read(1, 512, 0, 1)),
            Ok(command(2, 0, 0xff)),
            Err(EndpointError::Disabled),
            Ok(command(3, 18, 3)),
            Ok(read(4, 512, 0, 1)),
            Err(EndpointError::BufferOverflow),
        ],
        &mut disk,
        &mut policy,
    );
    assert_eq!(result, Err(Error::Endpoint(EndpointError::BufferOverflow)));
    assert_eq!(traffic.borrow().enables, 2);
    assert_eq!(
        disk.events,
        ["begin", "read:0", "end", "begin", "read:0", "end"]
    );
    let mut expected = vec![vec![16; 64]; 8];
    expected.extend([csw(1, 0, 0), csw(2, 0, 1), sense(0, 0), csw(3, 0, 0)]);
    expected.extend(vec![vec![32; 64]; 8]);
    expected.push(csw(4, 0, 0));
    assert_eq!(traffic.borrow().output, expected);
}
#[test]
fn unsupported_capacity_fails_before_opening_session() {
    let mut disk = Disk {
        last: u32::MAX,
        ..Default::default()
    };
    let (result, traffic) = run(vec![], &mut disk, &mut Policy::default());
    assert_eq!(result, Err(Error::UnsupportedCapacity));
    assert!(disk.events.is_empty());
    assert_eq!(traffic.borrow().enables, 0);
    assert!(traffic.borrow().output.is_empty());
}

/// Exercise the actual migrated bricks together, through CBW/data/CSW packets.
/// This is a host dialogue, not a claim of physical USB enumeration.
#[test]
fn rolling_fat_msc_bytes_underrun_recovery_and_new_session() {
    use iobewi_fat16::{Fat16Config, FileSource, VirtualFat16};
    use iobewi_rolling_stream::{ReadStatus as WindowStatus, RollingStream};

    struct WindowFile(Rc<RefCell<RollingStream<2048>>>);
    impl FileSource for WindowFile {
        fn begin_session(&mut self) {
            self.0.borrow_mut().begin_session();
        }
        fn end_session(&mut self) {
            self.0.borrow_mut().end_session();
        }
        fn read_file_sector(&mut self, index: u32, out: &mut [u8; SECTOR_SIZE]) -> ReadStatus {
            match self.0.borrow_mut().read(u64::from(index) * 512, out) {
                WindowStatus::Ready => ReadStatus::Ready,
                WindowStatus::Pending => ReadStatus::Pending,
                WindowStatus::Expired => ReadStatus::Expired,
            }
        }
    }
    let window = Rc::new(RefCell::new(RollingStream::new(512, 1536)));
    let config = Fat16Config {
        sectors_per_cluster: 1,
        file_cluster_count: 4085,
        root_entries: 16,
        file_name: *b"LIVE    BIN",
        volume_label: *b"TEST VOLUME",
        volume_serial: 42,
    };
    let data_lba = config.geometry().unwrap().data_start_lba;
    let mut volume = VirtualFat16::new(WindowFile(window.clone()), config).unwrap();
    let mut policy = Policy::default(); // unavailable sectors are zero-filled here
    assert_eq!(window.borrow_mut().push(&[0x5a; 512]), 512);
    let (result, traffic) = run(
        vec![
            Ok(read(1, 512, data_lba, 1)),
            Ok(read(2, 512, data_lba + 1, 1)),
            Err(EndpointError::BufferOverflow),
        ],
        &mut volume,
        &mut policy,
    );
    assert_eq!(result, Err(Error::Endpoint(EndpointError::BufferOverflow)));
    let mut expected = vec![vec![0x5a; 64]; 8];
    expected.push(csw(1, 0, 0));
    expected.extend(vec![vec![0; 64]; 8]);
    expected.push(csw(2, 0, 0));
    assert_eq!(traffic.borrow().output, expected);
    assert_eq!(policy.seen.len(), 1);
    assert_eq!(policy.seen[0].0, ReadStatus::Pending);
    assert!(window.borrow().session().is_none());

    // Producer continues while disconnected; wrapped old bytes must not be replayed.
    assert_eq!(window.borrow_mut().push(&[0xa5; 2048]), 2048);
    let (result, traffic) = run(
        vec![
            Ok(read(3, 512, data_lba, 1)),
            Err(EndpointError::BufferOverflow),
        ],
        &mut volume,
        &mut policy,
    );
    assert_eq!(result, Err(Error::Endpoint(EndpointError::BufferOverflow)));
    let mut expected = vec![vec![0xa5; 64]; 8];
    expected.push(csw(3, 0, 0));
    assert_eq!(traffic.borrow().output, expected);
    assert!(window.borrow().session().is_none());
    assert_eq!(window.borrow().written(), 2560);
}
