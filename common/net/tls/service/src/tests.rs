//! Host tests: service policy over a fake crypto, and the fail-closed
//! outbound connector over a fake dialer. No MbedTLS, no ESP.

use super::*;
use crate::client::{ClientTlsError, SecureConnector};
use alloc::collections::BTreeMap;
use alloc::rc::Rc;
use alloc::string::ToString;
use core::cell::{Cell, RefCell};
use core::convert::Infallible;
use core::future::Future;
use core::task::{Context, Poll, Waker};
use iobewi_config_space::{ConfigManager, Snapshot};
use iobewi_crypto_core::Identity;
use iobewi_net_io::{Close, Connector, ErrorType, Read, Write};
use iobewi_net_tls_core::TlsDialer;

fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = core::pin::pin!(future);
    let mut cx = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(v) = future.as_mut().poll(&mut cx) {
            return v;
        }
    }
}

#[derive(Default)]
struct Mem {
    values: BTreeMap<String, Snapshot>,
    generation: u64,
    fail_commit: bool,
}

#[derive(Clone, Default)]
struct MemBackend(Rc<RefCell<Mem>>);

impl ConfigBackend for MemBackend {
    type Error = ();
    fn capacity_units(&self) -> usize {
        16384
    }
    fn reservation_units(&self, _: &str, b: Budget) -> Option<usize> {
        Some(b.max_bytes())
    }
    async fn load(&self, space: &str) -> Result<Option<Snapshot>, ()> {
        Ok(self.0.borrow().values.get(space).cloned())
    }
    async fn commit(&self, space: &str, data: &[u8]) -> Result<u64, ()> {
        let mut m = self.0.borrow_mut();
        if m.fail_commit {
            return Err(());
        }
        m.generation += 1;
        let generation = m.generation;
        m.values.insert(
            space.to_string(),
            Snapshot {
                generation,
                data: data.to_vec(),
            },
        );
        Ok(generation)
    }
    async fn clear(&self, space: &str) -> Result<u64, ()> {
        let mut m = self.0.borrow_mut();
        m.generation += 1;
        m.values.remove(space);
        Ok(m.generation)
    }
}

/// Accepts PEM strings of the form `CERT:<id>` / `KEY:<id>` / `CA:<id>`;
/// a pair matches when the ids are equal. Generation can be made to fail.
#[derive(Clone, Copy, Default)]
struct FakeCrypto {
    fail_generation: bool,
}

fn id(prefix: &str, pem: &str) -> Option<String> {
    pem.strip_prefix(prefix).map(|s| s.to_string())
}

impl TlsCrypto for FakeCrypto {
    type ServerConfig = String;

    fn validate_pair(&self, cert: &str, key: &str) -> Result<(), PairError> {
        match (id("CERT:", cert), id("KEY:", key)) {
            (Some(c), Some(k)) if c == k => Ok(()),
            (Some(_), Some(_)) => Err(PairError::Mismatch),
            _ => Err(PairError::Invalid),
        }
    }
    fn server_config(&self, cert: &str, key: &str) -> Option<String> {
        self.validate_pair(cert, key).ok().map(|_| cert.to_string())
    }
    fn generate_identity(&self, common_name: &str) -> Option<Identity> {
        (!self.fail_generation).then(|| Identity {
            cert_pem: alloc::format!("CERT:{common_name}"),
            key_pem: alloc::format!("KEY:{common_name}"),
        })
    }
    fn validate_ca(&self, ca: &str) -> bool {
        ca.starts_with("CA:")
    }
}

fn setup() -> (TlsService<FakeCrypto>, ConfigSpace<MemBackend>, MemBackend) {
    let backend = MemBackend::default();
    let space = ConfigManager::new(backend.clone())
        .claim("tls", CONFIG_BUDGET)
        .unwrap();
    (TlsService::new(FakeCrypto::default()), space, backend)
}

#[test]
fn identity_is_generated_once_when_absent_and_reused_after() {
    let (svc, space, backend) = setup();
    block_on(svc.ensure_server_identity(&space, "node-a")).unwrap();
    let first = backend.0.borrow().values["tls"].data.clone();
    block_on(svc.ensure_server_identity(&space, "node-b")).unwrap();
    assert_eq!(
        backend.0.borrow().values["tls"].data,
        first,
        "an existing identity is never regenerated"
    );
    assert!(block_on(svc.server_identity_valid(&space)));
    assert_eq!(
        block_on(svc.server_config(&space)).as_deref(),
        Some("CERT:node-a")
    );
}

#[test]
fn corrupt_or_invalid_stored_identity_fails_closed_without_regeneration() {
    let (svc, space, backend) = setup();
    backend.0.borrow_mut().values.insert(
        "tls".into(),
        Snapshot {
            generation: 1,
            data: b"junk".to_vec(),
        },
    );
    assert_eq!(
        block_on(svc.ensure_server_identity(&space, "n")),
        Err(IdentityBootstrapError::Corrupt)
    );
    assert!(!block_on(svc.server_identity_valid(&space)));
    assert!(block_on(svc.server_config(&space)).is_none());
    assert_eq!(
        backend.0.borrow().values["tls"].data,
        b"junk",
        "corrupt data is left untouched"
    );

    // A well-formed config whose pair does not match is Invalid, not replaced.
    let bad = TlsConfig {
        ca_pem: String::new(),
        cert_pem: "CERT:a".into(),
        key_pem: "KEY:b".into(),
    };
    backend.0.borrow_mut().values.insert(
        "tls".into(),
        Snapshot {
            generation: 2,
            data: bad.encode().unwrap(),
        },
    );
    assert_eq!(
        block_on(svc.ensure_server_identity(&space, "n")),
        Err(IdentityBootstrapError::Invalid)
    );
}

#[test]
fn generation_failure_and_storage_failure_are_reported_and_nothing_is_stored() {
    let (_, space, backend) = setup();
    let failing = TlsService::new(FakeCrypto {
        fail_generation: true,
    });
    assert_eq!(
        block_on(failing.ensure_server_identity(&space, "n")),
        Err(IdentityBootstrapError::Generation)
    );
    assert!(backend.0.borrow().values.is_empty());

    let svc = TlsService::new(FakeCrypto::default());
    backend.0.borrow_mut().fail_commit = true;
    assert_eq!(
        block_on(svc.ensure_server_identity(&space, "n")),
        Err(IdentityBootstrapError::Storage)
    );
}

#[test]
fn save_cert_validates_before_storing_and_keeps_the_ca() {
    let (svc, space, backend) = setup();
    assert_eq!(
        block_on(svc.save_cert(&space, "CERT:a", "KEY:b")),
        Err(SaveCertError::Mismatch)
    );
    assert_eq!(
        block_on(svc.save_cert(&space, "nope", "KEY:b")),
        Err(SaveCertError::Invalid)
    );
    assert!(
        backend.0.borrow().values.is_empty(),
        "rejected material is never stored"
    );

    block_on(svc.save_ca(&space, "CA:root")).unwrap();
    block_on(svc.save_cert(&space, "CERT:a", "KEY:a")).unwrap();
    assert_eq!(block_on(svc.trusted_ca(&space)).as_deref(), Some("CA:root"));
    assert_eq!(
        block_on(svc.server_config(&space)).as_deref(),
        Some("CERT:a")
    );
}

#[test]
fn save_ca_rejects_invalid_ca_and_preserves_the_identity() {
    let (svc, space, _b) = setup();
    block_on(svc.save_cert(&space, "CERT:a", "KEY:a")).unwrap();
    assert_eq!(
        block_on(svc.save_ca(&space, "garbage")),
        Err(SaveCertError::Invalid)
    );
    assert!(block_on(svc.trusted_ca(&space)).is_none());
    block_on(svc.save_ca(&space, "CA:root")).unwrap();
    assert!(block_on(svc.server_identity_valid(&space)));
}

#[test]
fn oversized_material_is_a_storage_error() {
    let (svc, space, _b) = setup();
    let cert = alloc::format!("CERT:{}", "x".repeat(MAX_CERT_LEN));
    let key = alloc::format!("KEY:{}", "x".repeat(MAX_CERT_LEN));
    assert_eq!(
        block_on(svc.save_cert(&space, &cert, &key)),
        Err(SaveCertError::Storage)
    );
}

// ---- secure outbound connector --------------------------------------------

struct Stream;
impl ErrorType for Stream {
    type Error = Infallible;
}
impl Read for Stream {
    async fn read(&mut self, _: &mut [u8]) -> Result<usize, Infallible> {
        Ok(0)
    }
}
impl Write for Stream {
    async fn write(&mut self, b: &[u8]) -> Result<usize, Infallible> {
        Ok(b.len())
    }
    async fn flush(&mut self) -> Result<(), Infallible> {
        Ok(())
    }
}
impl Close for Stream {
    async fn close(&mut self) -> Result<(), Infallible> {
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct FakeDialer<'d> {
    calls: &'d Cell<u32>,
    fail: bool,
}

#[derive(Debug)]
struct DialError;
impl Display for DialError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "TLS handshake failed: bad certificate")
    }
}

use core::fmt::Display;
std::thread_local! { static LAST_CA: RefCell<String> = const { RefCell::new(String::new()) }; }

impl TlsDialer for FakeDialer<'_> {
    type Error = DialError;
    type Connection<'a>
        = Stream
    where
        Self: 'a;
    async fn dial<'a>(
        &'a self,
        _h: &'a str,
        _p: u16,
        ca: &str,
        _rx: &'a mut [u8],
        _tx: &'a mut [u8],
    ) -> Result<Stream, DialError> {
        self.calls.set(self.calls.get() + 1);
        LAST_CA.with(|c| *c.borrow_mut() = ca.to_string());
        if self.fail {
            Err(DialError)
        } else {
            Ok(Stream)
        }
    }
}

fn leak_space(backend: &MemBackend) -> &'static ConfigSpace<MemBackend> {
    alloc::boxed::Box::leak(alloc::boxed::Box::new(
        ConfigManager::new(backend.clone())
            .claim("tls", CONFIG_BUDGET)
            .unwrap(),
    ))
}

static CLOCK_SET: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
fn clock_is_set() -> bool {
    CLOCK_SET.load(core::sync::atomic::Ordering::SeqCst)
}
fn clock_always_set() -> bool {
    true
}

#[test]
fn connector_refuses_before_dialing_without_clock_or_ca_then_dials_with_the_ca() {
    let (svc, _space, backend) = setup();
    let space = leak_space(&backend);
    let calls = Cell::new(0);
    let transport = SecureConnector {
        dialer: FakeDialer {
            calls: &calls,
            fail: false,
        },
        tls_config: space,
        clock_is_set,
    };
    let (mut rx, mut tx) = ([0u8; 4], [0u8; 4]);

    // 1. Clock not set: refused first, even though a CA exists.
    block_on(svc.save_ca(space, "CA:root")).unwrap();
    CLOCK_SET.store(false, core::sync::atomic::Ordering::SeqCst);
    let err = block_on(transport.connect("core.example", 8443, &mut rx, &mut tx))
        .err()
        .unwrap();
    assert!(matches!(err, ClientTlsError::ClockUnsynced));
    assert_eq!(err.to_string(), "clock not synchronized yet (SNTP)");
    assert_eq!(calls.get(), 0, "no dial before the clock is set");

    // 2. Clock set but no CA: refused.
    CLOCK_SET.store(true, core::sync::atomic::Ordering::SeqCst);
    backend.0.borrow_mut().values.clear();
    let err = block_on(transport.connect("core.example", 8443, &mut rx, &mut tx))
        .err()
        .unwrap();
    assert!(matches!(err, ClientTlsError::NoCa));
    assert_eq!(err.to_string(), "no CA configured (POST /v1alpha1/tls/ca)");
    assert_eq!(calls.get(), 0, "no dial without a CA");

    // 3. Both present: the stored CA is what the dialer verifies against.
    block_on(svc.save_ca(space, "CA:root")).unwrap();
    assert!(block_on(transport.connect("core.example", 8443, &mut rx, &mut tx)).is_ok());
    assert_eq!(calls.get(), 1);
    assert_eq!(LAST_CA.with(|c| c.borrow().clone()), "CA:root");
}

#[test]
fn dial_failure_is_reported_verbatim_not_downgraded() {
    let (svc, _space, backend) = setup();
    let space = leak_space(&backend);
    block_on(svc.save_ca(space, "CA:root")).unwrap();
    let calls = Cell::new(0);
    let transport = SecureConnector {
        dialer: FakeDialer {
            calls: &calls,
            fail: true,
        },
        tls_config: space,
        clock_is_set: clock_always_set,
    };
    let (mut rx, mut tx) = ([0u8; 4], [0u8; 4]);
    let err = block_on(transport.connect("core.example", 8443, &mut rx, &mut tx))
        .err()
        .unwrap();
    assert!(matches!(err, ClientTlsError::Connect(_)));
    assert_eq!(err.to_string(), "TLS handshake failed: bad certificate");
}
