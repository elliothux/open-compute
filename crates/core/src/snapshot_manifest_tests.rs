use super::*;
use crate::SnapshotTotalsV1;

fn release() -> PlatformReleaseIdentityV1 {
    PlatformReleaseIdentityV1 {
        schema_version: 1,
        platform_version: "0.1.0".to_owned(),
        git_revision: "test".to_owned(),
        rust_msrv: "1.98.0".to_owned(),
        workerd_version: "workerd test".to_owned(),
        workerd_lock_sha256: "1".repeat(64),
        runtime_assets_sha256: "2".repeat(64),
        dashboard_assets_sha256: "3".repeat(64),
        facade_capability_version: 1,
        control_schema_version: 8,
        scheduler_schema_version: 1,
        kv_schema_version: 1,
        d1_schema_version: 1,
        vectorize_schema_version: 1,
        ai_search_schema_version: 1,
        snapshot_format_version: 1,
    }
}

fn manifest() -> PlatformSnapshotManifestV1 {
    PlatformSnapshotManifestV1 {
        schema_version: 1,
        snapshot_id: uuid::Uuid::now_v7().hyphenated().to_string(),
        platform_id: PlatformId::generate().to_string(),
        label: "nightly".to_owned(),
        created_at_ms: 1,
        source_release: release(),
        source_schemas: BTreeMap::from([
            ("control".to_owned(), 8),
            ("d1".to_owned(), 1),
            ("kv".to_owned(), 1),
            ("scheduler".to_owned(), 1),
            ("vectorize".to_owned(), 1),
            ("ai_search".to_owned(), 1),
        ]),
        master_key_fingerprint: "4".repeat(64),
        s3_authority_fingerprint: "5".repeat(64),
        r2_prefix_fingerprint: "6".repeat(64),
        config_policy_sha256: "b".repeat(64),
        excluded_local_state: vec![
            "ann_cache".to_owned(),
            "images_sessions".to_owned(),
            "response_cache".to_owned(),
            "runtime_cache".to_owned(),
            "vector_search_cache".to_owned(),
        ],
        immutable_references: vec![SnapshotImmutableReferenceV1 {
            role: "version_artifact".to_owned(),
            sha256: "7".repeat(64),
            object_key: "system/artifacts/v1/sha256/x".to_owned(),
            size: 1,
        }],
        files: vec![
            SnapshotFileV1 {
                role: SnapshotFileRole::ControlSqlite,
                logical_id: "control".to_owned(),
                restore_path: "control.sqlite".to_owned(),
                object_key: "system/snapshots/v1/p/s/objects/000000.bin".to_owned(),
                size: 10,
                sha256: "8".repeat(64),
                mode: 0o600,
            },
            SnapshotFileV1 {
                role: SnapshotFileRole::SchedulerSqlite,
                logical_id: "scheduler".to_owned(),
                restore_path: "scheduler.sqlite".to_owned(),
                object_key: "system/snapshots/v1/p/s/objects/000001.bin".to_owned(),
                size: 10,
                sha256: "a".repeat(64),
                mode: 0o600,
            },
        ],
        totals: SnapshotTotalsV1 {
            files: 2,
            bytes: 20,
        },
        manifest_mac: "9".repeat(64),
    }
}

#[test]
fn manifest_paths_caps_and_uniqueness_are_strict() {
    let value = manifest();
    value.validate(10, 100, 100).unwrap();
    assert!(!value.canonical_unsigned_bytes().unwrap().is_empty());
    for path in [
        "",
        "/control.sqlite",
        "../control.sqlite",
        "kv//x",
        "logs/x",
        "do\\x",
    ] {
        assert!(!valid_restore_path(path), "accepted {path}");
    }
    for path in [
        "control.sqlite",
        "scheduler.sqlite",
        "kv/a/b/data.sqlite",
        "d1/a/b/data.sqlite",
        "vectorize/a/b/data.sqlite",
        "ai-search/a/b/data.sqlite",
        "do/workerd/x",
    ] {
        assert!(valid_restore_path(path), "rejected {path}");
    }
    let mut duplicate = value.clone();
    duplicate
        .immutable_references
        .push(duplicate.immutable_references[0].clone());
    assert_eq!(
        duplicate.validate(10, 100, 100).unwrap_err().code(),
        ErrorCode::SnapshotInvalid
    );
    for owner in ["control", "scheduler", "kv", "d1", "vectorize", "ai_search"] {
        let mut bad_schema = value.clone();
        *bad_schema.source_schemas.get_mut(owner).unwrap() += 1;
        assert!(bad_schema.validate(10, 100, 100).is_err(), "{owner}");
    }
    let mut omitted_exclusion = value.clone();
    omitted_exclusion.excluded_local_state.pop();
    assert!(omitted_exclusion.validate(10, 100, 100).is_err());
    let mut traversal = value;
    traversal.files[0].restore_path = "do/../control.sqlite".to_owned();
    assert!(traversal.validate(10, 100, 100).is_err());
}
