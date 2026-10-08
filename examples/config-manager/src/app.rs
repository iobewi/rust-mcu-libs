//! Hardware-independent configuration-manager scenario.
use iobewi_config_space::{Budget, ConfigBackend, ConfigManager};

pub async fn run<B: ConfigBackend>(backend: B) {
    let mut manager = ConfigManager::new(backend);
    let space = manager.claim("demo", Budget::new(32))
        .unwrap_or_else(|_| panic!("configuration reservation failed"));

    // Persist on the first boot; check the value on subsequent boots.
    match space.load().await {
        Ok(Some(snapshot)) => {
            assert_eq!(snapshot.data.as_slice(), b"config-ready");
            core::hint::black_box(snapshot.generation);
        }
        Ok(None) => {
            let generation = space.commit(b"config-ready").await
                .unwrap_or_else(|_| panic!("configuration write failed"));
            assert_eq!(generation, 1);
        }
        Err(_) => panic!("configuration read failed"),
    }

    let snapshot = space.load().await
        .unwrap_or_else(|_| panic!("verification read failed"))
        .unwrap_or_else(|| panic!("configuration missing"));
    assert_eq!(snapshot.data.as_slice(), b"config-ready");
}
