use super::*;
use crate::instance_registry::ServiceScope;

#[test]
fn instance_registration_validates_scope_paths_and_conflicts() {
    let dir = TempDir::new().unwrap();
    let config = write_config(dir.path(), "");
    let registry = InstanceRegistry::with_roots(
        dir.path().join("system-registry"),
        dir.path().join("user-registry"),
    );
    let binary = std::env::current_exe().unwrap();
    let now = SystemTime::UNIX_EPOCH;

    // Relative config paths and relative executables are rejected before any I/O.
    assert_eq!(
        registry
            .register_owned(
                Path::new("relative.toml"),
                &binary,
                ServiceScope::User,
                None,
                now
            )
            .unwrap_err()
            .code(),
        ErrorCode::ConfigPathInvalid
    );
    assert_eq!(
        registry
            .register_owned(&config, Path::new("ocd"), ServiceScope::User, None, now)
            .unwrap_err()
            .code(),
        ErrorCode::InstanceRegistryInvalid
    );

    // System scope requires an explicit non-root service account.
    assert_eq!(
        registry
            .register_owned(&config, &binary, ServiceScope::System, None, now)
            .unwrap_err()
            .code(),
        ErrorCode::InstanceRegistryInvalid
    );
    assert_eq!(
        registry
            .register_owned(&config, &binary, ServiceScope::System, Some("root"), now)
            .unwrap_err()
            .code(),
        ErrorCode::InstanceRegistryInvalid
    );
    // User scope must not carry a service account.
    assert_eq!(
        registry
            .register_owned(&config, &binary, ServiceScope::User, Some("someone"), now)
            .unwrap_err()
            .code(),
        ErrorCode::InstanceRegistryInvalid
    );

    // A fresh registration succeeds and is idempotent for the same identity.
    let record = registry
        .register_owned(&config, &binary, ServiceScope::User, None, now)
        .unwrap();
    let replay = registry
        .register_owned(&config, &binary, ServiceScope::User, None, now)
        .unwrap();
    assert_eq!(record, replay);

    // The same config registered to a different executable is a conflict.
    let other_binary = dir.path().join("other-ocd");
    fs::write(&other_binary, b"#!/bin/sh\n").unwrap();
    assert_eq!(
        registry
            .register_owned(&config, &other_binary, ServiceScope::User, None, now)
            .unwrap_err()
            .code(),
        ErrorCode::InstanceRegistryInvalid
    );

    // Registration with a config that no longer matches its recorded bytes fails validation.
    fs::write(&config, "# mutated").unwrap();
    assert!(
        registry
            .register_owned(&config, &binary, ServiceScope::User, None, now)
            .is_err()
    );
}
