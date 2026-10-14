//! Hardware-independent configuration-manager scenario.
use iobewi_config_space::{Budget, ConfigBackend, ConfigManager};

pub async fn run<B: ConfigBackend>(backend: B) {
    let mut manager = ConfigManager::new(backend);
    let space = manager
        .claim("demo", Budget::new(32))
        .unwrap_or_else(|_| panic!("configuration reservation failed"));

    // Persist on the first boot; check the value on subsequent boots.
    match space.load().await {
        Ok(Some(snapshot)) => {
            assert_eq!(snapshot.data.as_slice(), b"config-ready");
            log::info!(target: "example_config_manager", "config: retained generation {}", snapshot.generation);
        }
        Ok(None) => {
            let generation = space
                .commit(b"config-ready")
                .await
                .unwrap_or_else(|_| panic!("configuration write failed"));
            assert!(generation > 0);
            log::info!(target: "example_config_manager", "config: initialized generation {generation}");
        }
        Err(_) => panic!("configuration read failed"),
    }

    let snapshot = space
        .load()
        .await
        .unwrap_or_else(|_| panic!("verification read failed"))
        .unwrap_or_else(|| panic!("configuration missing"));
    assert_eq!(snapshot.data.as_slice(), b"config-ready");
    log::info!(target: "example_config_manager", "config: verified generation {}", snapshot.generation);
    #[cfg(feature = "exercise")]
    {
        let previous = snapshot.generation;
        assert_eq!(
            space
                .commit(b"replacement")
                .await
                .unwrap_or_else(|_| panic!("replacement failed")),
            previous + 1
        );
        assert_eq!(
            space
                .load()
                .await
                .unwrap_or_else(|_| panic!("load failed"))
                .unwrap()
                .data,
            b"replacement"
        );
        assert_eq!(
            space
                .clear()
                .await
                .unwrap_or_else(|_| panic!("clear failed")),
            previous + 2
        );
        assert!(
            space
                .load()
                .await
                .unwrap_or_else(|_| panic!("clear read failed"))
                .is_none()
        );
        assert_eq!(
            space
                .commit(b"config-ready")
                .await
                .unwrap_or_else(|_| panic!("recommit failed")),
            previous + 3
        );
        let last = space
            .load()
            .await
            .unwrap_or_else(|_| panic!("recommit read failed"))
            .unwrap();
        assert_eq!(last.data, b"config-ready");
        assert_eq!(last.generation, previous + 3);
        log::info!(target: "example_config_manager", "config: commit/clear/recommit PASS generation {}", last.generation);
    }
}
