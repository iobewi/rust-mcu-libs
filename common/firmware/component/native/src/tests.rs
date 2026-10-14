use super::*;
use crate::runtime::{ArtifactReader, Health, Identity, RuntimeError, WorkloadRuntime};
use crate::testing::{Behaviour, FakeNative, image};
use alloc::string::String;
use iobewi_firmware_update::compat::otm2::Side;
fn run<F: core::future::Future>(f: F) -> F::Output {
    let mut f = core::pin::pin!(f);
    let mut cx = core::task::Context::from_waker(core::task::Waker::noop());
    loop {
        if let core::task::Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
    }
}
struct Reader(std::vec::Vec<u8>);
impl ArtifactReader for Reader {
    async fn read(&self, at: u64, out: &mut [u8]) -> Result<(), RuntimeError> {
        let bytes = self
            .0
            .get(at as usize..at as usize + out.len())
            .ok_or(RuntimeError { reason: "bounds" })?;
        out.copy_from_slice(bytes);
        Ok(())
    }
}
fn artifact(b: Behaviour) -> (Identity, Reader) {
    let bytes = image(b, 3, RuntimeApi::new(1, 0));
    (
        Identity {
            slot: Side::A,
            id: String::from("hello"),
            version: String::from("1"),
            digest: [0; 32],
            size: bytes.len() as u32,
            requires: RuntimeApi::new(1, 0),
        },
        Reader(bytes),
    )
}
#[test]
fn real_runtime_gates_then_loads_and_cooperatively_stops() {
    run(async {
        let rt = NativeRuntime::new(FakeNative::new());
        let (a, r) = artifact(Behaviour::Cooperative);
        rt.preflight(&a, &r).await.unwrap();
        assert!(rt.running().await.is_none());
        rt.start(&a, &r).await.unwrap();
        assert_eq!(rt.running().await, Some(a));
        assert_eq!(rt.health().await, Health::Healthy);
        rt.stop().await;
        assert!(rt.running().await.is_none());
        assert_eq!(rt.last_stop(), StopOutcome::Cooperative);
    })
}
#[test]
fn invalid_header_never_replaces_a_running_component() {
    run(async {
        let rt = NativeRuntime::new(FakeNative::new());
        let (a, r) = artifact(Behaviour::Cooperative);
        rt.start(&a, &r).await.unwrap();
        let (mut bad, mut reader) = artifact(Behaviour::Cooperative);
        reader.0[0] = 0;
        bad.id = String::from("bad");
        assert!(rt.start(&bad, &reader).await.is_err());
        assert_eq!(rt.running().await, Some(a));
    })
}
#[test]
fn ignored_stop_is_forced_and_start_timeout_halts() {
    run(async {
        let rt = NativeRuntime::new(FakeNative::new());
        let (a, r) = artifact(Behaviour::IgnoresStop);
        rt.start(&a, &r).await.unwrap();
        rt.stop().await;
        assert_eq!(rt.last_stop(), StopOutcome::Forced);
        assert!(rt.running().await.is_none());
        let (a, r) = artifact(Behaviour::NeverRuns);
        assert!(rt.start(&a, &r).await.is_err());
        assert!(rt.running().await.is_none());
    })
}
#[test]
fn incompatible_metadata_and_truncated_artifact_are_refused() {
    run(async {
        let rt = NativeRuntime::new(FakeNative::new());
        let (mut a, r) = artifact(Behaviour::Cooperative);
        a.requires = RuntimeApi::new(9, 0);
        assert!(rt.preflight(&a, &r).await.is_err());
        a.size = 1;
        assert!(rt.start(&a, &r).await.is_err());
        assert!(rt.running().await.is_none());
    })
}

#[test]
fn stalled_progress_becomes_unhealthy_and_is_quarantined() {
    run(async {
        let rt = NativeRuntime::new(FakeNative::new());
        let (a, r) = artifact(Behaviour::Freezes);
        rt.start(&a, &r).await.unwrap();
        for _ in 0..5 {
            rt.backend().advance(250);
            rt.sample();
        }
        rt.backend().advance(4_000);
        assert_eq!(rt.health().await, Health::Unhealthy);
        rt.backend().advance(11_000);
        rt.sample();
        assert!(rt.running().await.is_none());
        assert!(!rt.backend().executing());
    });
}
