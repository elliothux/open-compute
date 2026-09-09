use super::*;

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

pub(super) fn validate_local_object_root(
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

    pub(super) fn normalize_implicit_env_defaults(&mut self) {
        if let Self::S3(config) = self {
            config.normalize_implicit_env_defaults();
        }
    }

    pub(super) fn resolve_paths(&mut self, base: &Path) -> Result<(), PlatformError> {
        match self {
            Self::Local(config) => config.path = resolve_host_path(base, &config.path)?,
            Self::S3(config) => {
                resolve_optional_path(base, &mut config.access_key_id_file)?;
                resolve_optional_path(base, &mut config.secret_access_key_file)?;
            }
        }
        Ok(())
    }

    pub(super) fn validate(&self) -> Result<(), PlatformError> {
        match self {
            Self::Local(config) => config.validate(),
            Self::S3(config) => config.validate(),
        }
    }
}
