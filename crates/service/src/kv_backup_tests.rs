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
        ErrorCode::KvStorageFull,
        ErrorCode::KvUnavailable,
        ErrorCode::KvCorrupt,
        ErrorCode::KvBusy,
        ErrorCode::Internal,
    ] {
        assert_eq!(kv_error_code(code.as_str()), code);
    }
    assert_eq!(kv_error_code("UNKNOWN"), ErrorCode::Internal);

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
}
