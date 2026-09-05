//! Strict P0.1 TOML configuration types and static validation.
//!
//! Parsing never reads `.env`, the current directory, `$HOME`, or secret
//! values. Secret references stay symbolic until a later crate resolves them.

use crate::error::{ErrorCode, PlatformError};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};
use url::Url;

mod ai;
mod scheduler;
pub use ai::{
    AiAuthConfig, AiConfig, AiEmbeddingMetric, AiEmbeddingModelConfig, AiGenerationCapability,
    AiGenerationModelConfig, AiProviderConfig, AiTokenizer, AiTokenizerArtifactConfig,
    ResolvedEmbeddingModelContract, ResolvedTokenizerContract,
};
pub use scheduler::{SchedulerConfig, SchedulerPoolConfig, SchedulerPoolsConfig};

const DEFAULT_PUBLIC_BIND: &str = "127.0.0.1:8787";
const DEFAULT_DATA_DIR: &str = "/var/lib/open-compute";
const DEFAULT_MASTER_KEY_FILE: &str = "/var/lib/open-compute/keys/master.key";
const DEFAULT_OBJECT_DIR: &str = "/var/lib/open-compute/objects";
const DEFAULT_S3_ENDPOINT: &str = "https://s3.example.com";
const DEFAULT_S3_REGION: &str = "auto";
const DEFAULT_S3_BUCKET: &str = "open-compute";
const DEFAULT_OBJECT_PREFIX: &str = "system/";
const DEFAULT_R2_OBJECT_PREFIX: &str = "tenant/r2/";
const DATA_LOCK_FILE_NAME: &str = "platform.lock";

/// Top-level platform configuration.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PlatformConfig {
    /// HTTP listeners and admin auth.
    #[serde(default)]
    pub server: ServerConfig,
    /// Data directory, keys, and database bounds.
    #[serde(rename = "data")]
    pub data: DataConfig,
    /// Object storage authority.
    #[serde(rename = "storage")]
    pub object_storage: ObjectStorageConfig,
    /// Embedded workerd supervisor budgets.
    #[serde(default)]
    pub runtime: RuntimeConfig,
    /// Local artifact cache.
    #[serde(default)]
    pub cache: CacheConfig,
    /// Workers Cache and Cache API authority limits.
    #[serde(default)]
    pub response_cache: ResponseCacheConfig,
    /// Native Images binding execution limits.
    #[serde(default)]
    pub images: ImagesConfig,
    /// Isolated document parser and Markdown Conversion limits.
    #[serde(default)]
    pub document_parser: DocumentParserConfig,
    /// Operator-owned model providers and immutable AI model catalog.
    #[serde(default)]
    pub ai: AiConfig,
    /// Bounded metrics export.
    #[serde(default)]
    pub metrics: MetricsConfig,
    /// Workers Logs persistence, query, and realtime-tail capacity.
    #[serde(default)]
    pub observability: ObservabilityConfig,
    /// P1 platform-wide admission, resource-count, snapshot, and recovery limits.
    #[serde(default)]
    pub hardening: HardeningConfig,
    /// Worker ingress, deletion, and artifact retention policy.
    #[serde(default)]
    pub workers: WorkersConfig,
    /// Workers KV local database, connection, and stream limits.
    #[serde(default)]
    pub kv: KvConfig,
    /// Workers R2 object, staging, and concurrency limits.
    #[serde(default)]
    pub r2: R2Config,
    /// Workers D1 SQLite, result, and concurrency limits.
    #[serde(default)]
    pub d1: D1Config,
    /// Queue producer backlog and request-admission limits.
    #[serde(default)]
    pub queues: QueuesConfig,
    /// Workflow sequential execution, leases, and local retained-state capacity.
    #[serde(default)]
    pub workflows: crate::WorkflowsConfig,
    /// Durable Object identity, dispatch, RPC, and local-disk policy.
    #[serde(default)]
    pub durable_objects: DurableObjectsConfig,
    /// Durable Object alarm scheduler policy.
    #[serde(default)]
    pub scheduler: SchedulerConfig,
    /// Optional operator dashboard settings.
    #[serde(default)]
    pub dashboard: DashboardConfig,
}

impl PlatformConfig {
    /// Parse TOML without resolving secrets or reading the environment.
    pub fn from_toml_str(toml: &str) -> Result<Self, PlatformError> {
        let mut config: Self = toml::from_str(toml).map_err(|_| {
            PlatformError::new(ErrorCode::ConfigParseFailed, "invalid platform config TOML")
        })?;
        config.object_storage.normalize_implicit_env_defaults();
        config.validate()?;
        Ok(config)
    }

    /// Parse TOML, resolve every host path against `config_base`, then validate.
    pub fn from_toml_str_at(toml: &str, config_base: &Path) -> Result<Self, PlatformError> {
        require_absolute(config_base, "config_base")?;
        let mut config: Self = toml::from_str(toml).map_err(|_| {
            PlatformError::new(ErrorCode::ConfigParseFailed, "invalid platform config TOML")
        })?;
        config.resolve_paths(config_base)?;
        config.object_storage.normalize_implicit_env_defaults();
        config.validate()?;
        Ok(config)
    }

    /// Static validation. Does not touch the filesystem or environment.
    pub fn validate(&self) -> Result<(), PlatformError> {
        self.server.validate()?;
        self.data.validate()?;
        self.object_storage.validate()?;
        if let Some(local) = self.object_storage.as_local() {
            validate_local_object_root(&self.data, local)?;
        }
        self.runtime.validate()?;
        self.cache.validate()?;
        self.response_cache.validate()?;
        self.images.validate()?;
        self.document_parser.validate()?;
        self.ai.validate()?;
        self.metrics.validate()?;
        self.observability.validate()?;
        self.hardening.validate()?;
        if self.hardening.emergency_reserve_bytes >= self.data.free_space_hard_bytes {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "hardening.emergency_reserve_bytes must be below the storage hard reserve",
            ));
        }
        self.workers.validate()?;
        self.kv.validate()?;
        self.r2.validate()?;
        self.d1.validate()?;
        self.queues.validate()?;
        self.workflows.validate()?;
        self.durable_objects.validate()?;
        self.scheduler.validate()?;
        self.dashboard.validate();
        Ok(())
    }

    fn resolve_paths(&mut self, base: &Path) -> Result<(), PlatformError> {
        self.data.path = resolve_host_path(base, &self.data.path)?;
        self.data.master_key_file = resolve_host_path(base, &self.data.master_key_file)?;
        resolve_secret_path(base, &mut self.server.admin_auth)?;
        resolve_secret_path(base, &mut self.server.deployer_auth)?;
        resolve_secret_path(base, &mut self.server.read_only_auth)?;
        self.object_storage.resolve_paths(base)?;
        self.ai.resolve_paths(base)?;
        Ok(())
    }

    /// Explicit local fixture used only by repository tests.
    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn local_test_config() -> Self {
        Self {
            server: ServerConfig::default(),
            data: DataConfig::default(),
            object_storage: ObjectStorageConfig::Local(LocalObjectStorageConfig::default()),
            runtime: RuntimeConfig::default(),
            cache: CacheConfig::default(),
            response_cache: ResponseCacheConfig::default(),
            images: ImagesConfig::default(),
            document_parser: DocumentParserConfig::default(),
            ai: AiConfig::default(),
            metrics: MetricsConfig::default(),
            observability: ObservabilityConfig::default(),
            hardening: HardeningConfig::default(),
            workers: WorkersConfig::default(),
            kv: KvConfig::default(),
            r2: R2Config::default(),
            d1: D1Config::default(),
            queues: QueuesConfig::default(),
            workflows: crate::WorkflowsConfig::default(),
            durable_objects: DurableObjectsConfig::default(),
            scheduler: SchedulerConfig::default(),
            dashboard: DashboardConfig::default(),
        }
    }
}

/// P1 platform-wide limits that protect a single-node host.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, default)]
pub struct HardeningConfig {
    /// Maximum live Workers owned by one account.
    pub max_workers_per_account: u32,
    /// Maximum live routes owned by one account.
    pub max_routes_per_account: u32,
    /// Maximum retained versions owned by one Worker.
    pub max_versions_per_worker: u32,
    /// Maximum live resources of one product kind owned by one account.
    pub max_resources_per_kind_per_account: u32,
    /// Bytes retained exclusively for delete, cleanup, and bounded diagnostics.
    pub emergency_reserve_bytes: u64,
    /// Maximum files accepted in one platform snapshot.
    pub max_snapshot_files: u32,
    /// Maximum bytes accepted for one snapshot file.
    pub max_snapshot_file_bytes: u64,
    /// Maximum aggregate bytes accepted in one snapshot.
    pub max_snapshot_total_bytes: u64,
    /// Maximum canonical manifest bytes accepted from object storage.
    pub max_snapshot_manifest_bytes: u64,
    /// Additional local headroom required while staging a snapshot or restore.
    pub snapshot_staging_margin_bytes: u64,
    /// Age before an owned incomplete snapshot prefix may be reclaimed.
    pub incomplete_snapshot_grace_ms: u64,
    /// Age after which the most recent committed snapshot degrades operator health.
    pub snapshot_stale_after_ms: u64,
    /// Maximum bytes written to one local support bundle.
    pub max_support_bundle_bytes: u64,
}

impl Default for HardeningConfig {
    fn default() -> Self {
        Self {
            max_workers_per_account: 1_000,
            max_routes_per_account: 10_000,
            max_versions_per_worker: 1_000,
            max_resources_per_kind_per_account: 1_000,
            emergency_reserve_bytes: 64 * 1024 * 1024,
            max_snapshot_files: 1_000_000,
            max_snapshot_file_bytes: 64 * 1024 * 1024 * 1024,
            max_snapshot_total_bytes: 1024 * 1024 * 1024 * 1024,
            max_snapshot_manifest_bytes: 8 * 1024 * 1024,
            snapshot_staging_margin_bytes: 64 * 1024 * 1024,
            incomplete_snapshot_grace_ms: 24 * 60 * 60 * 1_000,
            snapshot_stale_after_ms: 7 * 24 * 60 * 60 * 1_000,
            max_support_bundle_bytes: 32 * 1024 * 1024,
        }
    }
}

impl HardeningConfig {
    fn validate(&self) -> Result<(), PlatformError> {
        if self.max_workers_per_account == 0
            || self.max_workers_per_account > 1_000_000
            || self.max_routes_per_account == 0
            || self.max_routes_per_account > 10_000_000
            || self.max_versions_per_worker == 0
            || self.max_versions_per_worker > 1_000_000
            || self.max_resources_per_kind_per_account == 0
            || self.max_resources_per_kind_per_account > 1_000_000
            || self.emergency_reserve_bytes == 0
            || self.max_snapshot_files == 0
            || self.max_snapshot_files > 10_000_000
            || self.max_snapshot_file_bytes == 0
            || self.max_snapshot_total_bytes < self.max_snapshot_file_bytes
            || self.max_snapshot_manifest_bytes == 0
            || self.max_snapshot_manifest_bytes > 64 * 1024 * 1024
            || self.snapshot_staging_margin_bytes == 0
            || self.incomplete_snapshot_grace_ms == 0
            || self.snapshot_stale_after_ms == 0
            || self.max_support_bundle_bytes == 0
        {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "P1 hardening policy is outside the platform bounds",
            ));
        }
        Ok(())
    }
}

/// Validate the operator-supplied `--config` bootstrap path before resolution.
pub fn validate_bootstrap_config_path(path: &Path) -> Result<(), PlatformError> {
    if path.as_os_str().is_empty() {
        return Err(PlatformError::new(
            ErrorCode::ConfigPathInvalid,
            "bootstrap --config path must not be empty",
        ));
    }
    Ok(())
}

/// Operator dashboard settings.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, default)]
pub struct DashboardConfig {
    /// Whether the static dashboard is served at `/operator/`.
    pub enabled: bool,
}

impl DashboardConfig {
    fn validate(&self) {}
}

/// Public/admin bind addresses and admin authentication.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct ServerConfig {
    /// Public worker/health bind address.
    pub public_bind: String,
    /// Optional dedicated admin bind. Empty means the public listener.
    pub admin_bind: Option<String>,
    /// Required admin auth secret reference.
    pub admin_auth: SecretReference,
    /// Required Worker/resource deployment token reference.
    pub deployer_auth: SecretReference,
    /// Required read-only catalog and status token reference.
    pub read_only_auth: SecretReference,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            public_bind: DEFAULT_PUBLIC_BIND.to_string(),
            admin_bind: None,
            admin_auth: SecretReference {
                env: Some("OPEN_COMPUTE_ADMIN_TOKEN".to_string()),
                file: None,
            },
            deployer_auth: SecretReference {
                env: Some("OPEN_COMPUTE_DEPLOYER_TOKEN".to_string()),
                file: None,
            },
            read_only_auth: SecretReference {
                env: Some("OPEN_COMPUTE_READ_ONLY_TOKEN".to_string()),
                file: None,
            },
        }
    }
}

impl ServerConfig {
    fn validate(&self) -> Result<(), PlatformError> {
        let public = parse_bind(&self.public_bind, "server.public_bind")?;
        let admin = match &self.admin_bind {
            Some(bind) if !bind.is_empty() => Some(parse_bind(bind, "server.admin_bind")?),
            _ => None,
        };
        let _admin_addr = admin.unwrap_or(public);
        self.admin_auth.validate("server.admin_auth")?;
        self.deployer_auth.validate("server.deployer_auth")?;
        self.read_only_auth.validate("server.read_only_auth")?;
        Ok(())
    }

    /// Parsed public bind address.
    pub fn public_addr(&self) -> Result<SocketAddr, PlatformError> {
        parse_bind(&self.public_bind, "server.public_bind")
    }

    /// Parsed dedicated admin bind, if configured.
    pub fn admin_addr(&self) -> Result<Option<SocketAddr>, PlatformError> {
        match &self.admin_bind {
            Some(bind) if !bind.is_empty() => Ok(Some(parse_bind(bind, "server.admin_bind")?)),
            _ => Ok(None),
        }
    }
}

/// Local platform data, key path, control database, and free-space settings.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct DataConfig {
    /// Absolute data root.
    pub path: PathBuf,
    /// Absolute master key file path.
    pub master_key_file: PathBuf,
    /// Optional env name that may also supply the master key.
    pub master_key_env: Option<String>,
    /// Control database `busy_timeout` in milliseconds.
    pub sqlite_busy_timeout_ms: u64,
    /// Soft free-space threshold in bytes; below this, status is degraded.
    pub free_space_soft_bytes: u64,
    /// Hard free-space threshold in bytes; below this, mutations are refused.
    pub free_space_hard_bytes: u64,
}

impl Default for DataConfig {
    fn default() -> Self {
        Self {
            path: PathBuf::from(DEFAULT_DATA_DIR),
            master_key_file: PathBuf::from(DEFAULT_MASTER_KEY_FILE),
            master_key_env: None,
            sqlite_busy_timeout_ms: 5_000,
            free_space_soft_bytes: 1_073_741_824,
            free_space_hard_bytes: 268_435_456,
        }
    }
}

impl DataConfig {
    fn validate(&self) -> Result<(), PlatformError> {
        require_absolute(&self.path, "data.path")?;
        require_absolute(&self.master_key_file, "data.master_key_file")?;
        if let Some(env) = &self.master_key_env {
            require_env_name(env, "data.master_key_env")?;
        }
        require_nonzero(self.sqlite_busy_timeout_ms, "data.sqlite_busy_timeout_ms")?;
        require_nonzero(self.free_space_soft_bytes, "data.free_space_soft_bytes")?;
        require_nonzero(self.free_space_hard_bytes, "data.free_space_hard_bytes")?;
        if self.free_space_hard_bytes > self.free_space_soft_bytes {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "data.free_space_hard_bytes must be <= data.free_space_soft_bytes",
            ));
        }
        Ok(())
    }

    /// Data-directory advisory lock path: `<data_dir>/platform.lock`.
    #[must_use]
    pub fn data_lock_path(&self) -> PathBuf {
        self.path.join(DATA_LOCK_FILE_NAME)
    }
}

/// S3-compatible object storage settings.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct S3Config {
    /// Service endpoint URL.
    pub endpoint: String,
    /// Region; `auto` is accepted.
    pub region: String,
    /// Bucket name.
    pub bucket: String,
    /// Use path-style addressing when true.
    pub force_path_style: bool,
    /// Verify TLS. P0 rejects `false` (fail closed).
    pub verify_tls: bool,
    /// Access key env var.
    pub access_key_id_env: Option<String>,
    /// Access key file.
    pub access_key_id_file: Option<PathBuf>,
    /// Secret key env var.
    pub secret_access_key_env: Option<String>,
    /// Secret key file.
    pub secret_access_key_file: Option<PathBuf>,
    /// Internal platform prefix, isolated from tenant prefixes.
    pub prefix: String,
    /// Tenant R2 namespace prefix, isolated from the internal platform prefix.
    pub r2_prefix: String,
    /// Bounded retry count.
    pub max_retries: u32,
    /// Initial retry backoff in milliseconds.
    pub retry_backoff_ms: u64,
    /// Connect timeout in milliseconds.
    pub connect_timeout_ms: u64,
    /// Request timeout in milliseconds.
    pub request_timeout_ms: u64,
}

impl Default for S3Config {
    fn default() -> Self {
        Self {
            endpoint: DEFAULT_S3_ENDPOINT.to_string(),
            region: DEFAULT_S3_REGION.to_string(),
            bucket: DEFAULT_S3_BUCKET.to_string(),
            force_path_style: true,
            verify_tls: true,
            access_key_id_env: Some("S3_ACCESS_KEY_ID".to_string()),
            access_key_id_file: None,
            secret_access_key_env: Some("S3_SECRET_ACCESS_KEY".to_string()),
            secret_access_key_file: None,
            prefix: DEFAULT_OBJECT_PREFIX.to_string(),
            r2_prefix: DEFAULT_R2_OBJECT_PREFIX.to_string(),
            max_retries: 3,
            retry_backoff_ms: 200,
            connect_timeout_ms: 5_000,
            request_timeout_ms: 30_000,
        }
    }
}

impl S3Config {
    /// Drop serde-injected default env names when file references are configured.
    pub fn normalize_implicit_env_defaults(&mut self) {
        // Partial S3 `[storage]` tables inherit serde defaults for env var names even when the
        // operator only configured file references. Drop those implicit defaults so
        // file-only configs do not also require matching process environment values.
        const DEFAULT_ACCESS_ENV: &str = "S3_ACCESS_KEY_ID";
        const DEFAULT_SECRET_ENV: &str = "S3_SECRET_ACCESS_KEY";
        if self.access_key_id_file.is_some()
            && self.access_key_id_env.as_deref() == Some(DEFAULT_ACCESS_ENV)
        {
            self.access_key_id_env = None;
        }
        if self.secret_access_key_file.is_some()
            && self.secret_access_key_env.as_deref() == Some(DEFAULT_SECRET_ENV)
        {
            self.secret_access_key_env = None;
        }
    }

    fn validate(&self) -> Result<(), PlatformError> {
        validate_s3_endpoint(&self.endpoint)?;
        if !self.verify_tls {
            return Err(PlatformError::new(
                ErrorCode::ConfigInvalid,
                "storage.verify_tls cannot be disabled",
            ));
        }
        if self.region.is_empty() {
            return Err(PlatformError::new(
                ErrorCode::ConfigInvalid,
                "storage.region must be non-empty",
            ));
        }
        if self.bucket.is_empty() {
            return Err(PlatformError::new(
                ErrorCode::ConfigInvalid,
                "storage.bucket must be non-empty",
            ));
        }
        validate_secret_pair(
            self.access_key_id_env.as_deref(),
            self.access_key_id_file.as_deref(),
            "storage.access_key_id",
        )?;
        validate_secret_pair(
            self.secret_access_key_env.as_deref(),
            self.secret_access_key_file.as_deref(),
            "storage.secret_access_key",
        )?;
        validate_object_prefixes(&self.prefix, &self.r2_prefix)?;
        require_nonzero(u64::from(self.max_retries), "storage.max_retries")?;
        require_nonzero(self.retry_backoff_ms, "storage.retry_backoff_ms")?;
        require_nonzero(self.connect_timeout_ms, "storage.connect_timeout_ms")?;
        require_nonzero(self.request_timeout_ms, "storage.request_timeout_ms")?;
        if self.request_timeout_ms < self.connect_timeout_ms {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "storage.request_timeout_ms must be >= storage.connect_timeout_ms",
            ));
        }
        Ok(())
    }
}

/// Direct local object-authority settings.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct LocalObjectStorageConfig {
    /// Absolute local object root.
    pub path: PathBuf,
    /// Internal platform object prefix.
    pub prefix: String,
    /// Tenant R2 object prefix.
    pub r2_prefix: String,
    /// Soft free-space threshold in bytes.
    pub free_space_soft_bytes: u64,
    /// Hard free-space threshold in bytes.
    pub free_space_hard_bytes: u64,
    /// Minimum age before a proven owned partial may be reclaimed on startup.
    pub partial_grace_ms: u64,
}

impl Default for LocalObjectStorageConfig {
    fn default() -> Self {
        Self {
            path: PathBuf::from(DEFAULT_OBJECT_DIR),
            prefix: DEFAULT_OBJECT_PREFIX.to_owned(),
            r2_prefix: DEFAULT_R2_OBJECT_PREFIX.to_owned(),
            free_space_soft_bytes: 1_073_741_824,
            free_space_hard_bytes: 268_435_456,
            partial_grace_ms: 3_600_000,
        }
    }
}

impl LocalObjectStorageConfig {
    fn validate(&self) -> Result<(), PlatformError> {
        require_absolute(&self.path, "storage.path")?;
        validate_object_prefixes(&self.prefix, &self.r2_prefix)?;
        require_nonzero(self.free_space_soft_bytes, "storage.free_space_soft_bytes")?;
        require_nonzero(self.free_space_hard_bytes, "storage.free_space_hard_bytes")?;
        require_nonzero(self.partial_grace_ms, "storage.partial_grace_ms")?;
        if self.free_space_hard_bytes > self.free_space_soft_bytes {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "storage.free_space_hard_bytes must be <= storage.free_space_soft_bytes",
            ));
        }
        Ok(())
    }
}

fn validate_local_object_root(
    data: &DataConfig,
    local: &LocalObjectStorageConfig,
) -> Result<(), PlatformError> {
    if local.path == Path::new("/") {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "local object root must not be the filesystem root",
        ));
    }
    let reserved = data.path.join("objects");
    let overlaps_data = local.path.starts_with(&data.path) || data.path.starts_with(&local.path);
    if overlaps_data && local.path != reserved {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "local object root must be data.path/objects or disjoint from data.path",
        ));
    }
    if data.master_key_file.starts_with(&local.path)
        || local.path.starts_with(&data.master_key_file)
    {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "local object root overlaps the master key path",
        ));
    }
    Ok(())
}

/// Exactly one configured object-byte authority.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "backend", rename_all = "snake_case", deny_unknown_fields)]
pub enum ObjectStorageConfig {
    /// Direct secure local filesystem authority.
    Local(LocalObjectStorageConfig),
    /// S3-compatible `SigV4` authority.
    S3(S3Config),
}

/// Stable low-cardinality object backend kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObjectStorageKind {
    /// Direct local filesystem backend.
    Local,
    /// S3-compatible backend.
    S3,
}

impl ObjectStorageKind {
    /// Stable configuration and observability token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::S3 => "s3",
        }
    }
}

impl ObjectStorageConfig {
    /// Selected backend kind.
    #[must_use]
    pub const fn kind(&self) -> ObjectStorageKind {
        match self {
            Self::Local(_) => ObjectStorageKind::Local,
            Self::S3(_) => ObjectStorageKind::S3,
        }
    }

    /// Canonical system prefix shared by every backend.
    #[must_use]
    pub fn prefix(&self) -> &str {
        match self {
            Self::Local(config) => &config.prefix,
            Self::S3(config) => &config.prefix,
        }
    }

    /// Canonical tenant R2 prefix shared by every backend.
    #[must_use]
    pub fn r2_prefix(&self) -> &str {
        match self {
            Self::Local(config) => &config.r2_prefix,
            Self::S3(config) => &config.r2_prefix,
        }
    }

    /// S3 settings when S3 is selected.
    #[must_use]
    pub const fn as_s3(&self) -> Option<&S3Config> {
        match self {
            Self::S3(config) => Some(config),
            Self::Local(_) => None,
        }
    }

    /// Mutable S3 settings when S3 is selected.
    #[must_use]
    pub const fn as_s3_mut(&mut self) -> Option<&mut S3Config> {
        match self {
            Self::S3(config) => Some(config),
            Self::Local(_) => None,
        }
    }

    /// Local settings when local storage is selected.
    #[must_use]
    pub const fn as_local(&self) -> Option<&LocalObjectStorageConfig> {
        match self {
            Self::Local(config) => Some(config),
            Self::S3(_) => None,
        }
    }

    fn normalize_implicit_env_defaults(&mut self) {
        if let Self::S3(config) = self {
            config.normalize_implicit_env_defaults();
        }
    }

    fn resolve_paths(&mut self, base: &Path) -> Result<(), PlatformError> {
        match self {
            Self::Local(config) => config.path = resolve_host_path(base, &config.path)?,
            Self::S3(config) => {
                resolve_optional_path(base, &mut config.access_key_id_file)?;
                resolve_optional_path(base, &mut config.secret_access_key_file)?;
            }
        }
        Ok(())
    }

    fn validate(&self) -> Result<(), PlatformError> {
        match self {
            Self::Local(config) => config.validate(),
            Self::S3(config) => config.validate(),
        }
    }
}

/// Supervisor budgets for the mandatory embedded workerd runtime.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct RuntimeConfig {
    /// Startup timeout in milliseconds.
    pub startup_timeout_ms: u64,
    /// SIGTERM grace period in milliseconds.
    pub shutdown_grace_ms: u64,
    /// Drain deadline in milliseconds before SIGTERM.
    pub drain_timeout_ms: u64,
    /// SIGKILL deadline after SIGTERM, in milliseconds.
    pub kill_timeout_ms: u64,
    /// Restart attempts allowed inside `restart_window_ms`.
    pub restart_budget: u32,
    /// Rolling restart window in milliseconds.
    pub restart_window_ms: u64,
    /// Initial restart backoff in milliseconds.
    pub restart_backoff_initial_ms: u64,
    /// Maximum restart backoff in milliseconds.
    pub restart_backoff_max_ms: u64,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            startup_timeout_ms: 20_000,
            shutdown_grace_ms: 10_000,
            drain_timeout_ms: 15_000,
            kill_timeout_ms: 5_000,
            restart_budget: 5,
            restart_window_ms: 60_000,
            restart_backoff_initial_ms: 200,
            restart_backoff_max_ms: 30_000,
        }
    }
}

impl RuntimeConfig {
    fn validate(&self) -> Result<(), PlatformError> {
        require_nonzero(self.startup_timeout_ms, "runtime.startup_timeout_ms")?;
        require_nonzero(self.shutdown_grace_ms, "runtime.shutdown_grace_ms")?;
        require_nonzero(self.drain_timeout_ms, "runtime.drain_timeout_ms")?;
        require_nonzero(self.kill_timeout_ms, "runtime.kill_timeout_ms")?;
        require_nonzero(u64::from(self.restart_budget), "runtime.restart_budget")?;
        require_nonzero(self.restart_window_ms, "runtime.restart_window_ms")?;
        require_nonzero(
            self.restart_backoff_initial_ms,
            "runtime.restart_backoff_initial_ms",
        )?;
        require_nonzero(
            self.restart_backoff_max_ms,
            "runtime.restart_backoff_max_ms",
        )?;
        if self.restart_backoff_initial_ms > self.restart_backoff_max_ms {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "runtime.restart_backoff_initial_ms must be <= runtime.restart_backoff_max_ms",
            ));
        }
        Ok(())
    }
}

/// Local artifact cache bounds.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct CacheConfig {
    /// Maximum cache size in bytes.
    pub max_bytes: u64,
    /// High watermark as a ratio of `max_bytes` (exclusive of 1.0).
    pub high_watermark_ratio: f64,
    /// Low watermark as a ratio of `max_bytes`. Must be < high.
    pub low_watermark_ratio: f64,
    /// Partial-file grace period in milliseconds.
    pub partial_grace_ms: u64,
    /// Maximum single artifact size in bytes.
    pub max_artifact_bytes: u64,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            max_bytes: 10_737_418_240,
            high_watermark_ratio: 0.90,
            low_watermark_ratio: 0.80,
            partial_grace_ms: 3_600_000,
            max_artifact_bytes: 536_870_912,
        }
    }
}

impl CacheConfig {
    fn validate(&self) -> Result<(), PlatformError> {
        require_nonzero(self.max_bytes, "cache.max_bytes")?;
        require_nonzero(self.partial_grace_ms, "cache.partial_grace_ms")?;
        require_nonzero(self.max_artifact_bytes, "cache.max_artifact_bytes")?;
        if !(self.low_watermark_ratio > 0.0 && self.low_watermark_ratio < 1.0) {
            return Err(PlatformError::new(
                ErrorCode::CacheBoundsInvalid,
                "cache.low_watermark_ratio must be in (0, 1)",
            ));
        }
        if !(self.high_watermark_ratio > 0.0 && self.high_watermark_ratio < 1.0) {
            return Err(PlatformError::new(
                ErrorCode::CacheBoundsInvalid,
                "cache.high_watermark_ratio must be in (0, 1)",
            ));
        }
        if self.low_watermark_ratio >= self.high_watermark_ratio {
            return Err(PlatformError::new(
                ErrorCode::CacheBoundsInvalid,
                "cache.low_watermark_ratio must be < cache.high_watermark_ratio",
            ));
        }
        if self.max_artifact_bytes > self.max_bytes {
            return Err(PlatformError::new(
                ErrorCode::CacheBoundsInvalid,
                "cache.max_artifact_bytes must be <= cache.max_bytes",
            ));
        }
        Ok(())
    }
}

/// Workers Cache and Cache API bounds for the single-node authority.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, default)]
pub struct ResponseCacheConfig {
    /// Maximum body bytes admitted for one cache entry.
    pub max_object_bytes: u64,
    /// Maximum logical body bytes retained by one Worker.
    pub max_bytes_per_worker: u64,
    /// Maximum canonical response-header bytes retained by one entry.
    pub max_header_bytes: u32,
    /// Maximum variants retained for one logical cache key.
    pub max_variants_per_key: u16,
    /// Maximum canonical tags retained by one entry.
    pub max_tags_per_entry: u16,
    /// Maximum UTF-8 bytes in one named-cache namespace.
    pub max_cache_name_bytes: u16,
    /// Maximum canonical URL bytes accepted as a cache key.
    pub max_url_bytes: u32,
    /// Maximum simultaneously open per-Worker cache databases.
    pub max_connections: u32,
    /// SQLite busy timeout in milliseconds.
    pub busy_timeout_ms: u64,
    /// Private backend request deadline in milliseconds.
    pub request_timeout_ms: u64,
    /// Refresh lease duration in milliseconds.
    pub refresh_lease_ms: u64,
    /// Maximum accepted freshness or stale lifetime in seconds.
    pub max_ttl_seconds: u64,
    /// Whether automatic-cache availability failures bypass to tenant code.
    pub fail_open: bool,
}

impl Default for ResponseCacheConfig {
    fn default() -> Self {
        Self {
            max_object_bytes: 16 * 1024 * 1024,
            max_bytes_per_worker: 1024 * 1024 * 1024,
            max_header_bytes: 32 * 1024,
            max_variants_per_key: 32,
            max_tags_per_entry: 64,
            max_cache_name_bytes: 128,
            max_url_bytes: 8 * 1024,
            max_connections: 128,
            busy_timeout_ms: 250,
            request_timeout_ms: 5_000,
            refresh_lease_ms: 30_000,
            max_ttl_seconds: 7 * 24 * 60 * 60,
            fail_open: true,
        }
    }
}

impl ResponseCacheConfig {
    fn validate(&self) -> Result<(), PlatformError> {
        for (value, name) in [
            (self.max_object_bytes, "response_cache.max_object_bytes"),
            (
                self.max_bytes_per_worker,
                "response_cache.max_bytes_per_worker",
            ),
            (
                u64::from(self.max_header_bytes),
                "response_cache.max_header_bytes",
            ),
            (
                u64::from(self.max_variants_per_key),
                "response_cache.max_variants_per_key",
            ),
            (
                u64::from(self.max_tags_per_entry),
                "response_cache.max_tags_per_entry",
            ),
            (
                u64::from(self.max_cache_name_bytes),
                "response_cache.max_cache_name_bytes",
            ),
            (
                u64::from(self.max_url_bytes),
                "response_cache.max_url_bytes",
            ),
            (
                u64::from(self.max_connections),
                "response_cache.max_connections",
            ),
            (self.busy_timeout_ms, "response_cache.busy_timeout_ms"),
            (self.request_timeout_ms, "response_cache.request_timeout_ms"),
            (self.refresh_lease_ms, "response_cache.refresh_lease_ms"),
            (self.max_ttl_seconds, "response_cache.max_ttl_seconds"),
        ] {
            require_nonzero(value, name)?;
        }
        if self.max_object_bytes > self.max_bytes_per_worker
            || self.max_object_bytes > 64 * 1024 * 1024
            || self.max_bytes_per_worker > 1024 * 1024 * 1024 * 1024
            || self.max_header_bytes > 64 * 1024
            || self.max_variants_per_key > 256
            || self.max_tags_per_entry > 256
            || self.max_cache_name_bytes > 256
            || self.max_url_bytes > 32 * 1024
            || self.max_connections > 1024
            || self.busy_timeout_ms > 5_000
            || self.request_timeout_ms > 60_000
            || self.refresh_lease_ms > 10 * 60 * 1_000
            || self.max_ttl_seconds > 365 * 24 * 60 * 60
        {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "response_cache limits are outside the supported bounds",
            ));
        }
        Ok(())
    }
}

/// Bounded native Images binding execution policy.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, default)]
pub struct ImagesConfig {
    /// Maximum bytes accepted for each image input.
    pub max_input_bytes: u64,
    /// Maximum encoded output bytes.
    pub max_output_bytes: u64,
    /// Maximum decoded pixels in one input or output image.
    pub max_pixels: u64,
    /// Maximum width or height in pixels.
    pub max_dimension: u32,
    /// Maximum transform operations in one chain.
    pub max_operations: u16,
    /// Maximum overlay images in one chain.
    pub max_overlays: u16,
    /// Maximum decoded frames; Day1 supports only non-animated raster inputs.
    pub max_frames: u16,
    /// Maximum in-flight image sessions retained by the process.
    pub max_sessions: u16,
    /// Maximum bytes retained across all in-flight image sessions.
    pub max_temp_bytes: u64,
    /// Idle image-session lifetime in milliseconds.
    pub session_ttl_ms: u64,
    /// Maximum concurrent transforms for the process.
    pub max_concurrency: u16,
    /// Maximum concurrent transforms for one account.
    pub max_concurrency_per_account: u16,
    /// End-to-end transform deadline in milliseconds.
    pub request_timeout_ms: u64,
}

impl Default for ImagesConfig {
    fn default() -> Self {
        Self {
            max_input_bytes: 20 * 1024 * 1024,
            max_output_bytes: 20 * 1024 * 1024,
            max_pixels: 40_000_000,
            max_dimension: 12_000,
            max_operations: 16,
            max_overlays: 8,
            max_frames: 1,
            max_sessions: 64,
            max_temp_bytes: 128 * 1024 * 1024,
            session_ttl_ms: 60_000,
            max_concurrency: 4,
            max_concurrency_per_account: 2,
            request_timeout_ms: 10_000,
        }
    }
}

impl ImagesConfig {
    fn validate(&self) -> Result<(), PlatformError> {
        for (value, name) in [
            (self.max_input_bytes, "images.max_input_bytes"),
            (self.max_output_bytes, "images.max_output_bytes"),
            (self.max_pixels, "images.max_pixels"),
            (u64::from(self.max_dimension), "images.max_dimension"),
            (u64::from(self.max_operations), "images.max_operations"),
            (u64::from(self.max_overlays), "images.max_overlays"),
            (u64::from(self.max_frames), "images.max_frames"),
            (u64::from(self.max_sessions), "images.max_sessions"),
            (self.max_temp_bytes, "images.max_temp_bytes"),
            (self.session_ttl_ms, "images.session_ttl_ms"),
            (u64::from(self.max_concurrency), "images.max_concurrency"),
            (
                u64::from(self.max_concurrency_per_account),
                "images.max_concurrency_per_account",
            ),
            (self.request_timeout_ms, "images.request_timeout_ms"),
        ] {
            require_nonzero(value, name)?;
        }
        if self.max_input_bytes > 20 * 1024 * 1024
            || self.max_output_bytes > 64 * 1024 * 1024
            || self.max_pixels > 100_000_000
            || self.max_dimension > 20_000
            || self.max_operations > 64
            || self.max_overlays > 32
            || self.max_frames != 1
            || self.max_sessions > 1024
            || self.max_temp_bytes > 4 * 1024 * 1024 * 1024
            || self.max_temp_bytes < self.max_input_bytes
            || self.session_ttl_ms > 10 * 60 * 1_000
            || self.max_concurrency > 256
            || self.max_concurrency_per_account > self.max_concurrency
            || self.request_timeout_ms > 120_000
        {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "images limits are outside the supported bounds",
            ));
        }
        Ok(())
    }
}

/// Bounded isolated document parser and Markdown Conversion policy.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, default)]
pub struct DocumentParserConfig {
    /// Maximum encoded bytes accepted for one document.
    pub max_input_bytes: u64,
    /// Maximum aggregate input or serialized-result bytes for one conversion call.
    pub max_batch_bytes: u64,
    /// Maximum documents accepted in one conversion call.
    pub max_batch_files: u16,
    /// Maximum normalized Markdown bytes returned for one document.
    pub max_output_bytes: u64,
    /// Maximum concurrent parser children for the process.
    pub max_concurrency: u16,
    /// Maximum concurrent parser children for one account.
    pub max_concurrency_per_account: u16,
    /// Maximum concurrent parser children for one immutable version.
    pub max_concurrency_per_version: u16,
    /// End-to-end parser child deadline in milliseconds.
    pub request_timeout_ms: u64,
    /// Maximum virtual address-space bytes available to one parser child.
    pub max_address_space_bytes: u64,
    /// Maximum CPU seconds available to one parser child.
    pub max_cpu_seconds: u64,
    /// Maximum bytes retained from child standard error for a content-free diagnostic.
    pub max_stderr_bytes: u64,
}

impl Default for DocumentParserConfig {
    fn default() -> Self {
        Self {
            max_input_bytes: 4 * 1024 * 1024,
            max_batch_bytes: 32 * 1024 * 1024,
            max_batch_files: 16,
            max_output_bytes: 16 * 1024 * 1024,
            max_concurrency: 4,
            max_concurrency_per_account: 2,
            max_concurrency_per_version: 1,
            request_timeout_ms: 30_000,
            max_address_space_bytes: 2 * 1024 * 1024 * 1024,
            max_cpu_seconds: 30,
            max_stderr_bytes: 64 * 1024,
        }
    }
}

impl DocumentParserConfig {
    fn validate(&self) -> Result<(), PlatformError> {
        for (value, name) in [
            (self.max_input_bytes, "document_parser.max_input_bytes"),
            (self.max_batch_bytes, "document_parser.max_batch_bytes"),
            (
                u64::from(self.max_batch_files),
                "document_parser.max_batch_files",
            ),
            (self.max_output_bytes, "document_parser.max_output_bytes"),
            (
                u64::from(self.max_concurrency),
                "document_parser.max_concurrency",
            ),
            (
                u64::from(self.max_concurrency_per_account),
                "document_parser.max_concurrency_per_account",
            ),
            (
                u64::from(self.max_concurrency_per_version),
                "document_parser.max_concurrency_per_version",
            ),
            (
                self.request_timeout_ms,
                "document_parser.request_timeout_ms",
            ),
            (
                self.max_address_space_bytes,
                "document_parser.max_address_space_bytes",
            ),
            (self.max_cpu_seconds, "document_parser.max_cpu_seconds"),
            (self.max_stderr_bytes, "document_parser.max_stderr_bytes"),
        ] {
            require_nonzero(value, name)?;
        }
        if self.max_input_bytes > 4 * 1024 * 1024
            || self.max_batch_bytes > 32 * 1024 * 1024
            || self.max_batch_bytes < self.max_input_bytes
            || self.max_batch_files > 16
            || self.max_output_bytes > 16 * 1024 * 1024
            || self.max_concurrency > 256
            || self.max_concurrency_per_account > self.max_concurrency
            || self.max_concurrency_per_version > self.max_concurrency_per_account
            || self.request_timeout_ms > 30_000
            || !(64 * 1024 * 1024..=2 * 1024 * 1024 * 1024).contains(&self.max_address_space_bytes)
            || self.max_cpu_seconds > 30
            || self.max_stderr_bytes > 64 * 1024
        {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "document_parser limits are outside the supported bounds",
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
    fn validate(&self) -> Result<(), PlatformError> {
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
    fn validate(&self) -> Result<(), PlatformError> {
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
    /// Maximum request body bytes forwarded to a tenant Worker.
    pub max_request_body_bytes: u64,
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
            max_request_body_bytes: 16 * 1024 * 1024,
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
    fn validate(&self) -> Result<(), PlatformError> {
        require_nonzero(self.max_bundle_bytes, "workers.max_bundle_bytes")?;
        require_nonzero(
            self.max_request_body_bytes,
            "workers.max_request_body_bytes",
        )?;
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
            || self.max_request_body_bytes > 64 * 1024 * 1024
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
    fn validate(&self) -> Result<(), PlatformError> {
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

    fn validate(&self) -> Result<(), PlatformError> {
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

    fn validate(&self) -> Result<(), PlatformError> {
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
    fn validate(&self) -> Result<(), PlatformError> {
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
    fn validate(&self) -> Result<(), PlatformError> {
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

/// Env and/or absolute-file secret reference. Values are not loaded here.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SecretReference {
    /// Environment variable name.
    #[serde(default)]
    pub env: Option<String>,
    /// Absolute file path.
    #[serde(default)]
    pub file: Option<PathBuf>,
}

impl SecretReference {
    pub(super) fn validate(&self, field: &'static str) -> Result<(), PlatformError> {
        validate_secret_pair(self.env.as_deref(), self.file.as_deref(), field)
    }
}

fn resolve_secret_path(base: &Path, secret: &mut SecretReference) -> Result<(), PlatformError> {
    resolve_optional_path(base, &mut secret.file)
}

fn resolve_optional_path(base: &Path, path: &mut Option<PathBuf>) -> Result<(), PlatformError> {
    if let Some(value) = path {
        *value = resolve_host_path(base, value)?;
    }
    Ok(())
}

fn resolve_host_path(base: &Path, configured: &Path) -> Result<PathBuf, PlatformError> {
    if configured.as_os_str().is_empty() || !base.is_absolute() {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "configured filesystem path is invalid",
        ));
    }
    let candidate = if configured.is_absolute() {
        configured.to_path_buf()
    } else {
        base.join(configured)
    };
    let mut normalized = PathBuf::new();
    for component in candidate.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(PlatformError::new(
                        ErrorCode::PathInvalid,
                        "configured filesystem path escapes the filesystem root",
                    ));
                }
            }
            Component::Normal(segment) => normalized.push(segment),
        }
    }
    if !normalized.is_absolute() || normalized.as_os_str().is_empty() {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "configured filesystem path did not resolve to an absolute path",
        ));
    }
    Ok(normalized)
}

fn validate_secret_pair(
    env: Option<&str>,
    file: Option<&Path>,
    field: &'static str,
) -> Result<(), PlatformError> {
    match (env, file) {
        (None, None) => Err(PlatformError::new(
            ErrorCode::SecretRefInvalid,
            "secret reference requires env, file, or both",
        )),
        (Some(name), None) => require_env_name(name, field),
        (None, Some(path)) => require_absolute(path, field),
        (Some(name), Some(path)) => {
            require_env_name(name, field)?;
            require_absolute(path, field)
        }
    }
}

fn require_env_name(name: &str, _field: &'static str) -> Result<(), PlatformError> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        || name.starts_with(|c: char| c.is_ascii_digit())
    {
        return Err(PlatformError::new(
            ErrorCode::SecretRefInvalid,
            "secret env name is invalid",
        ));
    }
    Ok(())
}

fn require_absolute(path: &Path, _field: &'static str) -> Result<(), PlatformError> {
    if !path.is_absolute() {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "configured path must be an absolute path",
        ));
    }
    if has_parent_dir(path) {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "configured path must not contain '..'",
        ));
    }
    if path.as_os_str().is_empty() {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "configured path must be non-empty",
        ));
    }
    Ok(())
}

fn has_parent_dir(path: &Path) -> bool {
    path.components().any(|c| matches!(c, Component::ParentDir))
}

fn require_nonzero(value: u64, _field: &'static str) -> Result<(), PlatformError> {
    if value == 0 {
        return Err(PlatformError::new(
            ErrorCode::LimitInvalid,
            "configured limit must be greater than zero",
        ));
    }
    Ok(())
}

fn parse_bind(value: &str, _field: &'static str) -> Result<SocketAddr, PlatformError> {
    value.parse::<SocketAddr>().map_err(|_| {
        PlatformError::new(
            ErrorCode::ConfigInvalid,
            "bind address is not a valid socket address",
        )
    })
}

fn validate_s3_endpoint(endpoint: &str) -> Result<(), PlatformError> {
    let url = Url::parse(endpoint).map_err(|_| {
        PlatformError::new(
            ErrorCode::ConfigInvalid,
            "storage.endpoint must be a well-formed HTTP(S) URL",
        )
    })?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "storage.endpoint must be an http(s) URL",
        ));
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "storage.endpoint must include a host",
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "storage.endpoint must not include a username or password",
        ));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "storage.endpoint must not include a query or fragment",
        ));
    }
    Ok(())
}

fn validate_object_prefix(prefix: &str, _field: &'static str) -> Result<(), PlatformError> {
    if prefix.is_empty() || prefix.len() > 1024 || !prefix.ends_with('/') {
        return Err(PlatformError::new(
            ErrorCode::ObjectStoragePrefixInvalid,
            "storage prefix must be non-empty and end with '/'",
        ));
    }
    if prefix.starts_with('/')
        || prefix.contains('\\')
        || prefix
            .split('/')
            .any(|segment| segment == "." || segment == "..")
        || prefix[..prefix.len() - 1].split('/').any(|segment| {
            segment.is_empty()
                || segment.len() > 255
                || !segment.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric()
                        || matches!(byte, b'-' | b'_' | b'.' | b'=' | b'+' | b'@')
                })
        })
    {
        return Err(PlatformError::new(
            ErrorCode::ObjectStoragePrefixInvalid,
            "storage prefix must use canonical bounded ASCII path segments",
        ));
    }
    Ok(())
}

fn validate_object_prefixes(prefix: &str, r2_prefix: &str) -> Result<(), PlatformError> {
    validate_object_prefix(prefix, "storage.prefix")?;
    validate_object_prefix(r2_prefix, "storage.r2_prefix")?;
    if prefix.starts_with(r2_prefix) || r2_prefix.starts_with(prefix) {
        return Err(PlatformError::new(
            ErrorCode::ObjectStoragePrefixInvalid,
            "system and R2 object prefixes must be disjoint",
        ));
    }
    if prefix.starts_with("tenant/") {
        return Err(PlatformError::new(
            ErrorCode::ObjectStoragePrefixInvalid,
            "storage.prefix must stay isolated from tenant prefixes",
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
