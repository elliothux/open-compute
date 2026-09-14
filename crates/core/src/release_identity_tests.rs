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
        dashboard_assets_sha256: "c".repeat(64),
        facade_capability_version: 1,
        snapshot_format_version: 1,
    }
}

#[test]
fn release_identity_and_metadata_validate_complete_registries() {
    assert!(release().validate());
    let mut metadata = PlatformReleaseMetadataV1 {
        schema_version: 1,
        release: release(),
        object_formats: [
            "ai_search_objects",
            "artifacts",
            "d1_backups",
            "kv_backups",
            "r2",
            "snapshots",
        ]
        .map(|name| (name.to_owned(), 1))
        .into(),
        workerd_local_disk_gate_result: "gate".to_owned(),
        conformance_result: "conformance".to_owned(),
        websocket_hibernation_result: "no-go".to_owned(),
    };
    assert!(metadata.validate());
    let encoded = serde_json::to_value(&metadata).unwrap();
    let decoded: PlatformReleaseMetadataV1 = serde_json::from_value(encoded.clone()).unwrap();
    assert_eq!(decoded, metadata);
    let mut missing_owner = metadata.clone();
    missing_owner.object_formats.remove("r2");
    assert!(!missing_owner.validate());
    let mut wrong_snapshot = metadata.clone();
    wrong_snapshot
        .object_formats
        .insert("snapshots".to_owned(), 2);
    assert!(!wrong_snapshot.validate());
    for field in [
        "upgrade_from_control_schema_min",
        "upgrade_from_platform_versions",
        "restore_compatible_platform_versions",
        "readable_object_formats",
        "target_schemas",
        "schema_definitions",
    ] {
        let mut obsolete = encoded.clone();
        obsolete
            .as_object_mut()
            .unwrap()
            .insert(field.to_owned(), serde_json::json!(1));
        assert!(serde_json::from_value::<PlatformReleaseMetadataV1>(obsolete).is_err());
    }
    for field in [
        "kv_schema_version_min",
        "kv_schema_version_max",
        "d1_schema_version_min",
        "d1_schema_version_max",
    ] {
        let mut obsolete = serde_json::to_value(release()).unwrap();
        obsolete
            .as_object_mut()
            .unwrap()
            .insert(field.to_owned(), serde_json::json!(1));
        assert!(serde_json::from_value::<PlatformReleaseIdentityV1>(obsolete).is_err());
    }
    metadata.workerd_local_disk_gate_result.clear();
    assert!(!metadata.validate());
}
