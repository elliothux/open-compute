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
    /// Cloudflare Artifacts public origin and bounded Git capacity.
    #[serde(default)]
    pub artifacts: ArtifactsConfig,
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
        self.artifacts.validate()?;
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
            artifacts: ArtifactsConfig::default(),
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

mod object_storage;
mod product_limits;
mod runtime_products;

pub use object_storage::*;
pub use product_limits::*;
pub use runtime_products::*;

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
