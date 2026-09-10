use super::*;

#[test]
fn inspection_layout_migration_and_repository_helpers_are_covered() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    let data = storage.data_dir();
    assert_eq!(data.root(), root);
    assert_eq!(data.control_db_path(), root.join("control.sqlite"));
    assert_eq!(data.keys_dir(), root.join("keys"));
    assert_eq!(data.runtime_dir(), root.join("runtime"));
    assert_eq!(data.artifact_cache_dir(), root.join("cache/artifacts"));
    assert_eq!(data.lock().path(), config.data_lock_path());
    assert_ne!(data.lock().startup_id().to_string(), "");
    assert_eq!(
        data.filesystem_durability(),
        data.lock().filesystem_durability()
    );

    let busy = crate::inspect_data_root(&config).unwrap();
    assert!(!busy.lock_available);
    assert!(!busy.holds_inspect_lock());
    drop(storage);
    let available = crate::inspect_data_root(&config).unwrap();
    assert!(available.lock_available);
    assert!(available.holds_inspect_lock());
    assert_eq!(available.root, root);
    drop(available);

    let mut relative = config.clone();
    relative.path = PathBuf::from("relative");
    assert_eq!(
        crate::inspect_data_root(&relative).unwrap_err().code(),
        ErrorCode::PathInvalid
    );
    let (_missing_tmp, missing_root) = unique_root();
    assert!(crate::inspect_data_root(&storage_config(&missing_root)).is_err());
    assert_eq!(
        crate::inspect_control_db(Path::new("relative.sqlite"), 100)
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
    assert_eq!(
        crate::inspect_control_db(&root.join("missing.sqlite"), 100)
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );

    assert_eq!(crate::migrations::current_schema_version(), 18);
    let migration_registry = crate::migrations::migration_registry();
    assert_eq!(migration_registry.len(), 18);
    assert!(
        migration_registry
            .iter()
            .all(|(_, _, checksum)| checksum.len() == 32)
    );
    assert!(crate::migrations::expected_checksum(1).is_ok());
    assert!(crate::migrations::expected_checksum(2).is_ok());
    assert!(crate::migrations::expected_checksum(3).is_ok());
    assert!(crate::migrations::expected_checksum(4).is_ok());
    assert!(crate::migrations::expected_checksum(5).is_ok());
    assert!(crate::migrations::expected_checksum(6).is_ok());
    assert!(crate::migrations::expected_checksum(7).is_ok());
    assert!(crate::migrations::expected_checksum(8).is_ok());
    assert_eq!(
        crate::migrations::expected_checksum(crate::migrations::current_schema_version() + 1)
            .unwrap_err()
            .code(),
        ErrorCode::SchemaTooNew
    );
    assert_eq!(
        crate::migrations::expected_checksum(0).unwrap_err().code(),
        ErrorCode::MigrationFailed
    );
    let uri = crate::control_db::sqlite_readonly_uri(Path::new("/tmp/a b?#%.sqlite"));
    assert_eq!(uri, "file:/tmp/a%20b%3F%23%25.sqlite?mode=ro&immutable=1");
    assert!(crate::ControlDb::open(Path::new("/"), 100).is_err());

    use crate::workers::{
        array32, db_error, idempotency_ref_id, invariant, route_not_found, validate_exact_route,
        validate_referrer, validate_worker_name, version_not_found, worker_not_found,
    };
    for state in [
        VersionState::Staging,
        VersionState::Validating,
        VersionState::Ready,
        VersionState::Rejected,
        VersionState::Deleting,
        VersionState::Tombstoned,
    ] {
        assert_eq!(VersionState::parse(state.as_str()).unwrap(), state);
    }
    assert!(VersionState::parse("bad").is_err());
    assert_eq!(
        crate::RouteKind::parse("platform_path").unwrap(),
        crate::RouteKind::PlatformPath
    );
    assert_eq!(
        crate::RouteKind::parse("exact_host").unwrap(),
        crate::RouteKind::ExactHost
    );
    assert!(crate::RouteKind::parse("bad").is_err());
    for valid in ["a", "worker-1"] {
        validate_worker_name(valid).unwrap();
    }
    for invalid in ["", "-bad", "bad-", "Upper", &"a".repeat(64)] {
        assert!(validate_worker_name(invalid).is_err());
    }
    validate_referrer("route", "host/path:one").unwrap();
    assert!(validate_referrer("", "id").is_err());
    assert!(validate_referrer("kind", "bad value").is_err());
    validate_exact_route("example.com", "/path", Some("handler_1$")).unwrap();
    for (host, path, entrypoint) in [
        ("", "/", None),
        ("UPPER.example", "/", None),
        ("example.com", "relative", None),
        ("example.com", "/bad?query", None),
        ("example.com", "/", Some("bad-name")),
    ] {
        assert!(validate_exact_route(host, path, entrypoint).is_err());
    }
    let account = AccountId::generate();
    assert_eq!(idempotency_ref_id(account, "scope", "key").len(), 64);
    assert!(array32(&[0_u8; 32]).is_ok());
    assert!(array32(&[0_u8; 31]).is_err());
    assert_eq!(worker_not_found().code(), ErrorCode::WorkerNotFound);
    assert_eq!(version_not_found().code(), ErrorCode::VersionNotFound);
    assert_eq!(route_not_found().code(), ErrorCode::RouteNotFound);
    assert_eq!(invariant().code(), ErrorCode::VersionInvariantViolation);
    assert_eq!(db_error().code(), ErrorCode::Internal);
}
