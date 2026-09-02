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

impl<'a> QueueConsumerRepository<'a> {
    /// Bind the central control database.
    #[must_use]
    pub const fn new(db: &'a ControlDb) -> Self {
        Self { db }
    }

    /// Read one live or tombstoned Queue consumer attachment by identity.
    pub fn get(&self, id: QueueConsumerId) -> Result<QueueConsumerRecord, PlatformError> {
        self.db.with_read(|connection| {
            connection
                .query_row(
                    "SELECT id, account_id, queue_id, worker_id, declaration_id,
                            version_id, pending_declaration_id, pending_version_id,
                            consumer_generation, state, availability,
                            availability_code, created_at_ms, updated_at_ms, deleted_at_ms
                     FROM queue_consumers WHERE id = ?1",
                    [id.to_string()],
                    map_record,
                )
                .map_err(|_| invariant())
        })
    }

    /// Read one immutable Queue consumer declaration by identity.
    pub fn declaration(
        &self,
        id: QueueConsumerId,
    ) -> Result<QueueConsumerDeclaration, PlatformError> {
        self.db.with_read(|connection| {
            connection
                .query_row(
                    "SELECT id, version_id, queue_id, queue_lifecycle_generation,
                            entrypoint, max_batch_size, max_batch_timeout_seconds,
                            max_retries, retry_delay_seconds, max_concurrency,
                            dlq_queue_id, dlq_lifecycle_generation, capability_version,
                            descriptor_sha256, created_at_ms
                     FROM version_queue_consumers WHERE id = ?1",
                    [id.to_string()],
                    map_declaration,
                )
                .map_err(|_| invariant())
        })
    }

    /// List a bounded global operator view of non-tombstoned attachments.
    pub fn list_live(&self, limit: u32) -> Result<Vec<QueueConsumerRecord>, PlatformError> {
        if limit == 0 {
            return Err(invariant());
        }
        self.db.with_read(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT id, account_id, queue_id, worker_id, declaration_id,
                            version_id, pending_declaration_id, pending_version_id,
                            consumer_generation, state, availability,
                            availability_code, created_at_ms, updated_at_ms, deleted_at_ms
                     FROM queue_consumers WHERE state != 'tombstoned'
                     ORDER BY account_id, queue_id, id LIMIT ?1",
                )
                .map_err(|_| invariant())?;
            let rows = statement
                .query_map([i64::from(limit)], map_record)
                .map_err(|_| invariant())?;
            collect(rows)
        })
    }

    /// Read immutable declarations for one version.
    pub fn version_declarations(
        &self,
        version_id: VersionId,
    ) -> Result<Vec<QueueConsumerDeclaration>, PlatformError> {
        self.db.with_read(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT id, version_id, queue_id, queue_lifecycle_generation,
                            entrypoint, max_batch_size, max_batch_timeout_seconds,
                            max_retries, retry_delay_seconds, max_concurrency,
                            dlq_queue_id, dlq_lifecycle_generation, capability_version,
                            descriptor_sha256, created_at_ms
                     FROM version_queue_consumers WHERE version_id = ?1
                     ORDER BY queue_id, id",
                )
                .map_err(|_| invariant())?;
            let rows = statement
                .query_map([version_id.to_string()], map_declaration)
                .map_err(|_| invariant())?;
            collect(rows)
        })
    }

    /// Read the one non-tombstoned attachment for a Queue, if present.
    pub fn live_for_queue(
        &self,
        queue_id: QueueId,
    ) -> Result<Option<QueueConsumerRecord>, PlatformError> {
        self.db.with_read(|connection| {
            connection
                .query_row(
                    "SELECT id, account_id, queue_id, worker_id, declaration_id,
                            version_id, pending_declaration_id, pending_version_id,
                            consumer_generation, state, availability,
                            availability_code, created_at_ms, updated_at_ms, deleted_at_ms
                     FROM queue_consumers WHERE queue_id = ?1 AND state != 'tombstoned'",
                    [queue_id.to_string()],
                    map_record,
                )
                .optional()
                .map_err(|_| invariant())
        })
    }

    /// List non-tombstoned attachments owned by one Worker.
    pub fn live_for_worker(
        &self,
        worker_id: WorkerId,
    ) -> Result<Vec<QueueConsumerRecord>, PlatformError> {
        self.db.with_read(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT id, account_id, queue_id, worker_id, declaration_id,
                            version_id, pending_declaration_id, pending_version_id,
                            consumer_generation, state, availability,
                            availability_code, created_at_ms, updated_at_ms, deleted_at_ms
                     FROM queue_consumers WHERE worker_id = ?1 AND state != 'tombstoned'
                     ORDER BY queue_id, id",
                )
                .map_err(|_| invariant())?;
            let rows = statement
                .query_map([worker_id.to_string()], map_record)
                .map_err(|_| invariant())?;
            collect(rows)
        })
    }

    /// Create a projection-pending attachment for an exact ready declaration.
    pub fn create_attachment(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
        declaration: &QueueConsumerDeclaration,
        now_ms: i64,
    ) -> Result<QueueConsumerRecord, PlatformError> {
        if self.live_for_queue(declaration.queue_id)?.is_some() {
            return Err(PlatformError::new(
                ErrorCode::QueueConsumerConflict,
                "Queue already has a live push consumer",
            ));
        }
        let id = QueueConsumerId::generate();
        self.db.with_immediate(|tx| {
            tx.execute(
                "INSERT INTO queue_consumers
                 (id, account_id, queue_id, worker_id, declaration_id, version_id,
                  consumer_generation, state, availability, availability_code,
                  created_at_ms, updated_at_ms, deleted_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, 'activating', 'degraded',
                         'QUEUE_CONSUMER_PROJECTION_PENDING', ?7, ?7, NULL)",
                params![
                    id.to_string(),
                    account_id.to_string(),
                    declaration.queue_id.to_string(),
                    worker_id.to_string(),
                    declaration.id.to_string(),
                    declaration.version_id.to_string(),
                    now_ms,
                ],
            )
            .map_err(|error| {
                if error.to_string().contains("UNIQUE") {
                    PlatformError::new(
                        ErrorCode::QueueConsumerConflict,
                        "Queue already has a live push consumer",
                    )
                } else {
                    invariant()
                }
            })?;
            read_record_tx(tx, id)
        })
    }

    /// Mark a staged projection accepting and expose the attachment as active.
    pub fn finish_activation(
        &self,
        id: QueueConsumerId,
        generation: u64,
        now_ms: i64,
    ) -> Result<bool, PlatformError> {
        self.finish_state(
            id,
            generation,
            QueueConsumerState::Activating,
            QueueConsumerState::Active,
            None,
            now_ms,
        )
    }

    /// Pause new claims without invalidating the current consumer generation.
    pub fn pause(
        &self,
        id: QueueConsumerId,
        generation: u64,
        request_id: RequestId,
        now_ms: i64,
    ) -> Result<bool, PlatformError> {
        self.finish_state(
            id,
            generation,
            QueueConsumerState::Active,
            QueueConsumerState::Paused,
            Some(("queue_consumer.pause", request_id)),
            now_ms,
        )
    }

    /// Resume new claims without invalidating the current consumer generation.
    pub fn resume(
        &self,
        id: QueueConsumerId,
        generation: u64,
        request_id: RequestId,
        now_ms: i64,
    ) -> Result<bool, PlatformError> {
        self.finish_state(
            id,
            generation,
            QueueConsumerState::Paused,
            QueueConsumerState::Active,
            Some(("queue_consumer.resume", request_id)),
            now_ms,
        )
    }

    /// Fence old claims and advance the live consumer generation before draining.
    pub fn begin_update(
        &self,
        id: QueueConsumerId,
        generation: u64,
        declaration: &QueueConsumerDeclaration,
        now_ms: i64,
    ) -> Result<bool, PlatformError> {
        let next = generation.checked_add(1).ok_or_else(invariant)?;
        self.db.with_immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE queue_consumers
                     SET consumer_generation = ?1, state = 'updating', availability = 'degraded',
                         pending_declaration_id = ?2, pending_version_id = ?3,
                         availability_code = CASE state
                           WHEN 'paused' THEN 'QUEUE_CONSUMER_DRAINING_PAUSED'
                           ELSE 'QUEUE_CONSUMER_DRAINING'
                         END,
                         updated_at_ms = ?4
                     WHERE id = ?5 AND queue_id = ?6 AND consumer_generation = ?7
                       AND state IN ('active', 'paused')",
                    params![
                        as_i64(next)?,
                        declaration.id.to_string(),
                        declaration.version_id.to_string(),
                        now_ms,
                        id.to_string(),
                        declaration.queue_id.to_string(),
                        as_i64(generation)?,
                    ],
                )
                .map_err(|_| invariant())?;
            Ok(changed == 1)
        })
    }

    /// Switch an already-draining attachment to the next frozen declaration.
    pub fn switch_target(
        &self,
        id: QueueConsumerId,
        generation: u64,
        declaration: &QueueConsumerDeclaration,
        now_ms: i64,
    ) -> Result<bool, PlatformError> {
        self.db.with_immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE queue_consumers SET declaration_id = ?1, version_id = ?2,
                            pending_declaration_id = NULL, pending_version_id = NULL,
                            updated_at_ms = ?3
                     WHERE id = ?4 AND queue_id = ?5 AND consumer_generation = ?6
                       AND state = 'updating' AND pending_declaration_id = ?1
                       AND pending_version_id = ?2",
                    params![
                        declaration.id.to_string(),
                        declaration.version_id.to_string(),
                        now_ms,
                        id.to_string(),
                        declaration.queue_id.to_string(),
                        as_i64(generation)?,
                    ],
                )
                .map_err(|_| invariant())?;
            Ok(changed == 1)
        })
    }

    /// Expose the switched target after its scheduler projection accepts claims.
    pub fn finish_update(
        &self,
        id: QueueConsumerId,
        generation: u64,
        paused: bool,
        now_ms: i64,
    ) -> Result<bool, PlatformError> {
        self.finish_state(
            id,
            generation,
            QueueConsumerState::Updating,
            if paused {
                QueueConsumerState::Paused
            } else {
                QueueConsumerState::Active
            },
            None,
            now_ms,
        )
    }

    /// Fence new claims before deleting an attachment.
    pub fn begin_delete(
        &self,
        id: QueueConsumerId,
        generation: u64,
        now_ms: i64,
    ) -> Result<bool, PlatformError> {
        self.db.with_immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE queue_consumers SET state = 'deleting', availability = 'degraded',
                            availability_code = 'QUEUE_CONSUMER_DRAINING', updated_at_ms = ?1
                     WHERE id = ?2 AND consumer_generation = ?3
                       AND state IN ('activating', 'active', 'paused', 'updating')",
                    params![now_ms, id.to_string(), as_i64(generation)?],
                )
                .map_err(|_| invariant())?;
            Ok(changed == 1)
        })
    }

    /// Retire a fully drained attachment and release its version referrer.
    pub fn finish_delete(
        &self,
        id: QueueConsumerId,
        generation: u64,
        now_ms: i64,
    ) -> Result<bool, PlatformError> {
        self.db.with_immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE queue_consumers SET state = 'tombstoned', availability = 'unavailable',
                            availability_code = 'QUEUE_CONSUMER_DELETED', updated_at_ms = ?1,
                            deleted_at_ms = ?1, pending_declaration_id = NULL,
                            pending_version_id = NULL
                     WHERE id = ?2 AND consumer_generation = ?3 AND state = 'deleting'",
                    params![now_ms, id.to_string(), as_i64(generation)?],
                )
                .map_err(|_| invariant())?;
            Ok(changed == 1)
        })
    }

    fn finish_state(
        self,
        id: QueueConsumerId,
        generation: u64,
        from: QueueConsumerState,
        to: QueueConsumerState,
        audit: Option<(&'static str, RequestId)>,
        now_ms: i64,
    ) -> Result<bool, PlatformError> {
        self.db.with_immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE queue_consumers SET state = ?1, availability = 'healthy',
                            availability_code = NULL, updated_at_ms = ?2
                     WHERE id = ?3 AND consumer_generation = ?4 AND state = ?5",
                    params![
                        to.as_str(),
                        now_ms,
                        id.to_string(),
                        as_i64(generation)?,
                        from.as_str(),
                    ],
                )
                .map_err(|_| invariant())?;
            if changed == 1
                && let Some((action, request_id)) = audit
            {
                audit_operator_action(tx, id, generation, action, request_id, now_ms)?;
            }
            Ok(changed == 1)
        })
    }
}

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
        let (dlq, dlq_generation) = declaration
            .dead_letter_queue
            .map_or((None, None), |(id, generation)| {
                (Some(id.to_string()), Some(generation))
            });
        tx.execute(
            "INSERT INTO version_queue_consumers
             (id, version_id, queue_id, queue_lifecycle_generation, entrypoint,
              max_batch_size, max_batch_timeout_seconds, max_retries, retry_delay_seconds,
              max_concurrency, dlq_queue_id, dlq_lifecycle_generation, capability_version,
              descriptor_sha256, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                declaration.id.to_string(),
                version_id.to_string(),
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
    }
    Ok(())
}

fn read_record_tx(
    tx: &Transaction<'_>,
    id: QueueConsumerId,
) -> Result<QueueConsumerRecord, PlatformError> {
    tx.query_row(
        "SELECT id, account_id, queue_id, worker_id, declaration_id, version_id,
                pending_declaration_id, pending_version_id, consumer_generation,
                state, availability, availability_code,
                created_at_ms, updated_at_ms, deleted_at_ms
         FROM queue_consumers WHERE id = ?1",
        [id.to_string()],
        map_record,
    )
    .map_err(|_| invariant())
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
        consumer_generation: unsigned(row.get(8)?)?,
        state: row
            .get::<_, String>(9)?
            .parse()
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        availability: row.get(10)?,
        availability_code: row.get(11)?,
        created_at_ms: row.get(12)?,
        updated_at_ms: row.get(13)?,
        deleted_at_ms: row.get(14)?,
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
