use super::*;

/// Cloudflare Artifacts public Git origin and single-machine capacity.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, default)]
pub struct ArtifactsConfig {
    /// Explicit deployment-owned origin used to construct Git remotes.
    pub public_origin: String,
    /// Maximum bytes accepted by one Git RPC request.
    pub max_request_bytes: u64,
    /// Maximum on-disk bytes owned by one Git repository.
    pub max_repository_bytes: u64,
    /// Maximum bytes returned by one object or file read.
    pub max_object_response_bytes: u64,
    /// Maximum concurrent Git requests.
    pub max_concurrent_requests: u32,
    /// Maximum time deletion waits for active repository leases.
    pub lease_drain_timeout_ms: u64,
    /// Maximum wall time for one external repository import.
    pub import_timeout_ms: u64,
    /// Default repository token lifetime in seconds.
    pub token_ttl_seconds: u32,
    /// Maximum repository token lifetime in seconds.
    pub max_token_ttl_seconds: u32,
}

impl Default for ArtifactsConfig {
    fn default() -> Self {
        Self {
            public_origin: "http://127.0.0.1:8787".to_owned(),
            max_request_bytes: 256 * 1024 * 1024,
            max_repository_bytes: 10 * 1024 * 1024 * 1024,
            max_object_response_bytes: 64 * 1024 * 1024,
            max_concurrent_requests: 16,
            lease_drain_timeout_ms: 30_000,
            import_timeout_ms: 300_000,
            token_ttl_seconds: 24 * 60 * 60,
            max_token_ttl_seconds: 365 * 24 * 60 * 60,
        }
    }
}

impl ArtifactsConfig {
    pub(super) fn validate(&self) -> Result<(), PlatformError> {
        let origin = Url::parse(&self.public_origin).map_err(|_| {
            PlatformError::new(
                ErrorCode::ConfigInvalid,
                "artifacts.public_origin must be an absolute HTTP(S) origin",
            )
        })?;
        if !matches!(origin.scheme(), "http" | "https")
            || origin.host_str().is_none()
            || origin.username() != ""
            || origin.password().is_some()
            || origin.query().is_some()
            || origin.fragment().is_some()
            || origin.path() != "/"
        {
            return Err(PlatformError::new(
                ErrorCode::ConfigInvalid,
                "artifacts.public_origin must be an absolute HTTP(S) origin without credentials or path",
            ));
        }
        if self.max_request_bytes < 1024 * 1024
            || self.max_request_bytes > 16 * 1024 * 1024 * 1024
            || self.max_repository_bytes < self.max_request_bytes
            || self.max_repository_bytes > 1024 * 1024 * 1024 * 1024
            || self.max_object_response_bytes == 0
            || self.max_object_response_bytes > self.max_request_bytes
            || self.max_concurrent_requests == 0
            || self.max_concurrent_requests > 1024
            || !(1_000..=300_000).contains(&self.lease_drain_timeout_ms)
            || !(1_000..=3_600_000).contains(&self.import_timeout_ms)
            || self.token_ttl_seconds < 60
            || self.token_ttl_seconds > self.max_token_ttl_seconds
            || self.max_token_ttl_seconds > 365 * 24 * 60 * 60
        {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "Artifacts policy exceeds the bounded Day 1 contract",
            ));
        }
        Ok(())
    }
}

/// Bounded metrics export settings.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct MetricsConfig {
    /// Whether `/metrics` is enabled.
    pub enabled: bool,
    /// Maximum bytes stored in any label value.
    pub max_label_value_bytes: u64,
    /// Maximum distinct series the process will retain.
    pub max_series: u64,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_label_value_bytes: 64,
            max_series: 1024,
        }
    }
}

impl MetricsConfig {
    pub(super) fn validate(&self) -> Result<(), PlatformError> {
        require_nonzero(self.max_label_value_bytes, "metrics.max_label_value_bytes")?;
        require_nonzero(self.max_series, "metrics.max_series")?;
        Ok(())
    }
}

/// Bounded single-machine Workers Logs and realtime-tail policy.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, default)]
pub struct ObservabilityConfig {
    /// Maximum retained log age in milliseconds.
    pub retention_ms: u64,
    /// Hard byte ceiling for `observability.sqlite`.
    pub max_database_bytes: u64,
    /// Cloudflare-compatible maximum log bytes captured for one invocation.
    pub max_invocation_log_bytes: u64,
    /// Maximum invocation envelopes waiting for persistence.
    pub ingest_queue_events: u32,
    /// Maximum envelopes committed in one SQLite transaction.
    pub ingest_batch_events: u32,
    /// Maximum delay before a partial ingest batch is committed.
    pub ingest_flush_ms: u64,
    /// Maximum simultaneous realtime clients for one Script.
    pub max_tail_sessions_per_script: u16,
    /// Maximum queued frame bytes for one realtime client.
    pub tail_client_queue_bytes: u64,
    /// Maximum events returned by one telemetry query.
    pub query_max_events: u32,
    /// Maximum telemetry query timeframe in milliseconds.
    pub query_max_timeframe_ms: u64,
    /// Explicit externally reachable HTTP(S) origin used to build tail WebSocket URLs.
    pub external_control_origin: String,
    /// Lifetime of one process-local Script Tail session.
    pub tail_session_ttl_ms: u64,
}

impl Default for ObservabilityConfig {
    fn default() -> Self {
        Self {
            retention_ms: 7 * 24 * 60 * 60 * 1_000,
            max_database_bytes: 1024 * 1024 * 1024,
            max_invocation_log_bytes: 256 * 1024,
            ingest_queue_events: 8_192,
            ingest_batch_events: 256,
            ingest_flush_ms: 100,
            max_tail_sessions_per_script: 10,
            tail_client_queue_bytes: 1024 * 1024,
            query_max_events: 2_000,
            query_max_timeframe_ms: 7 * 24 * 60 * 60 * 1_000,
            external_control_origin: "http://127.0.0.1:8787".to_owned(),
            tail_session_ttl_ms: 60 * 60 * 1_000,
        }
    }
}

impl ObservabilityConfig {
    pub(super) fn validate(&self) -> Result<(), PlatformError> {
        const WEEK_MS: u64 = 7 * 24 * 60 * 60 * 1_000;
        if self.retention_ms == 0
            || self.retention_ms > WEEK_MS
            || self.max_database_bytes < 1024 * 1024
            || self.max_database_bytes > 1024 * 1024 * 1024 * 1024
            || self.max_invocation_log_bytes != 256 * 1024
            || self.ingest_queue_events == 0
            || self.ingest_queue_events > 1_000_000
            || self.ingest_batch_events == 0
            || self.ingest_batch_events > self.ingest_queue_events
            || self.ingest_flush_ms == 0
            || self.ingest_flush_ms > 60_000
            || self.max_tail_sessions_per_script == 0
            || self.max_tail_sessions_per_script > 10
            || self.tail_client_queue_bytes < 4_096
            || self.tail_client_queue_bytes > 64 * 1024 * 1024
            || self.query_max_events == 0
            || self.query_max_events > 2_000
            || self.query_max_timeframe_ms == 0
            || self.query_max_timeframe_ms > self.retention_ms
            || self.tail_session_ttl_ms < 10_000
            || self.tail_session_ttl_ms > 24 * 60 * 60 * 1_000
        {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "observability policy exceeds the bounded Day 1 contract",
            ));
        }
        let origin = Url::parse(&self.external_control_origin).map_err(|_| {
            PlatformError::new(
                ErrorCode::ConfigInvalid,
                "observability.external_control_origin must be an absolute HTTP(S) origin",
            )
        })?;
        if !matches!(origin.scheme(), "http" | "https")
            || origin.host_str().is_none()
            || origin.username() != ""
            || origin.password().is_some()
            || origin.query().is_some()
            || origin.fragment().is_some()
            || origin.path() != "/"
        {
            return Err(PlatformError::new(
                ErrorCode::ConfigInvalid,
                "observability.external_control_origin must be an HTTP(S) origin without credentials or a path",
            ));
        }
        Ok(())
    }
}

/// P0.2 Worker host-side limits and retention policy.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, default)]
pub struct WorkersConfig {
    /// Maximum canonical `WorkerBundleV1` bytes accepted by Control API.
    pub max_bundle_bytes: u64,
    /// Deadline for waiting on in-flight version pins during delete.
    pub delete_drain_timeout_ms: u64,
    /// Minimum remote artifact orphan age before deletion.
    pub artifact_gc_grace_ms: u64,
    /// Background artifact GC interval.
    pub artifact_gc_interval_ms: u64,
    /// Maximum versions finalized in one crash-recovery batch.
    pub delete_recovery_batch: u32,
    /// Number of newest ready versions retained per Worker.
    pub retain_ready_versions: u32,
    /// Number of newest rejected versions retained per Worker.
    pub retain_rejected_versions: u32,
    /// Minimum version age before automatic retention deletion.
    pub version_min_retention_ms: u64,
}

impl Default for WorkersConfig {
    fn default() -> Self {
        Self {
            max_bundle_bytes: 17 * 1024 * 1024,
            delete_drain_timeout_ms: 5_000,
            artifact_gc_grace_ms: 24 * 60 * 60 * 1_000,
            artifact_gc_interval_ms: 60_000,
            delete_recovery_batch: 64,
            retain_ready_versions: 10,
            retain_rejected_versions: 10,
            version_min_retention_ms: 24 * 60 * 60 * 1_000,
        }
    }
}

impl WorkersConfig {
    pub(super) fn validate(&self) -> Result<(), PlatformError> {
        require_nonzero(self.max_bundle_bytes, "workers.max_bundle_bytes")?;
        require_nonzero(
            self.delete_drain_timeout_ms,
            "workers.delete_drain_timeout_ms",
        )?;
        require_nonzero(self.artifact_gc_grace_ms, "workers.artifact_gc_grace_ms")?;
        require_nonzero(
            self.artifact_gc_interval_ms,
            "workers.artifact_gc_interval_ms",
        )?;
        require_nonzero(
            u64::from(self.delete_recovery_batch),
            "workers.delete_recovery_batch",
        )?;
        require_nonzero(
            u64::from(self.retain_ready_versions),
            "workers.retain_ready_versions",
        )?;
        require_nonzero(
            u64::from(self.retain_rejected_versions),
            "workers.retain_rejected_versions",
        )?;
        require_nonzero(
            self.version_min_retention_ms,
            "workers.version_min_retention_ms",
        )?;
        if self.max_bundle_bytes > 64 * 1024 * 1024
            || self.delete_recovery_batch > 10_000
            || self.retain_ready_versions > 10_000
            || self.retain_rejected_versions > 10_000
        {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "Worker host policy exceeds the hard platform ceiling",
            ));
        }
        Ok(())
    }
}

/// P0.4 Workers KV local storage and concurrency policy.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, default)]
pub struct KvConfig {
    /// Frozen per-namespace SQLite quota for newly created namespaces.
    pub namespace_quota_bytes: u64,
    /// Global maximum concurrently opened SQLite connections.
    pub max_connections: u32,
    /// Maximum read connections admitted for one namespace.
    pub max_readers_per_namespace: u32,
    /// Global maximum active value streams.
    pub max_active_streams: u32,
    /// Per-namespace maximum active value streams.
    pub max_active_streams_per_namespace: u32,
    /// Idle handle lifetime before it is eligible for eviction.
    pub idle_handle_ttl_ms: u64,
    /// Foreground KV operation timeout.
    pub operation_timeout_ms: u64,
}

impl Default for KvConfig {
    fn default() -> Self {
        Self {
            namespace_quota_bytes: 1024 * 1024 * 1024,
            max_connections: 64,
            max_readers_per_namespace: 2,
            max_active_streams: 16,
            max_active_streams_per_namespace: 4,
            idle_handle_ttl_ms: 60_000,
            operation_timeout_ms: 30_000,
        }
    }
}

impl KvConfig {
    pub(super) fn validate(&self) -> Result<(), PlatformError> {
        const MIN_QUOTA: u64 = 256 * 1024 * 1024;
        if self.namespace_quota_bytes < MIN_QUOTA
            || self.max_connections == 0
            || self.max_connections > 1024
            || self.max_readers_per_namespace == 0
            || self.max_readers_per_namespace > 64
            || self.max_active_streams == 0
            || self.max_active_streams > 1024
            || self.max_active_streams_per_namespace == 0
            || self.max_active_streams_per_namespace > self.max_active_streams
            || self.idle_handle_ttl_ms == 0
            || self.operation_timeout_ms == 0
        {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "KV host policy is outside the hard platform bounds",
            ));
        }
        Ok(())
    }
}

/// P0.5 Workers R2 staging, object, and concurrency policy.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, default)]
pub struct R2Config {
    /// Frozen maximum object size for newly created buckets.
    pub max_object_bytes: u64,
    /// Global maximum concurrent single-part uploads.
    pub max_concurrent_uploads: u32,
    /// Global maximum active download streams.
    pub max_concurrent_downloads: u32,
    /// Global maximum bytes admitted to secure upload staging.
    pub max_staging_bytes: u64,
    /// Maximum concurrent metadata HEAD requests used by list include.
    pub max_metadata_head_concurrency: u32,
    /// Foreground R2 operation timeout.
    pub operation_timeout_ms: u64,
    /// Lifetime of an opaque signed list cursor.
    pub cursor_ttl_ms: u64,
}

impl Default for R2Config {
    fn default() -> Self {
        Self {
            max_object_bytes: 512 * 1024 * 1024,
            max_concurrent_uploads: 4,
            max_concurrent_downloads: 16,
            max_staging_bytes: 2 * 1024 * 1024 * 1024,
            max_metadata_head_concurrency: 8,
            operation_timeout_ms: 30_000,
            cursor_ttl_ms: 15 * 60 * 1000,
        }
    }
}

impl R2Config {
    /// Provider-independent single-part hard ceiling used by P0.5.
    pub const MAX_OBJECT_BYTES_HARD: u64 = 5 * 1024 * 1024 * 1024 - 5 * 1024 * 1024;

    pub(super) fn validate(&self) -> Result<(), PlatformError> {
        if self.max_object_bytes == 0
            || self.max_object_bytes > Self::MAX_OBJECT_BYTES_HARD
            || self.max_concurrent_uploads == 0
            || self.max_concurrent_uploads > 1024
            || self.max_concurrent_downloads == 0
            || self.max_concurrent_downloads > 4096
            || self.max_staging_bytes < self.max_object_bytes
            || self.max_metadata_head_concurrency == 0
            || self.max_metadata_head_concurrency > 1024
            || self.operation_timeout_ms == 0
            || self.cursor_ttl_ms == 0
            || self.cursor_ttl_ms > 24 * 60 * 60 * 1000
        {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "R2 host policy is outside the hard platform bounds",
            ));
        }
        Ok(())
    }
}

/// P0.6 Workers D1 SQLite, result, and concurrency policy.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, default)]
pub struct D1Config {
    /// Frozen per-database quota for newly created databases.
    pub database_quota_bytes: u64,
    /// Global maximum open tenant database handles.
    pub max_open_databases: u32,
    /// Maximum queued operations admitted for one database.
    pub max_queued_operations_per_database: u32,
    /// Maximum materialized rows in one terminal operation.
    pub max_result_rows: u32,
    /// Maximum encoded result bytes in one terminal operation.
    pub max_result_bytes: u64,
    /// Maximum SQLite VM progress steps in one operation.
    pub max_vm_steps: u64,
    /// Single-query wall deadline.
    pub query_timeout_ms: u64,
    /// Whole-batch wall deadline.
    pub batch_timeout_ms: u64,
    /// Idle handle lifetime before LRU eviction eligibility.
    pub idle_handle_ttl_ms: u64,
}

impl D1Config {
    /// Hard product quota ceiling accepted by the local P0.6 implementation.
    pub const DATABASE_QUOTA_BYTES_HARD: u64 = 10 * 1024 * 1024 * 1024;
    /// Maximum result bytes accepted by configuration.
    pub const MAX_RESULT_BYTES_HARD: u64 = 64 * 1024 * 1024;

    pub(super) fn validate(&self) -> Result<(), PlatformError> {
        const MIN_QUOTA: u64 = 64 * 1024 * 1024;
        if self.database_quota_bytes < MIN_QUOTA
            || self.database_quota_bytes > Self::DATABASE_QUOTA_BYTES_HARD
            || self.max_open_databases == 0
            || self.max_open_databases > 1024
            || self.max_queued_operations_per_database == 0
            || self.max_queued_operations_per_database > 4096
            || self.max_result_rows == 0
            || self.max_result_rows > 1_000_000
            || self.max_result_bytes == 0
            || self.max_result_bytes > Self::MAX_RESULT_BYTES_HARD
            || self.max_vm_steps == 0
            || self.max_vm_steps > 1_000_000_000
            || self.query_timeout_ms == 0
            || self.query_timeout_ms > 5 * 60 * 1000
            || self.batch_timeout_ms == 0
            || self.batch_timeout_ms > 5 * 60 * 1000
            || self.idle_handle_ttl_ms == 0
            || self.idle_handle_ttl_ms > 24 * 60 * 60 * 1000
        {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "D1 host policy is outside the hard platform bounds",
            ));
        }
        Ok(())
    }
}

impl Default for D1Config {
    fn default() -> Self {
        Self {
            database_quota_bytes: 1024 * 1024 * 1024,
            max_open_databases: 32,
            max_queued_operations_per_database: 64,
            max_result_rows: 10_000,
            max_result_bytes: 8 * 1024 * 1024,
            max_vm_steps: 10_000_000,
            query_timeout_ms: 30_000,
            batch_timeout_ms: 30_000,
            idle_handle_ttl_ms: 60_000,
        }
    }
}

/// Queue producer and consumer local backlog and concurrency policy.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, default)]
pub struct QueuesConfig {
    /// Default durable serialized-body quota assigned to newly created Queues.
    pub default_max_backlog_bytes: u64,
    /// Global private Queue producer requests admitted concurrently.
    pub max_in_flight_requests: u32,
    /// Private producer requests admitted concurrently for one immutable binding.
    pub max_in_flight_requests_per_binding: u32,
    /// Maximum concurrency accepted in one immutable Queue consumer declaration.
    pub max_consumer_concurrency: u32,
}

impl QueuesConfig {
    pub(super) fn validate(&self) -> Result<(), PlatformError> {
        if self.default_max_backlog_bytes == 0
            || self.default_max_backlog_bytes > 1024 * 1024 * 1024 * 1024
            || self.max_in_flight_requests == 0
            || self.max_in_flight_requests > 4096
            || self.max_in_flight_requests_per_binding == 0
            || self.max_in_flight_requests_per_binding > self.max_in_flight_requests
            || self.max_consumer_concurrency == 0
            || self.max_consumer_concurrency > 4096
        {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "Queue host policy is outside the hard platform bounds",
            ));
        }
        Ok(())
    }
}

impl Default for QueuesConfig {
    fn default() -> Self {
        Self {
            default_max_backlog_bytes: 1024 * 1024 * 1024,
            max_in_flight_requests: 64,
            max_in_flight_requests_per_binding: 8,
            max_consumer_concurrency: 32,
        }
    }
}

/// P0.7 Durable Object identity, transport, and local-disk policy.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, default)]
pub struct DurableObjectsConfig {
    /// Maximum UTF-8 bytes accepted for a namespace display name.
    pub max_namespace_name_bytes: u32,
    /// Maximum UTF-8 bytes accepted by `idFromName()`.
    pub max_object_name_bytes: u32,
    /// Maximum forwarded fetch request body bytes.
    pub max_fetch_body_bytes: u64,
    /// Foreground dispatch timeout.
    pub dispatch_timeout_ms: u64,
    /// Global number of active Durable Object dispatches.
    pub max_in_flight_dispatches: u32,
    /// Percentage at which health becomes degraded.
    pub disk_high_watermark_percent: u8,
    /// Percentage at which new objects and writes fail closed.
    pub disk_stop_writes_percent: u8,
    /// Maximum objects processed in one reconciliation batch.
    pub reconcile_batch: u32,
}

impl Default for DurableObjectsConfig {
    fn default() -> Self {
        Self {
            max_namespace_name_bytes: 128,
            max_object_name_bytes: 1024,
            max_fetch_body_bytes: 32 * 1024 * 1024,
            dispatch_timeout_ms: 30_000,
            max_in_flight_dispatches: 256,
            disk_high_watermark_percent: 85,
            disk_stop_writes_percent: 95,
            reconcile_batch: 64,
        }
    }
}

impl DurableObjectsConfig {
    pub(super) fn validate(&self) -> Result<(), PlatformError> {
        if self.max_namespace_name_bytes == 0
            || self.max_namespace_name_bytes > 128
            || self.max_object_name_bytes == 0
            || self.max_object_name_bytes > 1024
            || self.max_fetch_body_bytes == 0
            || self.max_fetch_body_bytes > 64 * 1024 * 1024
            || self.dispatch_timeout_ms == 0
            || self.dispatch_timeout_ms > 5 * 60 * 1000
            || self.max_in_flight_dispatches == 0
            || self.max_in_flight_dispatches > 4096
            || self.disk_high_watermark_percent == 0
            || self.disk_high_watermark_percent >= self.disk_stop_writes_percent
            || self.disk_stop_writes_percent > 99
            || self.reconcile_batch == 0
            || self.reconcile_batch > 10_000
        {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "Durable Object host policy is outside the hard platform bounds",
            ));
        }
        Ok(())
    }
}
