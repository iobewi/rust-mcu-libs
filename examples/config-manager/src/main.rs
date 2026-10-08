//! Host-only example of ConfigManager composition.
//! The in-memory backend is intentionally NOT durable storage.

use iobewi_config_space::{Budget, ConfigBackend, ConfigManager, Snapshot};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::future::Future;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

#[derive(Clone, Default)]
struct MemoryBackend {
    values: Rc<RefCell<BTreeMap<String, Snapshot>>>,
}

impl ConfigBackend for MemoryBackend {
    type Error = std::convert::Infallible;

    fn capacity_units(&self) -> usize {
        256
    }

    fn reservation_units(&self, _space: &str, budget: Budget) -> Option<usize> {
        Some(budget.max_bytes())
    }

    async fn load(&self, space: &str) -> Result<Option<Snapshot>, Self::Error> {
        Ok(self.values.borrow().get(space).cloned())
    }

    async fn commit(&self, space: &str, data: &[u8]) -> Result<u64, Self::Error> {
        let mut values = self.values.borrow_mut();
        let generation = values.get(space).map_or(1, |old| old.generation + 1);
        values.insert(
            space.to_owned(),
            Snapshot {
                generation,
                data: data.to_vec(),
            },
        );
        Ok(generation)
    }

    async fn clear(&self, space: &str) -> Result<u64, Self::Error> {
        let mut values = self.values.borrow_mut();
        let generation = values.get(space).map_or(1, |old| old.generation + 1);
        values.remove(space);
        Ok(generation)
    }
}

// This tiny runner is valid ONLY for this example's immediately-ready
// in-memory futures. It is not a general-purpose async executor.
fn run_ready<F: Future>(future: F) -> F::Output {
    let waker = Waker::noop();
    let mut context = Context::from_waker(waker);
    let mut future = std::pin::pin!(future);
    match future.as_mut().poll(&mut context) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("in-memory operation unexpectedly pending"),
    }
}

fn main() {
    let mut manager = ConfigManager::new(MemoryBackend::default());
    let wifi = manager.claim("wifi", Budget::new(128)).unwrap();
    let device = manager.claim("device", Budget::new(64)).unwrap();

    assert_eq!(manager.claim_count(), 2);
    assert_eq!(manager.remaining_units(), 64);

    let generation = run_ready(wifi.commit(b"my-ssid")).unwrap();
    assert_eq!(generation, 1);
    let snapshot = run_ready(wifi.load()).unwrap().unwrap();
    assert_eq!(snapshot.data, b"my-ssid");
    assert_eq!(snapshot.generation, 1);

    run_ready(device.commit(b"sensor-01")).unwrap();
    assert!(run_ready(wifi.commit(&[0; 129])).is_err());

    println!(
        "Two isolated configuration spaces; wifi generation={generation}, remaining={} bytes",
        manager.remaining_units()
    );
}
