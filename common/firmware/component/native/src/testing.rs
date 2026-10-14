//! An in-memory [`NativeBackend`] for host tests: it records what was loaded and "executes"
//! the Workload as a scripted behaviour chosen by the first code byte, stepping whenever the
//! runtime waits. It proves the policy (gate, load, start, stop, health), not native
//! execution (that is the hardware gate).

extern crate std;

use core::cell::{Cell, RefCell};
use core::sync::atomic::Ordering;
use std::vec::Vec;

use iobewi_firmware_update::compat::otm2::RuntimeApi;
use iobewi_workload_abi::{ControlBlockV1, state};
use iobewi_workload_image::{HEADER_LEN, ImageHeader, TargetLayout, esp32s3_layout};

use crate::{LaunchError, NativeBackend};

pub const LAYOUT: TargetLayout = esp32s3_layout(RuntimeApi::new(1, 0), 1);

/// What the fake Workload does, selected by `code[0]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Behaviour {
    /// Runs, counts progress, returns when asked to stop.
    Cooperative = 0,
    /// Runs and never looks at the stop request.
    IgnoresStop = 1,
    /// `launch` succeeds but the Workload never announces RUNNING.
    NeverRuns = 2,
    /// Runs a few steps then reports FAILED (panic).
    Panics = 3,
    /// Runs, then stops making progress (frozen).
    Freezes = 4,
}

impl Behaviour {
    fn from_byte(b: u8) -> Self {
        match b {
            1 => Behaviour::IgnoresStop,
            2 => Behaviour::NeverRuns,
            3 => Behaviour::Panics,
            4 => Behaviour::Freezes,
            _ => Behaviour::Cooperative,
        }
    }
}

/// Build an image the fake backend understands, valid for [`LAYOUT`].
pub fn image(behaviour: Behaviour, seed: u8, requires: RuntimeApi) -> Vec<u8> {
    let mut code = std::vec![0u8; 64];
    code[0] = behaviour as u8;
    for (i, b) in code.iter_mut().enumerate().skip(1) {
        *b = (i as u8).wrapping_mul(13).wrapping_add(seed);
    }
    let data: Vec<u8> = (0..16).map(|i| (i as u8) ^ seed).collect();
    let header = ImageHeader {
        target: LAYOUT.target.code(),
        abi_version: LAYOUT.abi_version,
        requires,
        flags: 0,
        image_size: (HEADER_LEN + code.len() + data.len()) as u32,
        entry: LAYOUT.code_addr + 4,
        code_addr: LAYOUT.code_addr,
        code_offset: HEADER_LEN as u32,
        code_size: code.len() as u32,
        data_addr: LAYOUT.data_addr,
        data_offset: (HEADER_LEN + code.len()) as u32,
        data_size: data.len() as u32,
        bss_size: 32,
    };
    let mut out = header.encode().to_vec();
    out.extend_from_slice(&code);
    out.extend_from_slice(&data);
    out
}

pub struct FakeNative {
    control: ControlBlockV1,
    pub code: RefCell<Vec<u8>>,
    pub data: RefCell<Vec<u8>>,
    clock: Cell<u64>,
    running: Cell<bool>,
    behaviour: Cell<Behaviour>,
    steps: Cell<u32>,
    pub launches: Cell<u32>,
    pub halts: Cell<u32>,
    pub clears: Cell<u32>,
    /// Entry address of the last launch.
    pub last_entry: Cell<u32>,
    /// Code area writes happened after the last `clear_region`.
    pub fail_launch: Cell<bool>,
}

impl FakeNative {
    pub fn new() -> Self {
        Self {
            control: ControlBlockV1::new(),
            code: RefCell::new(Vec::new()),
            data: RefCell::new(Vec::new()),
            clock: Cell::new(1_000),
            running: Cell::new(false),
            behaviour: Cell::new(Behaviour::Cooperative),
            steps: Cell::new(0),
            launches: Cell::new(0),
            halts: Cell::new(0),
            clears: Cell::new(0),
            last_entry: Cell::new(0),
            fail_launch: Cell::new(false),
        }
    }

    /// Is the fake "core" executing Workload code right now?
    pub fn executing(&self) -> bool {
        self.running.get()
    }

    pub fn advance(&self, ms: u64) {
        self.clock.set(self.clock.get() + ms);
    }

    fn step(&self) {
        if !self.running.get() {
            return;
        }
        let c = &self.control;
        let steps = self.steps.get() + 1;
        self.steps.set(steps);
        let st = c.state.load(Ordering::Acquire);
        match (self.behaviour.get(), st) {
            (Behaviour::NeverRuns, _) => {}
            (_, state::STARTING) => c.state.store(state::RUNNING, Ordering::Release),
            (Behaviour::Cooperative, state::RUNNING) => {
                if c.stop_requested.load(Ordering::Acquire) != 0 {
                    c.state.store(state::STOPPED, Ordering::Release);
                    self.running.set(false);
                } else {
                    c.progress.fetch_add(1, Ordering::Release);
                }
            }
            (Behaviour::IgnoresStop, state::RUNNING) => {
                c.progress.fetch_add(1, Ordering::Release);
            }
            (Behaviour::Panics, state::RUNNING) => {
                if steps > 3 {
                    c.fault.store(1, Ordering::Release);
                    c.state.store(state::FAILED, Ordering::Release);
                } else {
                    c.progress.fetch_add(1, Ordering::Release);
                }
            }
            (Behaviour::Freezes, state::RUNNING) if steps <= 3 => {
                c.progress.fetch_add(1, Ordering::Release);
            }
            _ => {}
        }
    }
}

impl Default for FakeNative {
    fn default() -> Self {
        Self::new()
    }
}

impl NativeBackend for FakeNative {
    fn layout(&self) -> TargetLayout {
        LAYOUT
    }
    fn control(&self) -> &ControlBlockV1 {
        &self.control
    }
    fn clear_region(&self) {
        assert!(
            !self.running.get(),
            "the region must never be rewritten while code runs"
        );
        self.clears.set(self.clears.get() + 1);
        self.code.borrow_mut().clear();
        self.data.borrow_mut().clear();
    }
    fn write_code(&self, offset: u32, bytes: &[u8]) {
        assert!(!self.running.get(), "code written while a Workload runs");
        let mut code = self.code.borrow_mut();
        assert_eq!(offset as usize, code.len(), "code is copied in order");
        assert!(code.len() + bytes.len() <= LAYOUT.code_capacity as usize);
        code.extend_from_slice(bytes);
    }
    fn write_data(&self, offset: u32, bytes: &[u8]) {
        assert!(!self.running.get(), "data written while a Workload runs");
        let mut data = self.data.borrow_mut();
        assert_eq!(offset as usize, data.len());
        data.extend_from_slice(bytes);
    }
    fn launch(&self, entry: u32) -> Result<(), LaunchError> {
        if self.fail_launch.get() {
            return Err(LaunchError("launch failed"));
        }
        assert!(
            entry >= LAYOUT.code_addr && entry < LAYOUT.code_addr + self.code.borrow().len() as u32,
            "entry must be inside the loaded code"
        );
        self.last_entry.set(entry);
        self.launches.set(self.launches.get() + 1);
        self.behaviour
            .set(Behaviour::from_byte(self.code.borrow()[0]));
        self.steps.set(0);
        self.running.set(true);
        Ok(())
    }
    fn halt(&self) {
        self.halts.set(self.halts.get() + 1);
        self.running.set(false);
    }
    fn now_ms(&self) -> u64 {
        self.clock.get()
    }
    async fn delay_ms(&self, ms: u32) {
        self.advance(u64::from(ms));
        self.step();
    }
}
