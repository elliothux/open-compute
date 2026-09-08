use super::*;

#[test]
fn backup_error_mapping_and_file_hashing_are_stable() {
    for code in [
        ErrorCode::ResourceNotFound,
        ErrorCode::ResourceNotReady,
        ErrorCode::ResourceInvariantViolation,
        ErrorCode::IdempotencyConflict,
        ErrorCode::ArtifactUnavailable,
        ErrorCode::ArtifactIntegrityError,
        ErrorCode::ObjectStorageUnavailable,
        ErrorCode::DiskHardLimit,
        ErrorCode::LimitInvalid,
        ErrorCode::D1DatabaseFull,
        ErrorCode::D1DatabaseCorrupt,
        ErrorCode::D1IdentityMismatch,
        ErrorCode::D1MigrationDrift,
        ErrorCode::Internal,
    ] {
        assert_eq!(d1_error_code(code.as_str()), code);
    }
    assert_eq!(d1_error_code("UNKNOWN"), ErrorCode::Internal);

    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("data.sqlite");
    std::fs::write(&path, b"abc").unwrap();
    let (digest, size) = hash_file(&path).unwrap();
    assert_eq!(
        hex::encode(digest),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(size, 3);
    assert_eq!(
        hash_file(&temp.path().join("missing")).unwrap_err().code(),
        ErrorCode::Internal
    );
    assert_eq!(internal().code(), ErrorCode::Internal);
}

#[test]
fn replayed_backup_failure_maps_stored_error_code() {
    let backup = open_compute_storage::D1BackupRecord {
        id: "01aaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
        source_resource_id: ResourceId::generate(),
        state: D1BackupState::Failed,
        object_key: None,
        sha256: None,
        size_bytes: None,
        d1_schema_version: 1,
        sqlite_user_version: 1,
        created_at_ms: 1,
        completed_at_ms: Some(2),
        error_code: Some(ErrorCode::D1DatabaseCorrupt.as_str().to_owned()),
    };
    let err = replayed_backup_failure(&backup);
    assert_eq!(err.code(), ErrorCode::D1DatabaseCorrupt);
    let backup = open_compute_storage::D1BackupRecord {
        error_code: None,
        ..backup
    };
    assert_eq!(replayed_backup_failure(&backup).code(), ErrorCode::Internal);
}
