use super::*;

#[test]
fn current_persisted_codes_round_trip_and_unknown_is_rejected() {
    for code in [
        ErrorCode::QuotaExceeded,
        ErrorCode::AdmissionBusy,
        ErrorCode::StoragePressure,
        ErrorCode::PlatformUnavailable,
        ErrorCode::QueueConfigPending,
    ] {
        assert_eq!(ErrorCode::from_stable_str(code.as_str()), Some(code));
    }
    assert_eq!(ErrorCode::from_stable_str("UNKNOWN"), None);
    assert_eq!(ErrorCode::from_stable_str("quota_exceeded"), None);
}

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
        ErrorCode::QuotaExceeded,
        ErrorCode::AdmissionBusy,
        ErrorCode::StoragePressure,
        ErrorCode::PlatformUnavailable,
        ErrorCode::SnapshotInvalid,
        ErrorCode::RestoreInvalid,
        ErrorCode::SchemaUnsupported,
        ErrorCode::ReleaseUnsupported,
        ErrorCode::SupportBundleInvalid,
        ErrorCode::DataDirInUse,
    ] {
        assert!(!code.as_str().is_empty());
    }
}

#[test]
fn readiness_distinguishes_serviceable_degradation_from_required_failure() {
    assert!(ReadinessReason::Ready.is_ready());
    assert!(ReadinessReason::SnapshotStale.is_ready());
    assert!(ReadinessReason::DiskHardLimit.is_ready());
    assert!(!ReadinessReason::Starting.is_ready());
    assert!(!ReadinessReason::S3Unavailable.is_ready());
    assert_eq!(ReadinessReason::S3Unavailable.as_str(), "S3_UNAVAILABLE");
}

#[test]
fn system_scheduler_clock_produces_future_monotonic_deadlines() {
    use crate::{SchedulerClock as _, SystemSchedulerClock};
    use std::time::{Duration, Instant};

    let before = Instant::now();
    let deadline = SystemSchedulerClock.monotonic_deadline(Duration::from_millis(1));
    assert!(deadline >= before);
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
        ErrorCode::KvKeyInvalid,
        ErrorCode::KvKeyTooLarge,
        ErrorCode::KvValueTooLarge,
        ErrorCode::KvMetadataInvalid,
        ErrorCode::KvMetadataTooLarge,
        ErrorCode::KvInvalidOptions,
        ErrorCode::KvTooManyKeys,
        ErrorCode::KvResponseTooLarge,
        ErrorCode::KvCursorInvalid,
        ErrorCode::KvBusy,
        ErrorCode::KvStorageFull,
        ErrorCode::KvUnavailable,
        ErrorCode::KvCorrupt,
        ErrorCode::KvResultUnknown,
        ErrorCode::KvInternalProtocolError,
        ErrorCode::R2KeyInvalid,
        ErrorCode::R2KeyTooLarge,
        ErrorCode::R2InvalidOptions,
        ErrorCode::R2UnsupportedCondition,
        ErrorCode::R2UnsupportedFeature,
        ErrorCode::R2ObjectTooLarge,
        ErrorCode::R2MetadataTooLarge,
        ErrorCode::R2CursorInvalid,
        ErrorCode::R2BucketNotEmpty,
        ErrorCode::R2PreconditionFailed,
        ErrorCode::R2Overloaded,
        ErrorCode::R2ProviderUnavailable,
        ErrorCode::R2ResultUnknown,
        ErrorCode::R2ObjectMetadataInvalid,
        ErrorCode::R2PrefixCollision,
        ErrorCode::D1TypeError,
        ErrorCode::D1SqlInvalid,
        ErrorCode::D1ParameterMismatch,
        ErrorCode::D1AuthorizerDenied,
        ErrorCode::D1LimitError,
        ErrorCode::D1Timeout,
        ErrorCode::D1ColumnNotFound,
        ErrorCode::D1InvalidBatch,
        ErrorCode::D1SessionUnsupported,
        ErrorCode::D1MigrationDrift,
        ErrorCode::D1DatabaseFull,
        ErrorCode::D1Overloaded,
        ErrorCode::D1ResultUnknown,
        ErrorCode::D1DatabaseCorrupt,
        ErrorCode::D1IdentityMismatch,
        ErrorCode::D1InternalProtocolError,
        ErrorCode::DoNamespaceNotFound,
        ErrorCode::DoIdInvalid,
        ErrorCode::DoObjectDeleting,
        ErrorCode::DoDeploymentStale,
        ErrorCode::DoClassNotFound,
        ErrorCode::DoStorageUnavailable,
        ErrorCode::DoStorageLimit,
        ErrorCode::DoDispatchTimeout,
        ErrorCode::DoRpcUnsupported,
        ErrorCode::DoRuntimeException,
        ErrorCode::DoPlacementOptionUnsupported,
        ErrorCode::DoNamespaceNotEmpty,
        ErrorCode::DoInternalProtocolError,
        ErrorCode::SchedulerUnavailable,
        ErrorCode::SchedulerCorrupt,
        ErrorCode::SchedulerBusy,
        ErrorCode::SchedulerInternalProtocolError,
        ErrorCode::SchedulerKindNotEnabled,
        ErrorCode::DoAlarmIndexUnavailable,
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
        ReadinessReason::SchedulerUnavailable,
        ReadinessReason::SchedulerBacklog,
        ReadinessReason::S3Degraded,
        ReadinessReason::DiskSoftLimit,
        ReadinessReason::SnapshotStale,
        ReadinessReason::Ready,
    ];
    for reason in reasons {
        assert_eq!(reason.to_string(), reason.as_str());
        assert_eq!(
            reason.is_ready(),
            matches!(
                reason,
                ReadinessReason::Ready
                    | ReadinessReason::SchedulerUnavailable
                    | ReadinessReason::SchedulerBacklog
                    | ReadinessReason::S3Degraded
                    | ReadinessReason::DiskSoftLimit
                    | ReadinessReason::DiskHardLimit
                    | ReadinessReason::SnapshotStale
            )
        );
    }
}
