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
    /// A KV key is empty or otherwise outside the documented key grammar.
    KvKeyInvalid,
    /// A KV key exceeds the 512-byte UTF-8 limit.
    KvKeyTooLarge,
    /// A KV value exceeds the 25 MiB limit.
    KvValueTooLarge,
    /// KV metadata is not canonical JSON-compatible data.
    KvMetadataInvalid,
    /// Canonical KV metadata exceeds 1024 bytes.
    KvMetadataTooLarge,
    /// KV expiration, cache, list, or type options are invalid.
    KvInvalidOptions,
    /// A KV multi-get contains more than 100 keys.
    KvTooManyKeys,
    /// A KV aggregate response exceeds the 25 MiB response budget.
    KvResponseTooLarge,
    /// A KV list cursor is malformed, expired, or scoped incorrectly.
    KvCursorInvalid,
    /// A KV namespace writer or connection is temporarily busy.
    KvBusy,
    /// KV storage quota or the platform disk safety floor was reached.
    KvStorageFull,
    /// One KV namespace is temporarily unavailable.
    KvUnavailable,
    /// One KV namespace database failed integrity validation.
    KvCorrupt,
    /// A KV mutation may have committed before its result was observed.
    KvResultUnknown,
    /// The private KV adapter protocol was malformed.
    KvInternalProtocolError,
    /// R2 object key type, UTF-8, or segment shape is invalid.
    R2KeyInvalid,
    /// R2 object key exceeds its logical bucket's dynamic provider-key budget.
    R2KeyTooLarge,
    /// R2 range, conditional, metadata, or list options are invalid.
    R2InvalidOptions,
    /// A requested R2 write condition cannot be executed atomically.
    R2UnsupportedCondition,
    /// An R2 feature is intentionally outside the P0.5 capability.
    R2UnsupportedFeature,
    /// R2 object bytes exceed the bucket's frozen single-part limit.
    R2ObjectTooLarge,
    /// Canonical R2 custom metadata exceeds its fixed budget.
    R2MetadataTooLarge,
    /// An R2 list cursor is malformed, expired, or scoped incorrectly.
    R2CursorInvalid,
    /// A logical R2 bucket is non-empty and force deletion was not requested.
    R2BucketNotEmpty,
    /// An R2 conditional operation did not match the current object.
    R2PreconditionFailed,
    /// R2 concurrency or staging capacity is temporarily saturated.
    R2Overloaded,
    /// The configured R2 provider is unavailable.
    R2ProviderUnavailable,
    /// An R2 mutation may have committed before its response was observed.
    R2ResultUnknown,
    /// Provider metadata for one R2 object failed validation.
    R2ObjectMetadataInvalid,
    /// A logical bucket physical identity marker belongs to another authority.
    R2PrefixCollision,
    /// A JavaScript value cannot be represented by the D1 binding protocol.
    D1TypeError,
    /// SQL is empty, malformed, or contains an unexpected second statement.
    D1SqlInvalid,
    /// Bound values do not match the prepared statement parameter slots.
    D1ParameterMismatch,
    /// SQLite authorizer rejected tenant SQL.
    D1AuthorizerDenied,
    /// A D1 SQL, value, row, result, or VM bound was exceeded.
    D1LimitError,
    /// A D1 query or batch exceeded its wall deadline.
    D1Timeout,
    /// `first(column)` named a column absent from the result.
    D1ColumnNotFound,
    /// A D1 batch is empty, oversized, forged, or crosses owner scope.
    D1InvalidBatch,
    /// A requested D1 replica or bookmark session is not implemented locally.
    D1SessionUnsupported,
    /// An applied D1 migration identity conflicts with different SQL.
    D1MigrationDrift,
    /// A D1 database quota or disk safety bound was reached.
    D1DatabaseFull,
    /// A D1 operation queue or blocking executor is saturated.
    D1Overloaded,
    /// A D1 mutation may have committed before its response was observed.
    D1ResultUnknown,
    /// A tenant D1 SQLite file failed integrity validation.
    D1DatabaseCorrupt,
    /// A tenant D1 file belongs to a different account or resource.
    D1IdentityMismatch,
    /// The private D1 facade/transport/backend protocol was malformed.
    D1InternalProtocolError,
    /// Durable Object namespace or binding authority does not exist.
    DoNamespaceNotFound,
    /// Public Durable Object identity is malformed or belongs to another namespace.
    DoIdInvalid,
    /// Durable Object deletion has fenced new dispatches.
    DoObjectDeleting,
    /// A late call carries an execution generation older than the host actor has observed.
    DoDeploymentStale,
    /// The active deployment no longer exports the namespace class.
    DoClassNotFound,
    /// Native Durable Object storage or its local disk is unavailable.
    DoStorageUnavailable,
    /// Durable Object local-disk capacity policy rejected a write or new identity.
    DoStorageLimit,
    /// Durable Object dispatch exceeded its bounded foreground deadline.
    DoDispatchTimeout,
    /// The requested plain-data RPC method or value is outside the P0.7 surface.
    DoRpcUnsupported,
    /// Tenant Durable Object code raised an opaque runtime exception.
    DoRuntimeException,
    /// Single-node P0.7 does not implement placement hints.
    DoPlacementOptionUnsupported,
    /// A namespace still owns live objects and force deletion was not requested.
    DoNamespaceNotEmpty,
    /// The private Durable Object transport protocol was malformed.
    DoInternalProtocolError,
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
            Self::KvKeyInvalid => "KV_KEY_INVALID",
            Self::KvKeyTooLarge => "KV_KEY_TOO_LARGE",
            Self::KvValueTooLarge => "KV_VALUE_TOO_LARGE",
            Self::KvMetadataInvalid => "KV_METADATA_INVALID",
            Self::KvMetadataTooLarge => "KV_METADATA_TOO_LARGE",
            Self::KvInvalidOptions => "KV_INVALID_OPTIONS",
            Self::KvTooManyKeys => "KV_TOO_MANY_KEYS",
            Self::KvResponseTooLarge => "KV_RESPONSE_TOO_LARGE",
            Self::KvCursorInvalid => "KV_CURSOR_INVALID",
            Self::KvBusy => "KV_BUSY",
            Self::KvStorageFull => "KV_STORAGE_FULL",
            Self::KvUnavailable => "KV_UNAVAILABLE",
            Self::KvCorrupt => "KV_CORRUPT",
            Self::KvResultUnknown => "KV_RESULT_UNKNOWN",
            Self::KvInternalProtocolError => "KV_INTERNAL_PROTOCOL_ERROR",
            Self::R2KeyInvalid => "R2_KEY_INVALID",
            Self::R2KeyTooLarge => "R2_KEY_TOO_LARGE",
            Self::R2InvalidOptions => "R2_INVALID_OPTIONS",
            Self::R2UnsupportedCondition => "R2_UNSUPPORTED_CONDITION",
            Self::R2UnsupportedFeature => "R2_UNSUPPORTED_FEATURE",
            Self::R2ObjectTooLarge => "R2_OBJECT_TOO_LARGE",
            Self::R2MetadataTooLarge => "R2_METADATA_TOO_LARGE",
            Self::R2CursorInvalid => "R2_CURSOR_INVALID",
            Self::R2BucketNotEmpty => "R2_BUCKET_NOT_EMPTY",
            Self::R2PreconditionFailed => "R2_PRECONDITION_FAILED",
            Self::R2Overloaded => "R2_OVERLOADED",
            Self::R2ProviderUnavailable => "R2_PROVIDER_UNAVAILABLE",
            Self::R2ResultUnknown => "R2_RESULT_UNKNOWN",
            Self::R2ObjectMetadataInvalid => "R2_OBJECT_METADATA_INVALID",
            Self::R2PrefixCollision => "R2_PREFIX_COLLISION",
            Self::D1TypeError => "D1_TYPE_ERROR",
            Self::D1SqlInvalid => "D1_SQL_INVALID",
            Self::D1ParameterMismatch => "D1_PARAMETER_MISMATCH",
            Self::D1AuthorizerDenied => "D1_AUTHORIZER_DENIED",
            Self::D1LimitError => "D1_LIMIT_ERROR",
            Self::D1Timeout => "D1_TIMEOUT",
            Self::D1ColumnNotFound => "D1_COLUMN_NOTFOUND",
            Self::D1InvalidBatch => "D1_INVALID_BATCH",
            Self::D1SessionUnsupported => "D1_SESSION_UNSUPPORTED",
            Self::D1MigrationDrift => "D1_MIGRATION_DRIFT",
            Self::D1DatabaseFull => "D1_DATABASE_FULL",
            Self::D1Overloaded => "D1_OVERLOADED",
            Self::D1ResultUnknown => "D1_RESULT_UNKNOWN",
            Self::D1DatabaseCorrupt => "D1_DATABASE_CORRUPT",
            Self::D1IdentityMismatch => "D1_IDENTITY_MISMATCH",
            Self::D1InternalProtocolError => "D1_INTERNAL_PROTOCOL_ERROR",
            Self::DoNamespaceNotFound => "DO_NAMESPACE_NOT_FOUND",
            Self::DoIdInvalid => "DO_ID_INVALID",
            Self::DoObjectDeleting => "DO_OBJECT_DELETING",
            Self::DoDeploymentStale => "DO_DEPLOYMENT_STALE",
            Self::DoClassNotFound => "DO_CLASS_NOT_FOUND",
            Self::DoStorageUnavailable => "DO_STORAGE_UNAVAILABLE",
            Self::DoStorageLimit => "DO_STORAGE_LIMIT",
            Self::DoDispatchTimeout => "DO_DISPATCH_TIMEOUT",
            Self::DoRpcUnsupported => "DO_RPC_UNSUPPORTED",
            Self::DoRuntimeException => "DO_RUNTIME_EXCEPTION",
            Self::DoPlacementOptionUnsupported => "DO_PLACEMENT_OPTION_UNSUPPORTED",
            Self::DoNamespaceNotEmpty => "DO_NAMESPACE_NOT_EMPTY",
            Self::DoInternalProtocolError => "DO_INTERNAL_PROTOCOL_ERROR",
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
