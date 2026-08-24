use super::*;

#[test]
fn public_error_exposes_only_code_and_static_operator_text() {
    const MESSAGE: &str = "master key fingerprint mismatch";
    let err = PlatformError::new(ErrorCode::MasterKeyMismatch, MESSAGE);
    let display = err.to_string();
    let debug = format!("{err:?}");
    let json = serde_json::to_string(&err).expect("json");
    assert!(display.contains("MASTER_KEY_MISMATCH"));
    assert!(debug.contains("MASTER_KEY_MISMATCH"));
    assert!(json.contains("MASTER_KEY_MISMATCH"));
    assert_eq!(err.message(), MESSAGE);
    assert!(!display.contains("ocmk1:"));
    assert!(!json.contains("secret"));
}

#[test]
fn every_section_16_failure_has_a_stable_code() {
    for code in [
        ErrorCode::RuntimeInvalid,
        ErrorCode::ConfigCompileFailed,
        ErrorCode::MigrationFailed,
        ErrorCode::SchemaTooNew,
        ErrorCode::MasterKeyMismatch,
        ErrorCode::S3Unavailable,
        ErrorCode::CacheEntryCorrupt,
        ErrorCode::RuntimeExitedBeforeReady,
        ErrorCode::RuntimeExitedInFlight,
        ErrorCode::ProcessKilled,
        ErrorCode::DiskHardLimit,
        ErrorCode::DataDirInUse,
    ] {
        assert!(!code.as_str().is_empty());
    }
}

#[test]
fn readiness_ready_is_the_only_ready_reason() {
    assert!(ReadinessReason::Ready.is_ready());
    assert!(!ReadinessReason::Starting.is_ready());
    assert_eq!(ReadinessReason::S3Unavailable.as_str(), "S3_UNAVAILABLE");
}

#[test]
fn every_error_and_readiness_token_formats_stably() {
    let codes = [
        ErrorCode::ConfigPathInvalid,
        ErrorCode::ConfigParseFailed,
        ErrorCode::ConfigInvalid,
        ErrorCode::RuntimeInvalid,
        ErrorCode::ConfigCompileFailed,
        ErrorCode::MigrationFailed,
        ErrorCode::SchemaTooNew,
        ErrorCode::MasterKeyMismatch,
        ErrorCode::S3Unavailable,
        ErrorCode::CacheEntryCorrupt,
        ErrorCode::RuntimeExitedBeforeReady,
        ErrorCode::RuntimeExitedInFlight,
        ErrorCode::ProcessKilled,
        ErrorCode::DiskHardLimit,
        ErrorCode::DataDirInUse,
        ErrorCode::AdminAuthRequired,
        ErrorCode::SecretRefInvalid,
        ErrorCode::PathInvalid,
        ErrorCode::S3PrefixInvalid,
        ErrorCode::CacheBoundsInvalid,
        ErrorCode::LimitInvalid,
        ErrorCode::ArtifactIntegrityError,
        ErrorCode::AccountNotFound,
        ErrorCode::WorkerNotFound,
        ErrorCode::WorkerNameConflict,
        ErrorCode::WorkerDeleted,
        ErrorCode::DeploymentNotFound,
        ErrorCode::DeploymentNotReady,
        ErrorCode::DeploymentActive,
        ErrorCode::DeploymentReferenced,
        ErrorCode::DeploymentInvariantViolation,
        ErrorCode::BundleInvalid,
        ErrorCode::BundleTooLarge,
        ErrorCode::BundleRuntimeInvalid,
        ErrorCode::CompatibilityUnsupported,
        ErrorCode::ArtifactUnavailable,
        ErrorCode::RouteNotFound,
        ErrorCode::RouteConflict,
        ErrorCode::EntrypointNotFound,
        ErrorCode::SecretInvalid,
        ErrorCode::IdempotencyConflict,
        ErrorCode::RuntimeUnavailable,
        ErrorCode::RuntimeResultUnknown,
        ErrorCode::ResourceLimitExceeded,
        ErrorCode::ResourceNotFound,
        ErrorCode::ResourceNameConflict,
        ErrorCode::ResourceNotReady,
        ErrorCode::ResourceReferenced,
        ErrorCode::ResourceUnavailable,
        ErrorCode::ResourceInvariantViolation,
        ErrorCode::BindingNotFound,
        ErrorCode::BindingTypeMismatch,
        ErrorCode::BindingPermissionDenied,
        ErrorCode::BindingCapabilityUnsupported,
        ErrorCode::BindingProtocolError,
        ErrorCode::BindingLimitExceeded,
        ErrorCode::BindingResultUnknown,
        ErrorCode::Internal,
    ];
    for code in codes {
        assert_eq!(code.to_string(), code.as_str());
    }

    let reasons = [
        ReadinessReason::Starting,
        ReadinessReason::MigrationFailed,
        ReadinessReason::MasterKeyMismatch,
        ReadinessReason::S3Unavailable,
        ReadinessReason::RuntimeStarting,
        ReadinessReason::RuntimeRestartBackoff,
        ReadinessReason::RuntimeInvalid,
        ReadinessReason::Draining,
        ReadinessReason::SchemaTooNew,
        ReadinessReason::DataDirInUse,
        ReadinessReason::DiskHardLimit,
        ReadinessReason::ConfigInvalid,
        ReadinessReason::Ready,
    ];
    for reason in reasons {
        assert_eq!(reason.to_string(), reason.as_str());
        assert_eq!(reason.is_ready(), reason == ReadinessReason::Ready);
    }
}
