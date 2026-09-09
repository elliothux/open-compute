use super::*;

#[test]
fn runtime_source_error_mapping_is_stable_and_sanitized() {
    for input in [
        ErrorCode::ArtifactIntegrityError,
        ErrorCode::CacheEntryCorrupt,
    ] {
        let mapped = map_artifact_error(open_compute_core::PlatformError::new(input, "raw path"));
        assert_eq!(mapped.code(), ErrorCode::ArtifactIntegrityError);
        assert!(!mapped.message().contains("raw path"));
    }
    let unavailable = map_artifact_error(open_compute_core::PlatformError::new(
        ErrorCode::ObjectStorageUnavailable,
        "signed URL",
    ));
    assert_eq!(unavailable.code(), ErrorCode::ArtifactUnavailable);
    assert!(!unavailable.message().contains("signed URL"));
    assert_eq!(not_ready().code(), ErrorCode::VersionNotReady);
    assert_eq!(
        source_invariant().code(),
        ErrorCode::VersionInvariantViolation
    );
}
