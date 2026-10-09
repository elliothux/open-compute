//! Stable error codes and a secret-safe error type.

use serde::{Deserialize, Serialize, Serializer};
use std::fmt::Debug;
use thiserror::Error;

/// Stable, operator-visible failure code.
///
/// Codes cover every P0.1 failure in the platform foundation design section 16
/// plus the static validation failures this crate owns.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
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
    /// The configured object-byte authority is temporarily unavailable.
    ObjectStorageUnavailable,
    /// Persisted object bytes or object metadata failed integrity verification.
    ObjectStorageIntegrityError,
    /// The configured object authority does not match the initialized platform.
    ObjectStorageAuthorityMismatch,
    /// The configured object authority rejected a write for capacity reasons.
    ObjectStorageCapacity,
    /// Cache entry failed integrity checks.
    CacheEntryCorrupt,
    /// workerd exited before becoming ready.
    RuntimeExitedBeforeReady,
    /// workerd exited while handling a request; result may be unknown.
    RuntimeExitedInFlight,
    /// ocd was SIGKILL'd; only fsynced state is promised.
    ProcessKilled,
    /// Data directory free space reached the hard limit.
    DiskHardLimit,
    /// A platform-wide resource count or immutable product quota was exceeded.
    QuotaExceeded,
    /// A bounded admission queue or reservation counter is saturated.
    AdmissionBusy,
    /// The host hard reserve would be violated by a local-state mutation.
    StoragePressure,
    /// The process is draining or an offline operation owns the data directory.
    PlatformUnavailable,
    /// A platform snapshot manifest, object, or local tree failed validation.
    SnapshotInvalid,
    /// A fresh-host restore target or restored authority failed validation.
    RestoreInvalid,
    /// Persisted schema identity does not match this implementation.
    SchemaUnsupported,
    /// A release identity is not supported for restore.
    ReleaseUnsupported,
    /// A support-bundle output path or allowlisted input failed validation.
    SupportBundleInvalid,
    /// Data directory exclusive lock is held by another instance.
    DataDirInUse,
    /// Admin bind is non-loopback and no admin auth secret is configured.
    AdminAuthRequired,
    /// A secret reference is incomplete or internally contradictory.
    SecretRefInvalid,
    /// A configured filesystem path is not an acceptable absolute path.
    PathInvalid,
    /// Object-storage prefixes are not valid isolated platform prefixes.
    ObjectStoragePrefixInvalid,
    /// Cache watermark or size bounds are inconsistent.
    CacheBoundsInvalid,
    /// Timeout, retry, or size was zero or outside the documented bound.
    LimitInvalid,
    /// Artifact bytes or metadata failed integrity verification.
    ArtifactIntegrityError,
    /// Requested Worker does not exist in the instance.
    WorkerNotFound,
    /// A live Worker already owns the requested name.
    WorkerNameConflict,
    /// The Worker has been tombstoned.
    WorkerDeleted,
    /// Requested version does not exist for the Worker.
    VersionNotFound,
    /// Version is not ready for the requested operation.
    VersionNotReady,
    /// Version is currently active and cannot be deleted.
    VersionActive,
    /// Version still has a live referrer or in-flight pin.
    VersionReferenced,
    /// A Service binding declaration is missing, forged, or outside its instance boundary.
    ServiceBindingDenied,
    /// The dynamically resolved target Worker has no callable active version.
    ServiceTargetNotReady,
    /// The resolved Service target cannot currently be reached.
    ServiceUnavailable,
    /// The selected named Service entrypoint is absent from the active version.
    ServiceEntrypointNotFound,
    /// One root Service invocation exhausted its depth, count, or concurrency budget.
    ServiceLimitExceeded,
    /// A Service invocation exceeded its bounded foreground deadline.
    ServiceTimeout,
    /// Another retained version still declares the Worker as a Service target.
    ServiceTargetReferenced,
    /// Immutable version metadata no longer matches its descriptor.
    VersionInvariantViolation,
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
    /// Public route did not resolve to a live active version.
    RouteNotFound,
    /// A live route already owns the canonical host and path prefix.
    RouteConflict,
    /// Requested named entrypoint does not exist.
    EntrypointNotFound,
    /// A version secret name or value is invalid.
    SecretInvalid,
    /// An idempotency key was reused with a different canonical request.
    IdempotencyConflict,
    /// workerd is not available for dispatch.
    RuntimeUnavailable,
    /// A runtime response started but its final result is unknown.
    RuntimeResultUnknown,
    /// A request or runtime resource limit was exceeded.
    ResourceLimitExceeded,
    /// Canonical static-asset manifest validation failed.
    AssetManifestInvalid,
    /// A static-asset logical URL path is invalid.
    AssetPathInvalid,
    /// A static-asset manifest, file, rule, or upload exceeded a fixed limit.
    AssetLimitExceeded,
    /// A version upload is missing one or more verified objects.
    AssetUploadIncomplete,
    /// An upload session or object conflicts with its immutable input identity.
    AssetUploadConflict,
    /// A referenced static-asset object failed integrity verification.
    AssetIntegrityError,
    /// The configured static-asset object provider is unavailable.
    AssetStorageUnavailable,
    /// Static-asset routing or rule configuration is unsupported.
    AssetConfigUnsupported,
    /// Requested resource does not exist in the authorized instance.
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
    /// R2 object key exceeds Cloudflare's 1024-byte UTF-8 limit.
    R2KeyTooLarge,
    /// R2 range, conditional, metadata, or list options are invalid.
    R2InvalidOptions,
    /// Caller-supplied R2 checksum bytes do not match the staged object.
    R2ChecksumMismatch,
    /// SSE-C key material is malformed or does not match the stored object.
    R2SsecInvalid,
    /// Multipart upload identity, part set, or lifecycle state is invalid.
    R2MultipartInvalid,
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
    /// A D1 session bookmark or constraint is malformed, forged, or not valid for this database.
    D1SessionError,
    /// `dump()` is rejected for the current non-alpha D1 database model.
    D1DumpError,
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
    /// A tenant D1 file belongs to a different instance or resource.
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
    DoVersionStale,
    /// The active version no longer exports the namespace class.
    DoClassNotFound,
    /// Native Durable Object storage or its local disk is unavailable.
    DoStorageUnavailable,
    /// Durable Object local-disk capacity policy rejected a write or new identity.
    DoStorageLimit,
    /// Durable Object dispatch exceeded its bounded foreground deadline.
    DoDispatchTimeout,
    /// The requested Durable Object RPC member or value is not serializable or callable.
    DoRpcUnsupported,
    /// Tenant Durable Object code raised an opaque runtime exception.
    DoRuntimeException,
    /// A namespace still owns live objects and force deletion was not requested.
    DoNamespaceNotEmpty,
    /// The private Durable Object transport protocol was malformed.
    DoInternalProtocolError,
    /// The independent scheduler database or dispatcher is unavailable.
    SchedulerUnavailable,
    /// The independent scheduler database failed integrity validation.
    SchedulerCorrupt,
    /// The bounded scheduler writer lane is temporarily busy.
    SchedulerBusy,
    /// The private alarm projection or dispatch protocol was malformed.
    SchedulerInternalProtocolError,
    /// The requested fixed scheduler workload is not enabled in this release.
    SchedulerKindNotEnabled,
    /// A Durable Object alarm authority mutation could not update its projection.
    DoAlarmIndexUnavailable,
    /// Requested Queue does not exist in the authorized instance.
    QueueNotFound,
    /// A live Queue already owns the requested display name.
    QueueNameConflict,
    /// Queue lifecycle does not currently admit the requested operation.
    QueueNotReady,
    /// Queue configuration is fenced while scheduler projection converges.
    QueueConfigPending,
    /// Queue still has a producer, consumer, or dead-letter referrer.
    QueueReferenced,
    /// A non-force delete was requested for a Queue with retained messages.
    QueueNotEmpty,
    /// Queue content type is unknown or deliberately unsupported.
    QueueContentTypeUnsupported,
    /// Queue message, body type, JSON value, or iterable is invalid.
    QueueInvalidMessage,
    /// One serialized Queue message exceeds the capability limit.
    QueueMessageTooLarge,
    /// Queue batch count or serialized body total exceeds its capability limit.
    QueueBatchLimitExceeded,
    /// Queue delivery delay is not an integer in the supported range.
    QueueDelayInvalid,
    /// Queue-local durable backlog capacity would be exceeded.
    QueueBacklogLimitExceeded,
    /// Queue scheduler storage is temporarily unavailable.
    QueueStorageUnavailable,
    /// Queue mutation may have committed before its response was observed.
    QueueSendResultUnknown,
    /// A Queue already has a non-tombstoned push consumer.
    QueueConsumerConflict,
    /// Queue consumer control or scheduler authority is not ready for dispatch.
    QueueConsumerNotReady,
    /// Queue consumer projection has not converged to control authority.
    QueueConsumerProjectionPending,
    /// A Queue claim or completion references an obsolete consumer generation.
    QueueConsumerGenerationStale,
    /// Native Queue disposition contains an invalid or forged decision.
    QueueDispositionInvalid,
    /// Queue retry delay is outside the supported integer range.
    QueueRetryDelayInvalid,
    /// A dead-letter Queue target violates identity or lifecycle policy.
    QueueDlqInvalid,
    /// A terminal message is retained while its dead-letter target is backpressured.
    QueueDlqBackpressured,
    /// The pinned runtime cannot provide the native Queue custom event contract.
    QueueCustomEventUnsupported,
    /// Cron expression is syntactically invalid.
    CronExpressionInvalid,
    /// Cron expression uses syntax outside the published local capability.
    CronExpressionUnsupported,
    /// Cron scheduler projection has not converged to control authority.
    CronProjectionPending,
    /// A Cron run or completion references an obsolete activation generation.
    CronActivationStale,
    /// The pinned runtime cannot provide the native scheduled custom event contract.
    CronCustomEventUnsupported,
    /// Queue catalog, binding, projection, or counter authority is inconsistent.
    QueueInvariantViolation,
    /// Requested Workflow definition does not exist in the authorized instance.
    WorkflowNotFound,
    /// Workflow lifecycle does not admit this operation.
    WorkflowNotReady,
    /// The frozen Workflow version failed runtime validation or is not ready.
    WorkflowVersionNotReady,
    /// Workflow binding identity, descriptor, or lifecycle generation is stale.
    WorkflowBindingStale,
    /// Caller and frozen Workflow execution capabilities disagree.
    WorkflowCapabilityMismatch,
    /// The current instance state cannot accept the requested lifecycle transition.
    WorkflowInstanceStateConflict,
    /// An instance already has an unfinished cross-database operation.
    WorkflowInstanceBusy,
    /// Expired history is awaiting proven cleanup before identity can be reused.
    WorkflowInstanceCleanupPending,
    /// Event type is outside the supported bounded ASCII alphabet.
    WorkflowEventTypeInvalid,
    /// An instance's bounded durable event inbox cannot admit another event.
    WorkflowEventQueueFull,
    /// A callback attempt reached its authoritative deadline.
    WorkflowStepTimeout,
    /// No additional business retry is available after the last failed attempt.
    WorkflowStepRetriesExhausted,
    /// The callback explicitly reported a native non-retryable failure.
    WorkflowNonRetryable,
    /// An event wait reached its deadline without an eligible committed event.
    WorkflowEventTimeout,
    /// External Workflow instance identity violates the capability validator.
    WorkflowInstanceIdInvalid,
    /// This definition already reserved the external instance identity.
    WorkflowInstanceAlreadyExists,
    /// Workflow instance does not exist in the authorized definition.
    WorkflowInstanceNotFound,
    /// Canonical Workflow input exceeds the byte limit.
    WorkflowPayloadTooLarge,
    /// Canonical Workflow result exceeds the byte limit.
    WorkflowResultTooLarge,
    /// Workflow JSON value or depth is outside the supported subset.
    WorkflowSerializationUnsupported,
    /// Workflow instance or instance durable-state quota would be exceeded.
    WorkflowStateQuotaExceeded,
    /// Workflow step count exceeds the configured local limit.
    WorkflowStepLimitExceeded,
    /// Workflow retry, timeout, or rollback overload is unsupported.
    WorkflowStepConfigUnsupported,
    /// Workflow duration or absolute timestamp is outside the supported grammar or range.
    WorkflowDurationInvalid,
    /// Workflow method is outside the selected caller or execution capability.
    WorkflowMethodUnsupported,
    /// Replay descriptor or completed frontier differs from durable history.
    WorkflowNonDeterministic,
    /// Workflow run generation, token, or lease is no longer current.
    WorkflowRunStale,
    /// Workflow step token is no longer current.
    WorkflowStepStale,
    /// Workflow transport or persistence outcome is unknown.
    WorkflowRuntimeUnavailable,
    /// Workflow tenant code failed with a sanitized known outcome.
    WorkflowExecutionFailed,
    /// Workflow definition or version still has a live referrer.
    WorkflowReferenced,
    /// Workflow durable identity, descriptor, or state is inconsistent.
    WorkflowInvariantViolation,
    /// A live Workflow definition already owns the requested instance-scoped name.
    WorkflowNameConflict,
    /// A Cache API key, URL, namespace, method, or option is invalid.
    CacheKeyInvalid,
    /// Cache API rejected a response that cannot be stored.
    CachePutRejected,
    /// A cache body, header, variant, tag, or quota limit was exceeded.
    CacheLimitExceeded,
    /// The cache metadata or body authority is temporarily unavailable.
    CacheUnavailable,
    /// A per-Worker cache database or entry failed integrity validation.
    CacheCorrupt,
    /// The private cache facade protocol was malformed.
    CacheProtocolError,
    /// A cache mutation may have committed before its result was observed.
    CacheResultUnknown,
    /// Image bytes are malformed, truncated, or not a supported raster image.
    ImageInputInvalid,
    /// The requested image input or output codec is unsupported.
    ImageFormatUnsupported,
    /// The requested image transform option is unsupported or invalid.
    ImageOptionUnsupported,
    /// Image bytes, dimensions, pixels, operations, overlays, or concurrency exceeded a limit.
    ImageLimitExceeded,
    /// Image execution exceeded its bounded deadline.
    ImageTimeout,
    /// The native image engine or its bounded executor is unavailable.
    ImageUnavailable,
    /// The private Images facade protocol was malformed.
    ImageProtocolError,
    /// Document bytes, file identity, or detected container are malformed.
    DocumentInputInvalid,
    /// The document format is outside the verified Markdown Conversion allowlist.
    DocumentFormatUnsupported,
    /// Fixed document OCR assets or the OCR engine are unavailable.
    DocumentOcrUnavailable,
    /// A normalized image cannot fit the configured VLM input envelope.
    DocumentVisionInputTooLarge,
    /// The document is encrypted and no password surface is supported.
    DocumentEncrypted,
    /// The document has no indexable text content.
    DocumentEmpty,
    /// A document or batch exceeded a fixed byte, output, or concurrency limit.
    DocumentLimitExceeded,
    /// A requested Markdown Conversion option is unsupported or invalid.
    DocumentOptionUnsupported,
    /// The isolated parser child exceeded its fixed wall deadline.
    DocumentTimeout,
    /// The isolated parser child or native parser is unavailable.
    DocumentUnavailable,
    /// The isolated parser child exited or violated its bounded output contract.
    DocumentProcessFailed,
    /// The private Markdown Conversion or parser-child protocol was malformed.
    DocumentProtocolError,
    /// A supported document could not be converted by the frozen parser contract.
    DocumentParseFailed,
    /// A valid document contained no text suitable for indexing.
    DocumentNoExtractableText,
    /// A secret-safe internal P0.2 failure.
    Internal,
    /// A local operator instance ID is malformed or inconsistent with its digest.
    InstanceIdInvalid,
    /// The requested instance does not exist or is unavailable.
    InstanceNotFound,
    /// Multiple running instances match and no explicit selector was provided.
    InstanceAmbiguous,
    /// The local instance registry is missing, corrupt, or fails closed checks.
    InstanceRegistryInvalid,
    /// A remote Cf target name, URL, instance, or record is invalid.
    TargetInvalid,
    /// The requested remote Cf target does not exist.
    TargetNotFound,
    /// The per-user remote target registry failed closed validation.
    TargetRegistryInvalid,
    /// The selected Cf executable or certified version is invalid.
    CfInvalid,
    /// Browser backend or its qualified host security boundary is unavailable.
    BrowserUnavailable,
    /// Browser route, option, or CDP method is outside the fixed capability inventory.
    BrowserUnsupported,
    /// Browser request does not match the fixed protocol.
    BrowserInputInvalid,
    /// Browser capacity, body, result, or message bound is exceeded.
    BrowserLimitExceeded,
    /// Browser command or action deadline elapsed.
    BrowserTimeout,
    /// Browser session is absent, closed, or belongs to another generation.
    BrowserSessionNotFound,
    /// A legacy project requires explicit official migration.
    WranglerProjectUnsupported,
}

mod code;

mod platform;

pub use platform::{PlatformError, ReadinessReason};

#[cfg(test)]
#[path = "error_tests.rs"]
mod tests;
