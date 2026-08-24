//! Strict P0.1 TOML configuration types and static validation.
//!
//! Parsing never reads `.env`, the current directory, `$HOME`, or secret
//! values. Secret references stay symbolic until a later crate resolves them.

use crate::error::{ErrorCode, PlatformError};
use ipnet::IpNet;
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use url::Url;

const DEFAULT_PUBLIC_BIND: &str = "127.0.0.1:8787";
const DEFAULT_DATA_DIR: &str = "/var/lib/open-compute";
const DEFAULT_MASTER_KEY_FILE: &str = "/var/lib/open-compute/keys/master.key";
const DEFAULT_S3_ENDPOINT: &str = "https://s3.example.com";
const DEFAULT_S3_REGION: &str = "auto";
const DEFAULT_S3_BUCKET: &str = "open-compute";
const DEFAULT_S3_PREFIX: &str = "system/";
const DEFAULT_RUNTIME_BINARY: &str = "/opt/open-compute/bin/workerd";
const DEFAULT_RUNTIME_LOCK_FILE: &str = "/opt/open-compute/runtime/workerd.lock.json";
const DEFAULT_RUNTIME_ASSETS: &str = "/opt/open-compute/runtime";
const DATA_LOCK_FILE_NAME: &str = "platform.lock";

/// Top-level platform configuration.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct PlatformConfig {
    /// HTTP listeners and admin auth.
    pub server: ServerConfig,
    /// Data directory, keys, and database bounds.
    pub storage: StorageConfig,
    /// Object storage authority.
    pub s3: S3Config,
    /// workerd binary and supervisor budgets.
    pub runtime: RuntimeConfig,
    /// Local artifact cache.
    pub cache: CacheConfig,
    /// Bounded metrics export.
    pub metrics: MetricsConfig,
    /// Bounded diagnostic retention.
    pub diagnostics: DiagnosticsConfig,
    /// Worker ingress, deletion, and artifact retention policy.
    pub workers: WorkersConfig,
}

impl PlatformConfig {
    /// Parse TOML without resolving secrets or reading the environment.
    pub fn from_toml_str(toml: &str) -> Result<Self, PlatformError> {
        let config: Self = toml::from_str(toml).map_err(|_| {
            PlatformError::new(ErrorCode::ConfigParseFailed, "invalid platform config TOML")
        })?;
        config.validate()?;
        Ok(config)
    }

    /// Static validation. Does not touch the filesystem or environment.
    pub fn validate(&self) -> Result<(), PlatformError> {
        self.server.validate()?;
        self.storage.validate()?;
        self.s3.validate()?;
        self.runtime.validate()?;
        self.cache.validate()?;
        self.metrics.validate()?;
        self.diagnostics.validate()?;
        self.workers.validate()?;
        Ok(())
    }
}

/// Validate the `--config` bootstrap path: it must be absolute.
///
/// This does not read the file or search `$HOME` / the current directory.
pub fn validate_bootstrap_config_path(path: &Path) -> Result<(), PlatformError> {
    if !path.is_absolute() {
        return Err(PlatformError::new(
            ErrorCode::ConfigPathInvalid,
            "bootstrap --config path must be absolute",
        ));
    }
    if has_parent_dir(path) {
        return Err(PlatformError::new(
            ErrorCode::ConfigPathInvalid,
            "bootstrap --config path must not contain '..'",
        ));
    }
    Ok(())
}

/// Public/admin bind addresses and optional admin authentication.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct ServerConfig {
    /// Public worker/health bind address.
    pub public_bind: String,
    /// Optional dedicated admin bind. Empty means the public listener.
    pub admin_bind: Option<String>,
    /// Optional admin auth secret reference. Required when admin bind is non-loopback.
    pub admin_auth: Option<SecretReference>,
    /// Trusted proxy CIDRs; empty by default.
    pub trusted_proxies: Vec<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            public_bind: DEFAULT_PUBLIC_BIND.to_string(),
            admin_bind: None,
            admin_auth: None,
            trusted_proxies: Vec::new(),
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
        let admin_addr = admin.unwrap_or(public);
        if !is_loopback(admin_addr.ip()) {
            match &self.admin_auth {
                Some(reference) => reference.validate("server.admin_auth")?,
                None => {
                    return Err(PlatformError::new(
                        ErrorCode::AdminAuthRequired,
                        "non-loopback server.admin_bind requires explicit server.admin_auth",
                    ));
                }
            }
        } else if let Some(reference) = &self.admin_auth {
            reference.validate("server.admin_auth")?;
        }
        for proxy in &self.trusted_proxies {
            if proxy.parse::<IpNet>().is_err() {
                return Err(PlatformError::new(
                    ErrorCode::ConfigInvalid,
                    "server.trusted_proxies entries must be IPv4 or IPv6 CIDR prefixes",
                ));
            }
        }
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

/// Data directory, key path, control database, and free-space settings.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct StorageConfig {
    /// Absolute data root.
    pub data_dir: PathBuf,
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

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            data_dir: PathBuf::from(DEFAULT_DATA_DIR),
            master_key_file: PathBuf::from(DEFAULT_MASTER_KEY_FILE),
            master_key_env: None,
            sqlite_busy_timeout_ms: 5_000,
            free_space_soft_bytes: 1_073_741_824,
            free_space_hard_bytes: 268_435_456,
        }
    }
}

impl StorageConfig {
    fn validate(&self) -> Result<(), PlatformError> {
        require_absolute(&self.data_dir, "storage.data_dir")?;
        require_absolute(&self.master_key_file, "storage.master_key_file")?;
        if let Some(env) = &self.master_key_env {
            require_env_name(env, "storage.master_key_env")?;
        }
        require_nonzero(
            self.sqlite_busy_timeout_ms,
            "storage.sqlite_busy_timeout_ms",
        )?;
        require_nonzero(self.free_space_soft_bytes, "storage.free_space_soft_bytes")?;
        require_nonzero(self.free_space_hard_bytes, "storage.free_space_hard_bytes")?;
        if self.free_space_hard_bytes > self.free_space_soft_bytes {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "storage.free_space_hard_bytes must be <= storage.free_space_soft_bytes",
            ));
        }
        Ok(())
    }

    /// Data-directory advisory lock path: `<data_dir>/platform.lock`.
    #[must_use]
    pub fn data_lock_path(&self) -> PathBuf {
        self.data_dir.join(DATA_LOCK_FILE_NAME)
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
            prefix: DEFAULT_S3_PREFIX.to_string(),
            max_retries: 3,
            retry_backoff_ms: 200,
            connect_timeout_ms: 5_000,
            request_timeout_ms: 30_000,
        }
    }
}

impl S3Config {
    fn validate(&self) -> Result<(), PlatformError> {
        validate_s3_endpoint(&self.endpoint)?;
        if !self.verify_tls {
            return Err(PlatformError::new(
                ErrorCode::ConfigInvalid,
                "s3.verify_tls cannot be disabled",
            ));
        }
        if self.region.is_empty() {
            return Err(PlatformError::new(
                ErrorCode::ConfigInvalid,
                "s3.region must be non-empty",
            ));
        }
        if self.bucket.is_empty() {
            return Err(PlatformError::new(
                ErrorCode::ConfigInvalid,
                "s3.bucket must be non-empty",
            ));
        }
        validate_secret_pair(
            self.access_key_id_env.as_deref(),
            self.access_key_id_file.as_deref(),
            "s3.access_key_id",
        )?;
        validate_secret_pair(
            self.secret_access_key_env.as_deref(),
            self.secret_access_key_file.as_deref(),
            "s3.secret_access_key",
        )?;
        validate_s3_prefix(&self.prefix)?;
        require_nonzero(u64::from(self.max_retries), "s3.max_retries")?;
        require_nonzero(self.retry_backoff_ms, "s3.retry_backoff_ms")?;
        require_nonzero(self.connect_timeout_ms, "s3.connect_timeout_ms")?;
        require_nonzero(self.request_timeout_ms, "s3.request_timeout_ms")?;
        if self.request_timeout_ms < self.connect_timeout_ms {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "s3.request_timeout_ms must be >= s3.connect_timeout_ms",
            ));
        }
        Ok(())
    }
}

/// workerd binary, lock, assets, and supervisor budgets.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct RuntimeConfig {
    /// Absolute workerd binary path.
    pub binary: PathBuf,
    /// Absolute packaged workerd release manifest.
    pub lock_file: PathBuf,
    /// Absolute static assets directory.
    pub assets_dir: PathBuf,
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
            binary: PathBuf::from(DEFAULT_RUNTIME_BINARY),
            lock_file: PathBuf::from(DEFAULT_RUNTIME_LOCK_FILE),
            assets_dir: PathBuf::from(DEFAULT_RUNTIME_ASSETS),
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
        require_absolute(&self.binary, "runtime.binary")?;
        require_absolute(&self.lock_file, "runtime.lock_file")?;
        require_absolute(&self.assets_dir, "runtime.assets_dir")?;
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
            max_series: 256,
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

/// Bounded diagnostics retention.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct DiagnosticsConfig {
    /// Maximum failed-start reports retained.
    pub max_failed_starts: u32,
    /// Maximum diagnostics directory size in bytes.
    pub max_bytes: u64,
}

impl Default for DiagnosticsConfig {
    fn default() -> Self {
        Self {
            max_failed_starts: 32,
            max_bytes: 16_777_216,
        }
    }
}

impl DiagnosticsConfig {
    fn validate(&self) -> Result<(), PlatformError> {
        require_nonzero(
            u64::from(self.max_failed_starts),
            "diagnostics.max_failed_starts",
        )?;
        require_nonzero(self.max_bytes, "diagnostics.max_bytes")?;
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
    /// Deadline for waiting on in-flight deployment pins during delete.
    pub delete_drain_timeout_ms: u64,
    /// Minimum remote artifact orphan age before deletion.
    pub artifact_gc_grace_ms: u64,
    /// Background artifact GC interval.
    pub artifact_gc_interval_ms: u64,
    /// Maximum deployments finalized in one crash-recovery batch.
    pub delete_recovery_batch: u32,
    /// Number of newest ready deployments retained per Worker.
    pub retain_ready_deployments: u32,
    /// Number of newest rejected deployments retained per Worker.
    pub retain_rejected_deployments: u32,
    /// Minimum deployment age before automatic retention deletion.
    pub deployment_min_retention_ms: u64,
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
            retain_ready_deployments: 10,
            retain_rejected_deployments: 10,
            deployment_min_retention_ms: 24 * 60 * 60 * 1_000,
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
            u64::from(self.retain_ready_deployments),
            "workers.retain_ready_deployments",
        )?;
        require_nonzero(
            u64::from(self.retain_rejected_deployments),
            "workers.retain_rejected_deployments",
        )?;
        require_nonzero(
            self.deployment_min_retention_ms,
            "workers.deployment_min_retention_ms",
        )?;
        if self.max_bundle_bytes > 64 * 1024 * 1024
            || self.max_request_body_bytes > 64 * 1024 * 1024
            || self.delete_recovery_batch > 10_000
            || self.retain_ready_deployments > 10_000
            || self.retain_rejected_deployments > 10_000
        {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "Worker host policy exceeds the hard platform ceiling",
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
    fn validate(&self, field: &'static str) -> Result<(), PlatformError> {
        validate_secret_pair(self.env.as_deref(), self.file.as_deref(), field)
    }
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
    path.components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
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

fn is_loopback(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(addr) => addr.is_loopback(),
        IpAddr::V6(addr) => addr.is_loopback(),
    }
}

fn validate_s3_endpoint(endpoint: &str) -> Result<(), PlatformError> {
    let url = Url::parse(endpoint).map_err(|_| {
        PlatformError::new(
            ErrorCode::ConfigInvalid,
            "s3.endpoint must be a well-formed HTTP(S) URL",
        )
    })?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "s3.endpoint must be an http(s) URL",
        ));
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "s3.endpoint must include a host",
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "s3.endpoint must not include a username or password",
        ));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "s3.endpoint must not include a query or fragment",
        ));
    }
    Ok(())
}

fn validate_s3_prefix(prefix: &str) -> Result<(), PlatformError> {
    if prefix.is_empty() || !prefix.ends_with('/') {
        return Err(PlatformError::new(
            ErrorCode::S3PrefixInvalid,
            "s3.prefix must be non-empty and end with '/'",
        ));
    }
    if prefix.starts_with('/') || prefix.contains('\\') || prefix.contains("..") {
        return Err(PlatformError::new(
            ErrorCode::S3PrefixInvalid,
            "s3.prefix must be a relative internal prefix without '..'",
        ));
    }
    if prefix.starts_with("tenant/") {
        return Err(PlatformError::new(
            ErrorCode::S3PrefixInvalid,
            "s3.prefix must stay isolated from tenant prefixes",
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
