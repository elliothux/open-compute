//! Immutable Queue consumer declarations and live control attachments.

use crate::control_db::ControlDb;
use open_compute_core::{
    ErrorCode, InstanceId, PlatformError, QueueConsumerId, QueueId, RequestId, VersionId, WorkerId,
};
use rusqlite::{OptionalExtension as _, Transaction, params};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// Maximum messages in one native Queue consumer batch.
pub const QUEUE_CONSUMER_MAX_BATCH_SIZE: u32 = 100;

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
        if !(1..=QUEUE_CONSUMER_MAX_BATCH_SIZE).contains(&self.max_batch_size)
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
    /// New batches may be claimed.
    Active,
    /// Operator pause stops new claims.
    Paused,
}

impl QueueConsumerState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
        }
    }
}

impl FromStr for QueueConsumerState {
    type Err = PlatformError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "active" => Ok(Self::Active),
            "paused" => Ok(Self::Paused),
            _ => Err(invariant()),
        }
    }
}

impl rusqlite::types::ToSql for QueueConsumerState {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        Ok(self.as_str().into())
    }
}

impl rusqlite::types::FromSql for QueueConsumerState {
    fn column_result(value: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        value
            .as_str()?
            .parse()
            .map_err(|_| rusqlite::types::FromSqlError::InvalidType)
    }
}

/// Live Queue consumer attachment.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueConsumerRecord {
    /// Attachment identity.
    pub id: QueueConsumerId,
    /// Owning instance.
    pub instance_id: InstanceId,
    /// Source Queue identity.
    pub queue_id: QueueId,
    /// Owning Worker.
    pub worker_id: WorkerId,
    /// Current declaration identity.
    pub declaration_id: QueueConsumerId,
    /// Current frozen version.
    pub version_id: VersionId,
    /// Monotonic consumer generation.
    pub consumer_generation: u64,
    /// Lifecycle state.
    pub state: QueueConsumerState,
    /// Creation timestamp.
    pub created_at_ms: i64,
    /// Last mutation timestamp.
    pub updated_at_ms: i64,
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
             (action, target_type, target_id, request_id, details_json, created_at_ms)
             SELECT ?1, 'queue_consumer', id, ?2, X'7B7D', ?3
             FROM queue_consumers WHERE id = ?4 AND consumer_generation = ?5",
            params![action, request_id, now_ms, id, as_i64(generation)?,],
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
            (Some(id), Some(generation))
        });
    tx.execute(
        "INSERT INTO version_queue_consumers
             (id, version_id, origin, queue_id, queue_lifecycle_generation, entrypoint,
              max_batch_size, max_batch_timeout_seconds, max_retries, retry_delay_seconds,
              max_concurrency, dlq_queue_id, dlq_lifecycle_generation, capability_version,
              descriptor_sha256, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
        params![
            declaration.id,
            version_id,
            origin.as_str(),
            declaration.queue_id,
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
        "SELECT id, (SELECT instance_id FROM instance_identity), queue_id, worker_id, declaration_id, version_id,
                consumer_generation,
                state,
                created_at_ms, updated_at_ms
         FROM queue_consumers WHERE id = ?1",
        [id],
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
        id: row.get(0)?,
        version_id: row.get(1)?,
        queue_id: row.get(2)?,
        queue_lifecycle_generation: unsigned(row.get(3)?)?,
        entrypoint: row.get(4)?,
        config: QueueConsumerConfig {
            max_batch_size: unsigned(row.get(5)?)?,
            max_batch_timeout_seconds: unsigned(row.get(6)?)?,
            max_retries: unsigned(row.get(7)?)?,
            retry_delay_seconds: unsigned(row.get(8)?)?,
            max_concurrency: unsigned(row.get(9)?)?,
        },
        dlq_queue_id: row.get(10)?,
        dlq_lifecycle_generation: row.get::<_, Option<i64>>(11)?.map(unsigned).transpose()?,
        capability_version: unsigned(row.get(12)?)?,
        descriptor_sha256: digest(row.get(13)?)?,
        created_at_ms: row.get(14)?,
    })
}

fn map_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<QueueConsumerRecord> {
    Ok(QueueConsumerRecord {
        id: row.get(0)?,
        instance_id: row.get(1)?,
        queue_id: row.get(2)?,
        worker_id: row.get(3)?,
        declaration_id: row.get(4)?,
        version_id: row.get(5)?,
        consumer_generation: unsigned(row.get(6)?)?,
        state: row.get(7)?,
        created_at_ms: row.get(8)?,
        updated_at_ms: row.get(9)?,
    })
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

pub(crate) fn insert_scheduler_targets_tx(
    tx: &Transaction<'_>,
    worker_id: WorkerId,
) -> rusqlite::Result<()> {
    tx.execute("INSERT OR IGNORE INTO queue_consumer_state
        (consumer_id,queue_id,consumer_generation,version_id,worker_id,execution_generation,entrypoint,state,
         max_batch_size,max_batch_timeout_ms,max_retries,retry_delay_seconds,max_concurrency,
         dlq_queue_id,dlq_queue_generation,descriptor_sha256,updated_at_ms)
        SELECT c.id,c.queue_id,c.consumer_generation,c.version_id,c.worker_id,w.route_generation,d.entrypoint,c.state,
          d.max_batch_size,d.max_batch_timeout_seconds*1000,d.max_retries,d.retry_delay_seconds,d.max_concurrency,
          d.dlq_queue_id,d.dlq_lifecycle_generation,d.descriptor_sha256,c.updated_at_ms
        FROM queue_consumers c JOIN version_queue_consumers d ON d.id=c.declaration_id JOIN workers w ON w.id=c.worker_id
        WHERE c.worker_id=?1",[worker_id])?;
    Ok(())
}
