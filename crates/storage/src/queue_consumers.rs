//! Immutable Queue consumer declarations and live control attachments.

use crate::ControlDb;
use open_compute_core::{
    AccountId, ErrorCode, PlatformError, QueueConsumerId, QueueId, RequestId, VersionId, WorkerId,
};
use rusqlite::{OptionalExtension as _, Transaction, params};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// Default native Queue batch size.
pub const QUEUE_CONSUMER_DEFAULT_BATCH_SIZE: u32 = 10;
/// Default wait for a partial Queue batch.
pub const QUEUE_CONSUMER_DEFAULT_BATCH_TIMEOUT_SECONDS: u32 = 5;
/// Default product retry count after the initial delivery.
pub const QUEUE_CONSUMER_DEFAULT_MAX_RETRIES: u32 = 3;
/// Default retry delay.
pub const QUEUE_CONSUMER_DEFAULT_RETRY_DELAY_SECONDS: u32 = 0;
/// Default per-consumer concurrent batch cap.
pub const QUEUE_CONSUMER_DEFAULT_MAX_CONCURRENCY: u32 = 4;

/// Frozen Queue consumer delivery policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct QueueConsumerConfig {
    /// Maximum messages in one native batch.
    pub max_batch_size: u32,
    /// Maximum wait after the oldest message becomes available.
    pub max_batch_timeout_seconds: u32,
    /// Maximum retries after the first known delivery failure.
    pub max_retries: u32,
    /// Default retry delay.
    pub retry_delay_seconds: u32,
    /// Maximum in-flight batches for this consumer.
    pub max_concurrency: u32,
}

impl Default for QueueConsumerConfig {
    fn default() -> Self {
        Self {
            max_batch_size: QUEUE_CONSUMER_DEFAULT_BATCH_SIZE,
            max_batch_timeout_seconds: QUEUE_CONSUMER_DEFAULT_BATCH_TIMEOUT_SECONDS,
            max_retries: QUEUE_CONSUMER_DEFAULT_MAX_RETRIES,
            retry_delay_seconds: QUEUE_CONSUMER_DEFAULT_RETRY_DELAY_SECONDS,
            max_concurrency: QUEUE_CONSUMER_DEFAULT_MAX_CONCURRENCY,
        }
    }
}

impl QueueConsumerConfig {
    /// Validate public API bounds and the operator-local concurrency ceiling.
    pub fn validate(self, local_max_concurrency: u32) -> Result<Self, PlatformError> {
        if !(1..=100).contains(&self.max_batch_size)
            || self.max_batch_timeout_seconds > 60
            || self.max_retries > 100
            || self.retry_delay_seconds > 86_400
            || self.max_concurrency == 0
            || self.max_concurrency > local_max_concurrency
        {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "Queue consumer configuration is outside supported bounds",
            ));
        }
        Ok(self)
    }
}

/// Immutable staging row inserted with a version.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewQueueConsumerDeclaration {
    /// Platform-generated declaration identity.
    pub id: QueueConsumerId,
    /// Source Queue identity.
    pub queue_id: QueueId,
    /// Frozen source Queue lifecycle generation.
    pub queue_lifecycle_generation: u64,
    /// Optional named Worker entrypoint.
    pub entrypoint: Option<String>,
    /// Frozen delivery policy.
    pub config: QueueConsumerConfig,
    /// Optional dead-letter Queue identity and exact lifecycle generation.
    pub dead_letter_queue: Option<(QueueId, u64)>,
    /// Capability version.
    pub capability_version: u32,
    /// Canonical declaration digest.
    pub descriptor_sha256: [u8; 32],
}

/// Immutable version Queue consumer declaration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueConsumerDeclaration {
    /// Declaration identity.
    pub id: QueueConsumerId,
    /// Owning version.
    pub version_id: VersionId,
    /// Source Queue identity.
    pub queue_id: QueueId,
    /// Frozen source Queue lifecycle generation.
    pub queue_lifecycle_generation: u64,
    /// Optional named Worker entrypoint.
    pub entrypoint: Option<String>,
    /// Frozen delivery policy.
    #[serde(flatten)]
    pub config: QueueConsumerConfig,
    /// Optional dead-letter Queue identity.
    pub dlq_queue_id: Option<QueueId>,
    /// Optional dead-letter Queue lifecycle generation.
    pub dlq_lifecycle_generation: Option<u64>,
    /// Capability version.
    pub capability_version: u32,
    /// Canonical declaration digest.
    #[serde(skip)]
    pub descriptor_sha256: [u8; 32],
    /// Creation timestamp.
    pub created_at_ms: i64,
}

/// Live Queue consumer lifecycle state.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueConsumerState {
    /// Control row exists while the scheduler projection is staged.
    Activating,
    /// New batches may be claimed.
    Active,
    /// Operator pause stops new claims.
    Paused,
    /// Old generation drains before a target switch.
    Updating,
    /// Projection drains before removal.
    Deleting,
    /// Immutable retired attachment.
    Tombstoned,
}

impl QueueConsumerState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Activating => "activating",
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Updating => "updating",
            Self::Deleting => "deleting",
            Self::Tombstoned => "tombstoned",
        }
    }
}

impl FromStr for QueueConsumerState {
    type Err = PlatformError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "activating" => Ok(Self::Activating),
            "active" => Ok(Self::Active),
            "paused" => Ok(Self::Paused),
            "updating" => Ok(Self::Updating),
            "deleting" => Ok(Self::Deleting),
            "tombstoned" => Ok(Self::Tombstoned),
            _ => Err(invariant()),
        }
    }
}

/// Live Queue consumer attachment.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueConsumerRecord {
    /// Attachment identity.
    pub id: QueueConsumerId,
    /// Owning account.
    pub account_id: AccountId,
    /// Source Queue identity.
    pub queue_id: QueueId,
    /// Owning Worker.
    pub worker_id: WorkerId,
    /// Current declaration identity.
    pub declaration_id: QueueConsumerId,
    /// Current frozen version.
    pub version_id: VersionId,
    /// Desired declaration persisted before the old generation starts draining.
    pub pending_declaration_id: Option<QueueConsumerId>,
    /// Desired version retained until the generation switch commits.
    pub pending_version_id: Option<VersionId>,
    /// Desired Worker retained until the generation switch commits.
    pub pending_worker_id: Option<WorkerId>,
    /// Monotonic consumer generation.
    pub consumer_generation: u64,
    /// Lifecycle state.
    pub state: QueueConsumerState,
    /// Stable availability spelling.
    pub availability: String,
    /// Stable reason when not healthy.
    pub availability_code: Option<String>,
    /// Creation timestamp.
    pub created_at_ms: i64,
    /// Last mutation timestamp.
    pub updated_at_ms: i64,
    /// Tombstone timestamp.
    pub deleted_at_ms: Option<i64>,
}

/// Control repository for Queue consumer declarations and attachments.
#[derive(Clone, Copy, Debug)]
pub struct QueueConsumerRepository<'a> {
    db: &'a ControlDb,
}

mod repository;

fn audit_operator_action(
    tx: &Transaction<'_>,
    id: QueueConsumerId,
    generation: u64,
    action: &str,
    request_id: RequestId,
    now_ms: i64,
) -> Result<(), PlatformError> {
    let changed = tx
        .execute(
            "INSERT INTO control_audit_events
             (account_id, action, target_type, target_id, request_id, details_json, created_at_ms)
             SELECT account_id, ?1, 'queue_consumer', id, ?2, X'7B7D', ?3
             FROM queue_consumers WHERE id = ?4 AND consumer_generation = ?5",
            params![
                action,
                request_id.to_string(),
                now_ms,
                id.to_string(),
                as_i64(generation)?,
            ],
        )
        .map_err(|_| invariant())?;
    if changed != 1 {
        return Err(invariant());
    }
    Ok(())
}

pub(crate) fn insert_staging_declarations(
    tx: &Transaction<'_>,
    version_id: VersionId,
    declarations: &[NewQueueConsumerDeclaration],
    now_ms: i64,
) -> Result<(), PlatformError> {
    for declaration in declarations {
        insert_declaration(
            tx,
            version_id,
            declaration,
            DeclarationOrigin::Version,
            now_ms,
        )?;
    }
    Ok(())
}

fn insert_declaration(
    tx: &Transaction<'_>,
    version_id: VersionId,
    declaration: &NewQueueConsumerDeclaration,
    origin: DeclarationOrigin,
    now_ms: i64,
) -> Result<(), PlatformError> {
    let (dlq, dlq_generation) = declaration
        .dead_letter_queue
        .map_or((None, None), |(id, generation)| {
            (Some(id.to_string()), Some(generation))
        });
    tx.execute(
        "INSERT INTO version_queue_consumers
             (id, version_id, origin, queue_id, queue_lifecycle_generation, entrypoint,
              max_batch_size, max_batch_timeout_seconds, max_retries, retry_delay_seconds,
              max_concurrency, dlq_queue_id, dlq_lifecycle_generation, capability_version,
              descriptor_sha256, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
        params![
            declaration.id.to_string(),
            version_id.to_string(),
            origin.as_str(),
            declaration.queue_id.to_string(),
            as_i64(declaration.queue_lifecycle_generation)?,
            declaration.entrypoint,
            i64::from(declaration.config.max_batch_size),
            i64::from(declaration.config.max_batch_timeout_seconds),
            i64::from(declaration.config.max_retries),
            i64::from(declaration.config.retry_delay_seconds),
            i64::from(declaration.config.max_concurrency),
            dlq,
            dlq_generation.map(as_i64).transpose()?,
            i64::from(declaration.capability_version),
            declaration.descriptor_sha256.as_slice(),
            now_ms,
        ],
    )
    .map_err(|_| invariant())?;
    Ok(())
}

fn read_record_tx(
    tx: &Transaction<'_>,
    id: QueueConsumerId,
) -> Result<QueueConsumerRecord, PlatformError> {
    tx.query_row(
        "SELECT id, account_id, queue_id, worker_id, declaration_id, version_id,
                pending_declaration_id, pending_version_id, pending_worker_id, consumer_generation,
                state, availability, availability_code,
                created_at_ms, updated_at_ms, deleted_at_ms
         FROM queue_consumers WHERE id = ?1",
        [id.to_string()],
        map_record,
    )
    .map_err(|_| invariant())
}

#[derive(Clone, Copy)]
enum DeclarationOrigin {
    Version,
    Api,
}

impl DeclarationOrigin {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Version => "version",
            Self::Api => "api",
        }
    }
}

fn map_declaration(row: &rusqlite::Row<'_>) -> rusqlite::Result<QueueConsumerDeclaration> {
    Ok(QueueConsumerDeclaration {
        id: parse(&row.get::<_, String>(0)?)?,
        version_id: parse(&row.get::<_, String>(1)?)?,
        queue_id: parse(&row.get::<_, String>(2)?)?,
        queue_lifecycle_generation: unsigned(row.get(3)?)?,
        entrypoint: row.get(4)?,
        config: QueueConsumerConfig {
            max_batch_size: unsigned(row.get(5)?)?,
            max_batch_timeout_seconds: unsigned(row.get(6)?)?,
            max_retries: unsigned(row.get(7)?)?,
            retry_delay_seconds: unsigned(row.get(8)?)?,
            max_concurrency: unsigned(row.get(9)?)?,
        },
        dlq_queue_id: row
            .get::<_, Option<String>>(10)?
            .as_deref()
            .map(parse)
            .transpose()?,
        dlq_lifecycle_generation: row.get::<_, Option<i64>>(11)?.map(unsigned).transpose()?,
        capability_version: unsigned(row.get(12)?)?,
        descriptor_sha256: digest(row.get(13)?)?,
        created_at_ms: row.get(14)?,
    })
}

fn map_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<QueueConsumerRecord> {
    Ok(QueueConsumerRecord {
        id: parse(&row.get::<_, String>(0)?)?,
        account_id: parse(&row.get::<_, String>(1)?)?,
        queue_id: parse(&row.get::<_, String>(2)?)?,
        worker_id: parse(&row.get::<_, String>(3)?)?,
        declaration_id: parse(&row.get::<_, String>(4)?)?,
        version_id: parse(&row.get::<_, String>(5)?)?,
        pending_declaration_id: row
            .get::<_, Option<String>>(6)?
            .as_deref()
            .map(parse)
            .transpose()?,
        pending_version_id: row
            .get::<_, Option<String>>(7)?
            .as_deref()
            .map(parse)
            .transpose()?,
        pending_worker_id: row
            .get::<_, Option<String>>(8)?
            .as_deref()
            .map(parse)
            .transpose()?,
        consumer_generation: unsigned(row.get(9)?)?,
        state: row
            .get::<_, String>(10)?
            .parse()
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        availability: row.get(11)?,
        availability_code: row.get(12)?,
        created_at_ms: row.get(13)?,
        updated_at_ms: row.get(14)?,
        deleted_at_ms: row.get(15)?,
    })
}

fn parse<T: FromStr>(value: &str) -> rusqlite::Result<T> {
    value.parse().map_err(|_| rusqlite::Error::InvalidQuery)
}

fn unsigned<T: TryFrom<i64>>(value: i64) -> rusqlite::Result<T> {
    T::try_from(value).map_err(|_| rusqlite::Error::InvalidQuery)
}

fn digest(value: Vec<u8>) -> rusqlite::Result<[u8; 32]> {
    value.try_into().map_err(|_| rusqlite::Error::InvalidQuery)
}

fn as_i64(value: u64) -> Result<i64, PlatformError> {
    i64::try_from(value).map_err(|_| invariant())
}

fn collect<T>(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>>,
) -> Result<Vec<T>, PlatformError> {
    rows.collect::<Result<Vec<_>, _>>().map_err(|_| invariant())
}

fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::QueueInvariantViolation,
        "Queue consumer control invariant failed",
    )
}
