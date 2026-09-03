//! Queue catalog value types and fixed producer limits.

use open_compute_core::{AccountId, BindingId, ErrorCode, PlatformError, QueueId, VersionId};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// Queue producer capability version implemented by P2.2.
pub const QUEUE_PRODUCER_CAPABILITY_VERSION: u32 = 1;
/// Public per-message body limit in decimal bytes.
pub const QUEUE_MAX_MESSAGE_BYTES: u64 = 128_000;
/// Public maximum messages in one producer batch.
pub const QUEUE_MAX_BATCH_MESSAGES: u32 = 100;
/// Public maximum aggregate body bytes in one producer batch.
pub const QUEUE_MAX_BATCH_BYTES: u64 = 256_000;
/// Public maximum delivery delay.
pub const QUEUE_MAX_DELAY_SECONDS: u32 = 86_400;
/// Minimum retained lifetime.
pub const QUEUE_MIN_RETENTION_SECONDS: u32 = 60;
/// Maximum retained lifetime.
pub const QUEUE_MAX_RETENTION_SECONDS: u32 = 1_209_600;
/// Default retained lifetime (four days).
pub const QUEUE_DEFAULT_RETENTION_SECONDS: u32 = 345_600;
/// Default local Queue backlog limit used when an operator does not lower it.
pub const QUEUE_DEFAULT_MAX_BACKLOG_BYTES: u64 = 1_073_741_824;

/// Queue lifecycle state in the central control catalog.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueState {
    /// Catalog identity exists while the scheduler projection is being created.
    Creating,
    /// Queue may be bound and used.
    Ready,
    /// New references and sends are fenced while deletion converges.
    Deleting,
    /// Immutable retired identity.
    Tombstoned,
}

impl QueueState {
    /// Stable database spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Creating => "creating",
            Self::Ready => "ready",
            Self::Deleting => "deleting",
            Self::Tombstoned => "tombstoned",
        }
    }
}

impl FromStr for QueueState {
    type Err = PlatformError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "creating" => Ok(Self::Creating),
            "ready" => Ok(Self::Ready),
            "deleting" => Ok(Self::Deleting),
            "tombstoned" => Ok(Self::Tombstoned),
            _ => Err(invariant()),
        }
    }
}

/// Queue-specific availability independent from lifecycle.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueAvailability {
    /// Catalog and scheduler projection agree exactly.
    Healthy,
    /// Reconciliation is required before new sends.
    Degraded,
    /// Queue-local authority cannot currently be used.
    Unavailable,
}

impl QueueAvailability {
    /// Stable database spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Degraded => "degraded",
            Self::Unavailable => "unavailable",
        }
    }
}

impl FromStr for QueueAvailability {
    type Err = PlatformError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "healthy" => Ok(Self::Healthy),
            "degraded" => Ok(Self::Degraded),
            "unavailable" => Ok(Self::Unavailable),
            _ => Err(invariant()),
        }
    }
}

/// Persisted Queue behavior and safety limits.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QueueConfig {
    /// Default delivery delay.
    pub delivery_delay_seconds: u32,
    /// Message retention lifetime.
    pub retention_seconds: u32,
    /// Per-message serialized body limit.
    pub max_message_bytes: u64,
    /// Per-batch message count limit.
    pub max_batch_messages: u32,
    /// Per-batch aggregate body limit.
    pub max_batch_bytes: u64,
    /// Queue-local durable body-byte quota.
    pub max_backlog_bytes: u64,
}

impl Default for QueueConfig {
    fn default() -> Self {
        Self {
            delivery_delay_seconds: 0,
            retention_seconds: QUEUE_DEFAULT_RETENTION_SECONDS,
            max_message_bytes: QUEUE_MAX_MESSAGE_BYTES,
            max_batch_messages: QUEUE_MAX_BATCH_MESSAGES,
            max_batch_bytes: QUEUE_MAX_BATCH_BYTES,
            max_backlog_bytes: QUEUE_DEFAULT_MAX_BACKLOG_BYTES,
        }
    }
}

impl QueueConfig {
    /// Validate fixed API ceilings and persisted local policy.
    pub fn validate(self) -> Result<Self, PlatformError> {
        if self.delivery_delay_seconds > QUEUE_MAX_DELAY_SECONDS
            || !(QUEUE_MIN_RETENTION_SECONDS..=QUEUE_MAX_RETENTION_SECONDS)
                .contains(&self.retention_seconds)
            || self.max_message_bytes == 0
            || self.max_message_bytes > QUEUE_MAX_MESSAGE_BYTES
            || self.max_batch_messages == 0
            || self.max_batch_messages > QUEUE_MAX_BATCH_MESSAGES
            || self.max_batch_bytes == 0
            || self.max_batch_bytes > QUEUE_MAX_BATCH_BYTES
            || self.max_backlog_bytes == 0
        {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "Queue configuration is outside the supported bounds",
            ));
        }
        Ok(self)
    }
}

/// Full control-plane Queue row.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueRecord {
    /// Immutable Queue identity.
    pub id: QueueId,
    /// Owning account.
    pub account_id: AccountId,
    /// Mutable display name.
    pub name: String,
    /// Lifecycle state.
    pub state: QueueState,
    /// Projection availability.
    pub availability: QueueAvailability,
    /// Stable degraded/unavailable reason.
    pub availability_code: Option<String>,
    /// Immutable binding-breaking lifecycle generation.
    pub lifecycle_generation: u64,
    /// Send-behavior projection generation.
    pub config_generation: u64,
    /// Whether consumer delivery is administratively paused.
    pub delivery_paused: bool,
    /// Persisted Queue policy.
    #[serde(flatten)]
    pub config: QueueConfig,
    /// Creation time.
    pub created_at_ms: i64,
    /// Last control mutation time.
    pub updated_at_ms: i64,
    /// Tombstone time.
    pub deleted_at_ms: Option<i64>,
}

/// Immutable Queue producer binding row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueueProducerBindingRecord {
    /// Binding identity.
    pub id: BindingId,
    /// Owning version.
    pub version_id: VersionId,
    /// Tenant environment name.
    pub name: String,
    /// Frozen Queue identity.
    pub queue_id: QueueId,
    /// Frozen Queue lifecycle generation.
    pub queue_lifecycle_generation: u64,
    /// Static capability version.
    pub capability_version: u32,
    /// Canonical descriptor digest.
    pub descriptor_sha256: [u8; 32],
    /// Creation time.
    pub created_at_ms: i64,
}

/// Staging input inserted in the same transaction as an immutable version.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewQueueProducerBinding {
    /// Platform-generated binding identity.
    pub id: BindingId,
    /// Tenant environment name.
    pub name: String,
    /// Frozen Queue identity.
    pub queue_id: QueueId,
    /// Frozen Queue lifecycle generation.
    pub queue_lifecycle_generation: u64,
    /// Static capability version.
    pub capability_version: u32,
    /// Canonical descriptor digest.
    pub descriptor_sha256: [u8; 32],
}

/// Trusted backend authorization assembled from version and Queue authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthorizedQueueBinding {
    /// Immutable binding.
    pub binding: QueueProducerBindingRecord,
    /// Current exact Queue control row.
    pub queue: QueueRecord,
    /// Account resolved through the version Worker.
    pub account_id: AccountId,
}

/// Atomic Queue create reservation outcome from the control authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum QueueCreateReservation {
    /// The reservation and creating Queue row were committed together.
    Reserved(QueueRecord),
    /// The exact prior operation already completed.
    Complete(Vec<u8>),
    /// The exact prior operation is still converging.
    Running,
    /// The exact prior operation failed deterministically.
    Failed(Vec<u8>),
}

/// Persisted running Queue mutation intent used for exact restart reconciliation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunningQueueMutation {
    /// Account-scoped idempotency owner.
    pub account_id: AccountId,
    /// Operation scope containing the immutable Queue identity.
    pub scope: String,
    /// Caller idempotency key.
    pub idempotency_key: String,
    /// Stored HMAC request fingerprint.
    pub request_fingerprint: [u8; 32],
    /// Mutation target.
    pub queue_id: QueueId,
    /// Canonical versioned mutation intent JSON.
    pub intent_json: Vec<u8>,
}

fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::QueueInvariantViolation,
        "Queue model invariant failed",
    )
}
