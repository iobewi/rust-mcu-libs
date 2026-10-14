//! Host qualification of EKV itself, without ConfigSpace framing or ESP drivers.
//! `cargo run -p iobewi-config-space --example qualify_ekv -- wear|cuts|reproduce`
//! A failed persistence gate returns exit code 1; compiling this example is not
//! equivalent to running it. No image produced here is a deployed flash format.
use core::{
    cell::RefCell,
    future::Future,
    task::{Context, Poll, Waker},
};
use ekv::{
    Database,
    flash::{Flash, PageID},
};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use std::{collections::BTreeMap, rc::Rc};

const PAGE: usize = 4096;
const PAGES: usize = 64;
type Db = Database<Memory, CriticalSectionRawMutex>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fault {
    PowerOff,
}
#[derive(Clone, Copy, Debug)]
struct Operation {
    erase: bool,
    page: usize,
    offset: usize,
    len: usize,
}
#[derive(Clone)]
struct Memory(Rc<RefCell<Image>>);
struct Image {
    bytes: Vec<u8>,
    programmed_once: Vec<bool>,
    erases: Vec<u64>,
    programmed: u64,
    operations: Vec<Operation>,
    cut: Option<(usize, usize)>,
    off: bool,
}
impl Memory {
    fn new() -> Self {
        Self(Rc::new(RefCell::new(Image {
            bytes: vec![255; PAGES * PAGE],
            programmed_once: vec![false; PAGES * PAGE],
            erases: vec![0; PAGES],
            programmed: 0,
            operations: Vec::new(),
            cut: None,
            off: false,
        })))
    }
    fn snapshot(&self) -> Self {
        let result = Self::new();
        result
            .0
            .borrow_mut()
            .bytes
            .clone_from(&self.0.borrow().bytes);
        result
            .0
            .borrow_mut()
            .programmed_once
            .clone_from(&self.0.borrow().programmed_once);
        result
    }
    fn arm(&self, op: usize, prefix: usize) {
        let mut s = self.0.borrow_mut();
        s.operations.clear();
        s.cut = Some((op, prefix));
    }
    fn restore_power(&self) {
        let mut s = self.0.borrow_mut();
        s.off = false;
        s.cut = None;
    }
}
impl Flash for Memory {
    type Error = Fault;
    fn page_count(&self) -> usize {
        PAGES
    }
    async fn read(&mut self, page: PageID, offset: usize, data: &mut [u8]) -> Result<(), Fault> {
        let s = self.0.borrow();
        if s.off {
            return Err(Fault::PowerOff);
        }
        assert!(page.index() < PAGES && offset + data.len() <= PAGE);
        assert!(offset.is_multiple_of(4) && data.len().is_multiple_of(4));
        let from = page.index() * PAGE + offset;
        data.copy_from_slice(&s.bytes[from..from + data.len()]);
        Ok(())
    }
    async fn erase(&mut self, page: PageID) -> Result<(), Fault> {
        let mut s = self.0.borrow_mut();
        if s.off {
            return Err(Fault::PowerOff);
        }
        assert!(page.index() < PAGES);
        s.operations.push(Operation {
            erase: true,
            page: page.index(),
            offset: 0,
            len: PAGE,
        });
        let cut = s.cut.filter(|(op, _)| *op == s.operations.len());
        let count = cut.map_or(PAGE, |(_, prefix)| prefix.min(PAGE));
        let from = page.index() * PAGE;
        s.bytes[from..from + count].fill(255);
        s.programmed_once[from..from + count].fill(false);
        s.erases[page.index()] += 1;
        if cut.is_some() {
            s.off = true;
            Err(Fault::PowerOff)
        } else {
            Ok(())
        }
    }
    async fn write(&mut self, page: PageID, offset: usize, data: &[u8]) -> Result<(), Fault> {
        let mut s = self.0.borrow_mut();
        if s.off {
            return Err(Fault::PowerOff);
        }
        assert!(page.index() < PAGES && offset + data.len() <= PAGE);
        assert!(offset.is_multiple_of(4) && data.len().is_multiple_of(4));
        s.operations.push(Operation {
            erase: false,
            page: page.index(),
            offset,
            len: data.len(),
        });
        let cut = s.cut.filter(|(op, _)| *op == s.operations.len());
        let count = cut.map_or(data.len(), |(_, prefix)| prefix.min(data.len()));
        let from = page.index() * PAGE + offset;
        for (i, byte) in data.iter().take(count).enumerate() {
            assert!(
                !s.programmed_once[from + i],
                "byte programmed twice per erase"
            );
            s.programmed_once[from + i] = true;
            assert_eq!(s.bytes[from + i], 255, "program each byte once per erase");
            s.bytes[from + i] &= *byte;
        }
        s.programmed += count as u64;
        if cut.is_some() {
            s.off = true;
            Err(Fault::PowerOff)
        } else {
            Ok(())
        }
    }
}
fn run<F: Future>(f: F) -> F::Output {
    let mut f = core::pin::pin!(f);
    let mut cx = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::yield_now();
    }
}
fn db(mem: &Memory, seed: u32) -> Db {
    let mut config = ekv::Config::default();
    config.random_seed = seed;
    Database::new(mem.clone(), config)
}
async fn write(db: &Db, key: &[u8], value: Option<&[u8]>) -> Result<(), String> {
    let mut tx = db.write_transaction().await;
    match value {
        Some(value) => tx
            .write(key, value)
            .await
            .map_err(|e| format!("write: {e:?}"))?,
        None => tx.delete(key).await.map_err(|e| format!("delete: {e:?}"))?,
    }
    tx.commit().await.map_err(|e| format!("commit: {e:?}"))
}
async fn read(db: &Db, key: &[u8]) -> Result<Option<Vec<u8>>, String> {
    let tx = db.read_transaction().await;
    let mut value = vec![0; 8192];
    match tx.read(key, &mut value).await {
        Ok(n) => {
            value.truncate(n);
            Ok(Some(value))
        }
        Err(ekv::ReadError::KeyNotFound) => Ok(None),
        Err(e) => Err(format!("read: {e:?}")),
    }
}
fn profile() {
    use ekv::config as c;
    const {
        assert!(
            c::CRC
                && c::ALIGN == 4
                && c::PAGE_SIZE == PAGE
                && c::MAX_PAGE_COUNT == 256
                && c::MAX_KEY_SIZE == 64
                && c::MAX_VALUE_SIZE == 8192
                && c::MAX_CHUNK_SIZE == 256
                && c::ERASE_VALUE == 255
                && c::BRANCHING_FACTOR == 2
                && c::SCRATCH_PAGE_COUNT == 4,
            "qualification must be rerun when the EKV profile changes"
        );
    }
}
fn baseline(updates: usize) -> Result<Memory, String> {
    let mem = Memory::new();
    let db = db(&mem, 1);
    run(db.format()).map_err(|e| format!("format: {e:?}"))?;
    for (key, value) in [
        (b"tls".as_slice(), vec![0x5a; 6154]),
        (b"wifi", vec![4; 128]),
        (b"log", vec![5; 1024]),
        (b"ota", b"previous".to_vec()),
    ] {
        run(write(&db, key, Some(&value)))?;
    }
    for n in 0..updates {
        run(write(&db, b"ota", Some(&vec![(n % 251) as u8; 2048])))?;
    }
    drop(db);
    Ok(mem)
}
fn wear() -> Result<(), String> {
    let mem = baseline(0)?;
    // Exclude formatting and initial cold values from workload counters.
    {
        let mut s = mem.0.borrow_mut();
        s.erases.fill(0);
        s.programmed = 0;
        s.operations.clear();
    }
    let database = db(&mem, 4);
    run(database.mount()).map_err(|e| format!("mount: {e:?}"))?;
    let mut peak_erases = 0;
    let mut peak_programmed = 0;
    let mut peak_operations = 0;
    for n in 0..800 {
        let before = {
            let s = mem.0.borrow();
            (
                s.erases.iter().sum::<u64>(),
                s.programmed,
                s.operations.len(),
            )
        };
        run(write(&database, b"ota", Some(&vec![(n % 251) as u8; 2048])))?;
        let s = mem.0.borrow();
        peak_erases = peak_erases.max(s.erases.iter().sum::<u64>() - before.0);
        peak_programmed = peak_programmed.max(s.programmed - before.1);
        peak_operations = peak_operations.max(s.operations.len() - before.2);
    }
    drop(database);
    let database = db(&mem, 42);
    run(database.mount()).map_err(|e| format!("reboot: {e:?}"))?;
    for (key, value) in [
        (b"tls".as_slice(), vec![0x5a; 6154]),
        (b"wifi", vec![4; 128]),
        (b"log", vec![5; 1024]),
        (b"ota", vec![(799 % 251) as u8; 2048]),
    ] {
        if run(read(&database, key))? != Some(value) {
            return Err("value lost after workload/reboot".into());
        }
    }
    let s = mem.0.borrow();
    let total: u64 = s.erases.iter().sum();
    println!(
        "WEAR updates=800 payload=2048 pages=64 erases={total} programmed={} useful={} amplification={:.6}",
        s.programmed,
        800 * 2048,
        s.programmed as f64 / (800. * 2048.)
    );
    println!(
        "erases per page min={} max={} used={} histogram={:?}",
        s.erases.iter().min().unwrap(),
        s.erases.iter().max().unwrap(),
        s.erases.iter().filter(|n| **n > 0).count(),
        s.erases
    );
    println!(
        "peak per commit erases={peak_erases} programmed={peak_programmed} operations={peak_operations}"
    );
    println!("PASS cold TLS/Wi-Fi/log and latest OTA preserved; no hardware lifetime claim");
    Ok(())
}
fn cuts() -> Result<(), String> {
    let mut cases = 0;
    let mut failures = 0;
    let mut frontier_counts = BTreeMap::new();
    for recycled in [false, true] {
        let baseline = baseline(if recycled { 200 } else { 0 })?;
        for deleting in [false, true] {
            let old_db = db(&baseline, 4);
            run(old_db.mount()).map_err(|e| format!("mount: {e:?}"))?;
            let old = run(read(&old_db, b"ota"))?;
            drop(old_db);
            let new = (!deleting).then(|| vec![9; 2048]);
            let probe = baseline.snapshot();
            let database = db(&probe, 4);
            run(database.mount()).map_err(|e| format!("mount: {e:?}"))?;
            probe.0.borrow_mut().operations.clear();
            run(write(&database, b"ota", new.as_deref()))?;
            let operations = probe.0.borrow().operations.clone();
            drop(database);
            frontier_counts.insert((recycled, deleting), operations.len());
            for (i, operation) in operations.iter().enumerate() {
                let mut prefixes = vec![
                    0,
                    1,
                    2,
                    3,
                    4,
                    7,
                    8,
                    15,
                    31,
                    127,
                    255,
                    256,
                    1024,
                    2048,
                    4095,
                    operation.len - 1,
                    operation.len,
                ];
                prefixes.retain(|n| *n <= operation.len);
                prefixes.sort_unstable();
                prefixes.dedup();
                for prefix in prefixes {
                    cases += 1;
                    let mem = baseline.snapshot();
                    let database = db(&mem, 4);
                    run(database.mount()).map_err(|e| format!("mount: {e:?}"))?;
                    mem.arm(i + 1, prefix);
                    let acknowledged = run(write(&database, b"ota", new.as_deref())).is_ok();
                    drop(database);
                    mem.restore_power();
                    let database = db(&mem, 77);
                    let outcome = (|| {
                        run(database.mount()).map_err(|e| format!("mount: {e:?}"))?;
                        let value = run(read(&database, b"ota"))?;
                        if value != old && value != new {
                            return Err("partial/resurrected value".into());
                        }
                        if acknowledged && value != new {
                            return Err("acknowledged commit lost".into());
                        }
                        for (key, value) in [
                            (b"tls".as_slice(), vec![0x5a; 6154]),
                            (b"wifi", vec![4; 128]),
                            (b"log", vec![5; 1024]),
                        ] {
                            if run(read(&database, key))? != Some(value) {
                                return Err("cold data lost".into());
                            }
                        }
                        run(write(&database, b"ota", Some(b"next")))?;
                        if run(read(&database, b"ota"))? != Some(b"next".to_vec()) {
                            return Err("continuation failed".into());
                        }
                        Ok::<_, String>(())
                    })();
                    if let Err(error) = outcome {
                        failures += 1;
                        if failures <= 12 {
                            eprintln!(
                                "FAIL recycled={recycled} delete={deleting} op={} {operation:?} prefix={prefix}: {error}",
                                i + 1
                            );
                        }
                    }
                }
            }
        }
    }
    println!("CUTS cases={cases} failed={failures} operation_frontiers={frontier_counts:?}");
    if failures == 0 {
        Ok(())
    } else {
        Err(format!(
            "power-fail qualification failed in {failures}/{cases} cases"
        ))
    }
}
fn reproduce() -> Result<(), String> {
    // Minimal engine-only reproducer: format, durable old value, torn replacement.
    let mem = Memory::new();
    let database = db(&mem, 1);
    run(database.format()).map_err(|e| format!("format: {e:?}"))?;
    run(write(&database, b"ota", Some(b"previous")))?;
    drop(database);
    let database = db(&mem, 4);
    run(database.mount()).map_err(|e| format!("mount: {e:?}"))?;
    mem.arm(25, 2);
    let result = run(write(&database, b"ota", Some(&[9; 2048])));
    let operation = mem
        .0
        .borrow()
        .operations
        .last()
        .copied()
        .ok_or("no operation")?;
    println!("torn operation={operation:?} prefix=2 write_result={result:?}");
    if result.is_ok() || operation.erase || operation.offset != 36 || operation.len != 8 {
        return Err("reproducer frontier changed; re-enumerate with cuts".into());
    }
    let from = operation.page * PAGE + operation.offset;
    println!(
        "torn header bytes={:02x?}",
        &mem.0.borrow().bytes[from..from + operation.len]
    );
    drop(database);
    mem.restore_power();
    let database = db(&mem, 77);
    run(database.mount())
        .map_err(|e| format!("reboot: {e:?}; previously acknowledged value is inaccessible"))?;
    if run(read(&database, b"ota"))? != Some(b"previous".to_vec()) {
        return Err("old value not recovered".into());
    }
    Ok(())
}
fn main() -> std::process::ExitCode {
    profile();
    let result = match std::env::args().nth(1).as_deref() {
        Some("wear") => wear(),
        Some("cuts") => cuts(),
        Some("reproduce") => reproduce(),
        _ => Err("usage: qualify_ekv wear|cuts|reproduce".into()),
    };
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("FAIL: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cold_values_survive_hot_updates_and_reboot() {
        profile();
        wear().unwrap();
    }

    #[test]
    fn read_only_reboots_do_not_program_or_erase() {
        let mem = baseline(0).unwrap();
        mem.0.borrow_mut().operations.clear();
        for seed in 0..100 {
            let database = db(&mem, seed);
            run(database.mount()).unwrap();
            assert_eq!(
                run(read(&database, b"tls")).unwrap(),
                Some(vec![0x5a; 6154])
            );
        }
        assert!(mem.0.borrow().operations.is_empty());
    }

    // Maintainer decision: continue development with the known upstream defect.
    // Keep the failing qualification runnable with --ignored; tracked in #70.
    #[test]
    #[ignore = "Known EKV power-fail defect; maintainer-approved deferral: https://github.com/iobewi/rust-mcu-libs/issues/70"]
    fn power_fail_qualification() {
        profile();
        cuts().unwrap();
    }
}
