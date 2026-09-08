use super::*;

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
    /// The selected object-byte authority is unavailable.
    ObjectStorageUnavailable,
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
    /// Durable Object alarm scheduler is unavailable or degraded.
    SchedulerUnavailable,
    /// Scheduler remains serviceable but backlog or repair work is elevated.
    SchedulerBacklog,
    /// Required object storage remains available while an optional product surface is degraded.
    ObjectStorageDegraded,
    /// Host disk crossed the soft pressure threshold while bounded service continues.
    DiskSoftLimit,
    /// The latest committed full-platform snapshot is missing or too old.
    SnapshotStale,
    /// Vectorize or AI Search background authority is temporarily unavailable.
    SearchUnavailable,
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
            Self::ObjectStorageUnavailable => "OBJECT_STORAGE_UNAVAILABLE",
            Self::RuntimeStarting => "RUNTIME_STARTING",
            Self::RuntimeRestartBackoff => "RUNTIME_RESTART_BACKOFF",
            Self::RuntimeInvalid => "RUNTIME_INVALID",
            Self::Draining => "DRAINING",
            Self::SchemaTooNew => "SCHEMA_TOO_NEW",
            Self::DataDirInUse => "DATA_DIR_IN_USE",
            Self::DiskHardLimit => "DISK_HARD_LIMIT",
            Self::ConfigInvalid => "CONFIG_INVALID",
            Self::SchedulerUnavailable => "SCHEDULER_UNAVAILABLE",
            Self::SchedulerBacklog => "SCHEDULER_BACKLOG",
            Self::ObjectStorageDegraded => "OBJECT_STORAGE_DEGRADED",
            Self::DiskSoftLimit => "DISK_SOFT_LIMIT",
            Self::SnapshotStale => "SNAPSHOT_STALE",
            Self::SearchUnavailable => "SEARCH_UNAVAILABLE",
            Self::Ready => "READY",
        }
    }

    /// Whether this reason reports the platform as ready to take traffic.
    #[must_use]
    pub const fn is_ready(self) -> bool {
        matches!(
            self,
            Self::Ready
                | Self::SchedulerUnavailable
                | Self::SchedulerBacklog
                | Self::ObjectStorageDegraded
                | Self::DiskSoftLimit
                | Self::DiskHardLimit
                | Self::SnapshotStale
        )
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
