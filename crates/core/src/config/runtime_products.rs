use super::*;

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
    pub(super) fn validate(&self) -> Result<(), PlatformError> {
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
    pub(super) fn validate(&self) -> Result<(), PlatformError> {
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
    pub(super) fn validate(&self) -> Result<(), PlatformError> {
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
    pub(super) fn validate(&self) -> Result<(), PlatformError> {
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
    pub(super) fn validate(&self) -> Result<(), PlatformError> {
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
