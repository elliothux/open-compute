use super::*;

#[test]
fn private_asset_protocol_parsing_and_errors_are_stable() {
    assert_eq!(parse_digest(&hex::encode([7; 32])).unwrap(), [7; 32]);
    for invalid in [
        "0",
        "AA00000000000000000000000000000000000000000000000000000000000000",
        "gg00000000000000000000000000000000000000000000000000000000000000",
    ] {
        assert_eq!(
            parse_digest(invalid).unwrap_err().code(),
            ErrorCode::BindingProtocolError
        );
    }
    assert_eq!(parse_stored_digest(&hex::encode([8; 32])).unwrap(), [8; 32]);
    assert_eq!(
        parse_stored_digest("invalid").unwrap_err().code(),
        ErrorCode::Internal
    );

    for (input, expected) in [
        (
            ErrorCode::ArtifactIntegrityError,
            ErrorCode::AssetIntegrityError,
        ),
        (ErrorCode::CacheEntryCorrupt, ErrorCode::AssetIntegrityError),
        (
            ErrorCode::ArtifactUnavailable,
            ErrorCode::AssetStorageUnavailable,
        ),
    ] {
        assert_eq!(
            map_asset_artifact_error(&PlatformError::new(input, "unsafe detail")).code(),
            expected
        );
    }

    for (input, status, exposed) in [
        (
            ErrorCode::BindingProtocolError,
            StatusCode::BAD_REQUEST,
            ErrorCode::BindingProtocolError,
        ),
        (
            ErrorCode::AssetIntegrityError,
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::AssetIntegrityError,
        ),
        (
            ErrorCode::DeploymentInvariantViolation,
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::AssetIntegrityError,
        ),
        (
            ErrorCode::AssetStorageUnavailable,
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::AssetStorageUnavailable,
        ),
        (
            ErrorCode::Internal,
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::AssetStorageUnavailable,
        ),
    ] {
        let response = asset_error(&PlatformError::new(input, "unsafe detail"));
        assert_eq!(response.status(), status);
        assert_eq!(response.headers()[ERROR_HEADER], exposed.as_str());
    }
    assert_eq!(protocol_error().code(), ErrorCode::BindingProtocolError);
    assert_eq!(invariant().code(), ErrorCode::DeploymentInvariantViolation);
    assert_eq!(internal().code(), ErrorCode::Internal);
}
