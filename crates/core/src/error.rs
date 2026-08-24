//! Stable error codes and a secret-safe error type.

use serde::{Serialize, Serializer};
use std::fmt::{Debug, Display, Formatter};
use thiserror::Error;

/// Stable, operator-visible failure code.
///
/// Codes cover every P0.1 failure in the platform foundation design section 16
/// plus the static validation failures this crate owns.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    /// `--config` path was missing, relative, or not a regular file path form.
    ConfigPathInvalid,
    /// TOML could not be parsed or contained unknown fields.
    ConfigParseFailed,
    /// Static validation failed (paths, ratios, timeouts, secret refs).
    ConfigInvalid,
    /// workerd hash/version did not match the lock.
    RuntimeInvalid,
    /// Static workerd config compilation failed.
    ConfigCompileFailed,
    /// Control-plane migration failed and was rolled back.
    MigrationFailed,
    /// On-disk schema is newer than this binary.
    SchemaTooNew,
    /// Configured master key does not match stored fingerprint.
    MasterKeyMismatch,
    /// S3 preflight failed after bounded retries.
    S3Unavailable,
    /// Cache entry failed integrity checks.
    CacheEntryCorrupt,
    /// workerd exited before becoming ready.
    RuntimeExitedBeforeReady,
    /// workerd exited while handling a request; result may be unknown.
    RuntimeExitedInFlight,
    /// platformd was SIGKILL'd; only fsynced state is promised.
    ProcessKilled,
    /// Data directory free space reached the hard limit.
    DiskHardLimit,
    /// Data directory exclusive lock is held by another instance.
    DataDirInUse,
    /// Admin bind is non-loopback and no admin auth secret is configured.
    AdminAuthRequired,
    /// A secret reference is incomplete or internally contradictory.
    SecretRefInvalid,
    /// A configured filesystem path is not an acceptable absolute path.
    PathInvalid,
    /// Internal S3 prefix is not a valid isolated platform prefix.
    S3PrefixInvalid,
    /// Cache watermark or size bounds are inconsistent.
    CacheBoundsInvalid,
    /// Timeout, retry, or size was zero or outside the documented bound.
    LimitInvalid,
    /// Artifact bytes or metadata failed integrity verification.
    ArtifactIntegrityError,
    /// Requested account does not exist or is tombstoned.
    AccountNotFound,
    /// Requested Worker does not exist in the account.
    WorkerNotFound,
    /// A live Worker already owns the requested name.
    WorkerNameConflict,
    /// The Worker has been tombstoned.
    WorkerDeleted,
    /// Requested deployment does not exist for the Worker.
    DeploymentNotFound,
    /// Deployment is not ready for the requested operation.
    DeploymentNotReady,
    /// Deployment is currently active and cannot be deleted.
    DeploymentActive,
    /// Deployment still has a live referrer or in-flight pin.
    DeploymentReferenced,
    /// Immutable deployment metadata no longer matches its descriptor.
    DeploymentInvariantViolation,
    /// Worker bundle framing, module metadata, or source is invalid.
    BundleInvalid,
    /// Worker bundle exceeds a configured structural limit.
    BundleTooLarge,
    /// Real runtime validation rejected the bundle.
    BundleRuntimeInvalid,
    /// Compatibility date or flag is not supported by the pinned runtime policy.
    CompatibilityUnsupported,
    /// A referenced artifact could not be opened.
    ArtifactUnavailable,
    /// Public route did not resolve to a live active deployment.
    RouteNotFound,
    /// A live route already owns the canonical host and path prefix.
    RouteConflict,
    /// Requested named entrypoint does not exist.
    EntrypointNotFound,
    /// A deployment secret name or value is invalid.
    SecretInvalid,
    /// An idempotency key was reused with a different canonical request.
    IdempotencyConflict,
    /// workerd is not available for dispatch.
    RuntimeUnavailable,
    /// A runtime response started but its final result is unknown.
    RuntimeResultUnknown,
    /// A request or runtime resource limit was exceeded.
    ResourceLimitExceeded,
    /// Requested resource does not exist in the authorized account.
    ResourceNotFound,
    /// A live resource already owns the requested display name.
    ResourceNameConflict,
    /// Resource lifecycle does not currently admit the requested operation.
    ResourceNotReady,
    /// Resource still has a retained referrer or in-flight pin.
    ResourceReferenced,
    /// One resource is unavailable without making the platform unavailable.
    ResourceUnavailable,
    /// Persisted resource identity, schema, or catalog data is inconsistent.
    ResourceInvariantViolation,
    /// Runtime binding authority row is missing.
    BindingNotFound,
    /// Binding kind does not match its adapter or resource.
    BindingTypeMismatch,
    /// Binding permission set rejects the requested method.
    BindingPermissionDenied,
    /// Binding capability version is not implemented by the static registry.
    BindingCapabilityUnsupported,
    /// Private binding transport frame is malformed or truncated.
    BindingProtocolError,
    /// Binding request, response, or stream exceeded its fixed budget.
    BindingLimitExceeded,
    /// A binding mutation may have committed before transport failure.
    BindingResultUnknown,
    /// A secret-safe internal P0.2 failure.
    Internal,
}

impl ErrorCode {
    /// Canonical uppercase snake-case token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ConfigPathInvalid => "CONFIG_PATH_INVALID",
            Self::ConfigParseFailed => "CONFIG_PARSE_FAILED",
            Self::ConfigInvalid => "CONFIG_INVALID",
            Self::RuntimeInvalid => "RUNTIME_INVALID",
            Self::ConfigCompileFailed => "CONFIG_COMPILE_FAILED",
            Self::MigrationFailed => "MIGRATION_FAILED",
            Self::SchemaTooNew => "SCHEMA_TOO_NEW",
            Self::MasterKeyMismatch => "MASTER_KEY_MISMATCH",
            Self::S3Unavailable => "S3_UNAVAILABLE",
            Self::CacheEntryCorrupt => "CACHE_ENTRY_CORRUPT",
            Self::RuntimeExitedBeforeReady => "RUNTIME_EXITED_BEFORE_READY",
            Self::RuntimeExitedInFlight => "RUNTIME_EXITED_IN_FLIGHT",
            Self::ProcessKilled => "PROCESS_KILLED",
            Self::DiskHardLimit => "DISK_HARD_LIMIT",
            Self::DataDirInUse => "DATA_DIR_IN_USE",
            Self::AdminAuthRequired => "ADMIN_AUTH_REQUIRED",
            Self::SecretRefInvalid => "SECRET_REF_INVALID",
            Self::PathInvalid => "PATH_INVALID",
            Self::S3PrefixInvalid => "S3_PREFIX_INVALID",
            Self::CacheBoundsInvalid => "CACHE_BOUNDS_INVALID",
            Self::LimitInvalid => "LIMIT_INVALID",
            Self::ArtifactIntegrityError => "ARTIFACT_INTEGRITY_ERROR",
            Self::AccountNotFound => "ACCOUNT_NOT_FOUND",
            Self::WorkerNotFound => "WORKER_NOT_FOUND",
            Self::WorkerNameConflict => "WORKER_NAME_CONFLICT",
            Self::WorkerDeleted => "WORKER_DELETED",
            Self::DeploymentNotFound => "DEPLOYMENT_NOT_FOUND",
            Self::DeploymentNotReady => "DEPLOYMENT_NOT_READY",
            Self::DeploymentActive => "DEPLOYMENT_ACTIVE",
            Self::DeploymentReferenced => "DEPLOYMENT_REFERENCED",
            Self::DeploymentInvariantViolation => "DEPLOYMENT_INVARIANT_VIOLATION",
            Self::BundleInvalid => "BUNDLE_INVALID",
            Self::BundleTooLarge => "BUNDLE_TOO_LARGE",
            Self::BundleRuntimeInvalid => "BUNDLE_RUNTIME_INVALID",
            Self::CompatibilityUnsupported => "COMPATIBILITY_UNSUPPORTED",
            Self::ArtifactUnavailable => "ARTIFACT_UNAVAILABLE",
            Self::RouteNotFound => "ROUTE_NOT_FOUND",
            Self::RouteConflict => "ROUTE_CONFLICT",
            Self::EntrypointNotFound => "ENTRYPOINT_NOT_FOUND",
            Self::SecretInvalid => "SECRET_INVALID",
            Self::IdempotencyConflict => "IDEMPOTENCY_CONFLICT",
            Self::RuntimeUnavailable => "RUNTIME_UNAVAILABLE",
            Self::RuntimeResultUnknown => "RUNTIME_RESULT_UNKNOWN",
            Self::ResourceLimitExceeded => "RESOURCE_LIMIT_EXCEEDED",
            Self::ResourceNotFound => "RESOURCE_NOT_FOUND",
            Self::ResourceNameConflict => "RESOURCE_NAME_CONFLICT",
            Self::ResourceNotReady => "RESOURCE_NOT_READY",
            Self::ResourceReferenced => "RESOURCE_REFERENCED",
            Self::ResourceUnavailable => "RESOURCE_UNAVAILABLE",
            Self::ResourceInvariantViolation => "RESOURCE_INVARIANT_VIOLATION",
            Self::BindingNotFound => "BINDING_NOT_FOUND",
            Self::BindingTypeMismatch => "BINDING_TYPE_MISMATCH",
            Self::BindingPermissionDenied => "BINDING_PERMISSION_DENIED",
            Self::BindingCapabilityUnsupported => "BINDING_CAPABILITY_UNSUPPORTED",
            Self::BindingProtocolError => "BINDING_PROTOCOL_ERROR",
            Self::BindingLimitExceeded => "BINDING_LIMIT_EXCEEDED",
            Self::BindingResultUnknown => "BINDING_RESULT_UNKNOWN",
            Self::Internal => "INTERNAL",
        }
    }
}

impl Display for ErrorCode {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Stable readiness reason returned by `/health/ready`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReadinessReason {
    /// Process is still executing the startup sequence.
    Starting,
    /// Control-plane migration failed.
    MigrationFailed,
    /// Master key fingerprint mismatch.
    MasterKeyMismatch,
    /// S3 is unavailable.
    S3Unavailable,
    /// workerd is spawning or probing.
    RuntimeStarting,
    /// workerd is waiting out restart backoff.
    RuntimeRestartBackoff,
    /// workerd binary/config is invalid; will not retry.
    RuntimeInvalid,
    /// Process is draining for shutdown.
    Draining,
    /// Schema is newer than this binary.
    SchemaTooNew,
    /// Data directory lock is held.
    DataDirInUse,
    /// Disk hard limit reached; mutations refused.
    DiskHardLimit,
    /// Static configuration is invalid.
    ConfigInvalid,
    /// All required components are ready.
    Ready,
}

impl ReadinessReason {
    /// Canonical uppercase snake-case token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "STARTING",
            Self::MigrationFailed => "MIGRATION_FAILED",
            Self::MasterKeyMismatch => "MASTER_KEY_MISMATCH",
            Self::S3Unavailable => "S3_UNAVAILABLE",
            Self::RuntimeStarting => "RUNTIME_STARTING",
            Self::RuntimeRestartBackoff => "RUNTIME_RESTART_BACKOFF",
            Self::RuntimeInvalid => "RUNTIME_INVALID",
            Self::Draining => "DRAINING",
            Self::SchemaTooNew => "SCHEMA_TOO_NEW",
            Self::DataDirInUse => "DATA_DIR_IN_USE",
            Self::DiskHardLimit => "DISK_HARD_LIMIT",
            Self::ConfigInvalid => "CONFIG_INVALID",
            Self::Ready => "READY",
        }
    }

    /// Whether this reason reports the platform as ready to take traffic.
    #[must_use]
    pub const fn is_ready(self) -> bool {
        matches!(self, Self::Ready)
    }
}

impl Display for ReadinessReason {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error that never embeds secret values in Display, Debug, or Serialize.
///
/// Messages are compile-time `&'static str` only. Callers cannot pass a
/// runtime `String` (credentials, parser payloads, or other secrets) through
/// the public constructor.
#[derive(Clone, Error)]
#[error("{code}: {message}")]
pub struct PlatformError {
    code: ErrorCode,
    message: &'static str,
}

impl PlatformError {
    /// Construct an error from a stable code and a compile-time operator message.
    #[must_use]
    pub const fn new(code: ErrorCode, message: &'static str) -> Self {
        Self { code, message }
    }

    /// Stable code.
    #[must_use]
    pub const fn code(&self) -> ErrorCode {
        self.code
    }

    /// Secret-free operator message.
    #[must_use]
    pub const fn message(&self) -> &'static str {
        self.message
    }
}

impl Debug for PlatformError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlatformError")
            .field("code", &self.code.as_str())
            .field("message", &self.message)
            .finish()
    }
}

impl Serialize for PlatformError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("PlatformError", 2)?;
        state.serialize_field("code", &self.code)?;
        state.serialize_field("message", &self.message)?;
        state.end()
    }
}

#[cfg(test)]
#[path = "error_tests.rs"]
mod tests;
