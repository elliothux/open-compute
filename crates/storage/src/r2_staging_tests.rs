use super::R2Staging;
use open_compute_core::{ErrorCode, ResourceId};

#[test]
fn creates_exclusive_canonical_files_and_cleans_stale_uploads() {
    let data = tempfile::tempdir().unwrap();
    let staging = R2Staging::open(data.path()).unwrap();
    let resource = ResourceId::generate();
    let request = uuid::Uuid::now_v7().hyphenated().to_string();
    let (path, file) = staging.create(resource, &request).unwrap();
    assert_eq!(file.metadata().unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(path.file_name().unwrap(), request.as_str());
    assert_eq!(
        staging.create(resource, &request).unwrap_err().code(),
        ErrorCode::PathInvalid
    );
    drop(file);
    assert_eq!(staging.cleanup().unwrap(), 1);
    assert!(!path.exists());
}

#[test]
fn rejects_noncanonical_request_and_unknown_startup_entries() {
    let data = tempfile::tempdir().unwrap();
    let staging = R2Staging::open(data.path()).unwrap();
    assert_eq!(
        staging
            .create(ResourceId::generate(), "not-a-uuid")
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
    std::fs::write(data.path().join("r2-staging/unowned"), b"keep").unwrap();
    assert_eq!(
        staging.cleanup().unwrap_err().code(),
        ErrorCode::PathInvalid
    );
    assert!(data.path().join("r2-staging/unowned").exists());
}

use std::os::unix::fs::PermissionsExt as _;
