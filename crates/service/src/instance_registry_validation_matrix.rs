use super::*;
use crate::instance_registry::ServiceScope;

#[test]
fn instance_registration_validates_scope_paths_and_conflicts() {
    let dir = TempDir::new().unwrap();
    let config = write_config(dir.path(), "");
    let loaded = load_platform_config_from(&config, Path::new("/")).unwrap();
    drop(
        open_compute_storage::PlatformStorage::bootstrap(
            &loaded.config.data,
            &open_compute_core::SystemClock,
        )
        .unwrap(),
    );
    let registry = InstanceRegistry::with_roots(
        dir.path().join("system-registry"),
        dir.path().join("user-registry"),
    );
    let now = SystemTime::UNIX_EPOCH;

    // Relative config paths are rejected before any I/O.
    assert_eq!(
        registry
            .register(Path::new("relative.toml"), ServiceScope::User, now)
            .unwrap_err()
            .code(),
        ErrorCode::ConfigPathInvalid
    );

    // A fresh registration succeeds; duplicate registration is rejected.
    registry.register(&config, ServiceScope::User, now).unwrap();
    assert_eq!(
        registry
            .register(&config, ServiceScope::User, now)
            .unwrap_err()
            .code(),
        ErrorCode::InstanceRegistryInvalid
    );

    // Registration with a config that no longer matches its recorded bytes fails validation.
    fs::write(&config, "# mutated").unwrap();
    assert!(registry.register(&config, ServiceScope::User, now).is_err());
}
