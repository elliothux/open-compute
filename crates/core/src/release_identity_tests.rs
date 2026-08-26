use super::*;

fn release() -> PlatformReleaseIdentityV1 {
    PlatformReleaseIdentityV1 {
        schema_version: 1,
        platform_version: "0.1.0".to_owned(),
        git_revision: "test".to_owned(),
        rust_msrv: "1.98.0".to_owned(),
        workerd_version: "workerd test".to_owned(),
        workerd_lock_sha256: "a".repeat(64),
        runtime_assets_sha256: "b".repeat(64),
        facade_capability_version: 1,
        control_schema_version: 2,
        scheduler_schema_version: 1,
        kv_schema_version_min: 1,
        kv_schema_version_max: 1,
        d1_schema_version_min: 1,
        d1_schema_version_max: 1,
        snapshot_format_version: 1,
        compatibility_policy_sha256: "c".repeat(64),
    }
}

#[test]
fn release_identity_and_metadata_validate_complete_registries() {
    assert!(release().validate());
    let mut metadata = PlatformReleaseMetadataV1 {
        schema_version: 1,
        release: release(),
        upgrade_from_control_schema_min: 1,
        upgrade_from_platform_versions: vec!["0.1.0".to_owned()],
        restore_compatible_platform_versions: vec!["0.1.0".to_owned()],
        target_schemas: BTreeMap::from([
            ("control".to_owned(), 2),
            ("scheduler".to_owned(), 1),
            ("kv".to_owned(), 1),
            ("d1".to_owned(), 1),
        ]),
        migrations: vec![
            ReleaseMigrationV1 {
                version: 1,
                name: "one".to_owned(),
                sha256: "d".repeat(64),
            },
            ReleaseMigrationV1 {
                version: 2,
                name: "two".to_owned(),
                sha256: "e".repeat(64),
            },
        ],
        readable_object_formats: BTreeMap::from([("snapshots".to_owned(), vec![1])]),
        workerd_local_disk_gate_result: "gate".to_owned(),
        conformance_result: "conformance".to_owned(),
        websocket_hibernation_result: "no-go".to_owned(),
    };
    assert!(metadata.validate());
    metadata.migrations[1].version = 3;
    assert!(!metadata.validate());
}
