//! Independent Queue catalog and immutable producer-binding authority.

use crate::catalog_page::{CatalogColumns, build_catalog_sql, record_catalog_cursor};
use crate::{
    CatalogCursor, CatalogDirection, CatalogListPage, CatalogSort, ControlDb, DeploymentState,
    IdempotencyReservation,
};
use open_compute_core::{
    AccountId, BindingId, DeploymentId, ErrorCode, PlatformError, QueueId, RequestId,
};
use rusqlite::{OptionalExtension as _, Transaction, params, params_from_iter};
use std::str::FromStr;

#[path = "queues/model.rs"]
mod model;
pub use model::*;
#[path = "queues/helpers.rs"]
mod helpers;
use helpers::*;

type MutationReservationRow = (String, Vec<u8>, Option<String>, Option<Vec<u8>>);

/// Queue catalog repository over `control.sqlite`.
#[derive(Clone, Copy, Debug)]
pub struct QueueRepository<'a> {
    db: &'a ControlDb,
}

impl<'a> QueueRepository<'a> {
    /// Bind the central control database.
    #[must_use]
    pub const fn new(db: &'a ControlDb) -> Self {
        Self { db }
    }

    /// Atomically reserve Queue create idempotency and insert its creating identity.
    #[allow(clippy::too_many_arguments)]
    pub fn reserve_create(
        &self,
        account_id: AccountId,
        queue_id: QueueId,
        name: &str,
        config: QueueConfig,
        idempotency_key: &str,
        fingerprint_key_id: &str,
        request_fingerprint: &[u8; 32],
        now_ms: i64,
        expires_at_ms: i64,
        max_live: u32,
    ) -> Result<QueueCreateReservation, PlatformError> {
        validate_name(name)?;
        let config = config.validate()?;
        self.db.with_immediate(|tx| {
            let existing: Option<(String, Vec<u8>, Option<Vec<u8>>)> = tx
                .query_row(
                    "SELECT state, request_fingerprint, response_json
                     FROM control_idempotency
                     WHERE account_id = ?1 AND scope = 'queue.create'
                       AND idempotency_key = ?2",
                    params![account_id.to_string(), idempotency_key],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
                .map_err(|_| db_error())?;
            if let Some((state, stored, response)) = existing {
                if stored.as_slice() != request_fingerprint {
                    return Err(PlatformError::new(
                        ErrorCode::IdempotencyConflict,
                        "Queue idempotency key fingerprint does not match",
                    ));
                }
                return match (state.as_str(), response) {
                    ("complete", Some(bytes)) => Ok(QueueCreateReservation::Complete(bytes)),
                    ("running", _) => Ok(QueueCreateReservation::Running),
                    ("failed", Some(bytes)) => Ok(QueueCreateReservation::Failed(bytes)),
                    _ => Err(invariant()),
                };
            }
            let live_count: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM queues
                     WHERE account_id = ?1 AND state != 'tombstoned'",
                    [account_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(|_| db_error())?;
            if live_count >= i64::from(max_live) {
                return Err(PlatformError::new(
                    ErrorCode::QuotaExceeded,
                    "account Queue count quota was exceeded",
                ));
            }
            tx.execute(
                "INSERT INTO control_idempotency
                 (account_id, scope, idempotency_key, fingerprint_key_id,
                  request_fingerprint, response_json, state, created_at_ms,
                  expires_at_ms, queue_id)
                 VALUES (?1, 'queue.create', ?2, ?3, ?4, NULL, 'running', ?5, ?6, ?7)",
                params![
                    account_id.to_string(),
                    idempotency_key,
                    fingerprint_key_id,
                    request_fingerprint.as_slice(),
                    now_ms,
                    expires_at_ms,
                    queue_id.to_string(),
                ],
            )
            .map_err(|_| db_error())?;
            let queue = insert_creating_tx(tx, account_id, queue_id, name, config, now_ms)?;
            Ok(QueueCreateReservation::Reserved(queue))
        })
    }

    /// Insert a new Queue in projection-pending state.
    pub fn insert_creating(
        &self,
        account_id: AccountId,
        queue_id: QueueId,
        name: &str,
        config: QueueConfig,
        now_ms: i64,
    ) -> Result<QueueRecord, PlatformError> {
        validate_name(name)?;
        let config = config.validate()?;
        self.db
            .with_immediate(|tx| insert_creating_tx(tx, account_id, queue_id, name, config, now_ms))
    }

    /// Read one Queue under its account scope.
    pub fn get(
        &self,
        account_id: AccountId,
        queue_id: QueueId,
    ) -> Result<QueueRecord, PlatformError> {
        self.db
            .with_read(|conn| read_queue_conn(conn, account_id, queue_id)?.ok_or_else(not_found))
    }

    /// List one bounded, filtered, and sorted Queue catalog page.
    #[allow(clippy::too_many_arguments)]
    pub fn list(
        &self,
        account_id: AccountId,
        search: Option<&str>,
        status: Option<QueueState>,
        sort: CatalogSort,
        direction: CatalogDirection,
        after: Option<CatalogCursor>,
        limit: u16,
    ) -> Result<CatalogListPage<QueueRecord>, PlatformError> {
        if limit == 0 || limit > 1000 {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "Queue list limit is invalid",
            ));
        }
        let search = search.map(str::trim).filter(|value| !value.is_empty());
        let exact_id = search.and_then(crate::search_as_queue_id);
        let search_needle = if exact_id.is_some() {
            None
        } else {
            search.map(str::to_lowercase)
        };
        let fetch = u32::from(limit).saturating_add(1);
        let query = build_catalog_sql(
            "SELECT id, account_id, name, state, availability, availability_code,
                    lifecycle_generation, config_generation, delivery_delay_seconds,
                    retention_seconds, max_message_bytes, max_batch_messages,
                    max_batch_bytes, max_backlog_bytes, created_at_ms, updated_at_ms,
                    deleted_at_ms
             FROM queues WHERE account_id = ? AND state != 'tombstoned'",
            CatalogColumns {
                id: "id",
                name: "name",
                state: "state",
                created_at: "created_at_ms",
                updated_at: "updated_at_ms",
            },
            account_id.to_string(),
            search_needle,
            exact_id.map(|id| id.to_string()),
            status.map(|value| value.as_str().to_string()),
            sort,
            direction,
            after,
            fetch,
        )?;
        self.db.with_read(|conn| {
            let mut statement = conn.prepare(&query.text).map_err(|_| db_error())?;
            let rows = statement
                .query_map(params_from_iter(query.values), map_queue)
                .map_err(|_| db_error())?;
            let mut queues = collect(rows)?;
            let next_cursor = if queues.len() > usize::from(limit) {
                queues.pop();
                queues.last().map(|queue| {
                    record_catalog_cursor(
                        sort,
                        direction,
                        &queue.name,
                        queue.created_at_ms,
                        queue.updated_at_ms,
                        &queue.id.to_string(),
                    )
                })
            } else {
                None
            };
            Ok(CatalogListPage {
                items: queues,
                next_cursor,
            })
        })
    }

    /// List a bounded stable batch that requires cross-database reconciliation.
    pub fn list_reconcile(&self, limit: u32) -> Result<Vec<QueueRecord>, PlatformError> {
        if limit == 0 || limit > 1000 {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "Queue reconcile limit is invalid",
            ));
        }
        self.db.with_read(|conn| {
            let mut statement = conn
                .prepare(
                    "SELECT id, account_id, name, state, availability, availability_code,
                            lifecycle_generation, config_generation, delivery_delay_seconds,
                            retention_seconds, max_message_bytes, max_batch_messages,
                            max_batch_bytes, max_backlog_bytes, created_at_ms, updated_at_ms,
                            deleted_at_ms
                     FROM queues
                     WHERE state != 'tombstoned'
                       AND (state IN ('creating', 'deleting') OR availability != 'healthy')
                     ORDER BY updated_at_ms, id LIMIT ?1",
                )
                .map_err(|_| db_error())?;
            let rows = statement
                .query_map([i64::from(limit)], map_queue)
                .map_err(|_| db_error())?;
            collect(rows)
        })
    }

    /// Atomically reserve a restart-recoverable Queue mutation intent.
    #[allow(clippy::too_many_arguments)]
    pub fn reserve_mutation(
        &self,
        account_id: AccountId,
        scope: &str,
        idempotency_key: &str,
        fingerprint_key_id: &str,
        request_fingerprint: &[u8; 32],
        queue_id: QueueId,
        intent_json: &[u8],
        now_ms: i64,
        expires_at_ms: i64,
    ) -> Result<IdempotencyReservation, PlatformError> {
        self.db.with_immediate(|tx| {
            let stored: Option<MutationReservationRow> = tx
                .query_row(
                    "SELECT state, request_fingerprint, queue_id, response_json
                     FROM control_idempotency
                     WHERE account_id = ?1 AND scope = ?2 AND idempotency_key = ?3",
                    params![account_id.to_string(), scope, idempotency_key],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()
                .map_err(|_| db_error())?;
            let Some((state, fingerprint, stored_queue, stored_intent)) = stored else {
                tx.execute(
                    "INSERT INTO control_idempotency
                     (account_id, scope, idempotency_key, fingerprint_key_id,
                      request_fingerprint, response_json, state, created_at_ms,
                      expires_at_ms, queue_id)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'running', ?7, ?8, ?9)",
                    params![
                        account_id.to_string(),
                        scope,
                        idempotency_key,
                        fingerprint_key_id,
                        request_fingerprint.as_slice(),
                        intent_json,
                        now_ms,
                        expires_at_ms,
                        queue_id.to_string(),
                    ],
                )
                .map_err(|_| db_error())?;
                return Ok(IdempotencyReservation::Reserved);
            };
            let queue = queue_id.to_string();
            if fingerprint.as_slice() != request_fingerprint
                || stored_queue.as_deref() != Some(queue.as_str())
            {
                return Err(PlatformError::new(
                    ErrorCode::IdempotencyConflict,
                    "Queue mutation reservation fingerprint or target conflicts",
                ));
            }
            match (state.as_str(), stored_intent) {
                ("running", Some(_)) => Ok(IdempotencyReservation::Running),
                ("complete", Some(response)) => Ok(IdempotencyReservation::Complete(response)),
                ("failed", Some(response)) => Ok(IdempotencyReservation::Failed(response)),
                ("running", _) => Err(PlatformError::new(
                    ErrorCode::IdempotencyConflict,
                    "Queue mutation intent conflicts with the running reservation",
                )),
                _ => Err(invariant()),
            }
        })
    }

    /// Replace one owned running Queue mutation intent before its destructive phase.
    pub fn replace_mutation_intent(
        &self,
        mutation: &RunningQueueMutation,
        intent_json: &[u8],
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE control_idempotency SET response_json = ?1
                     WHERE account_id = ?2 AND scope = ?3 AND idempotency_key = ?4
                       AND state = 'running' AND request_fingerprint = ?5 AND queue_id = ?6",
                    params![
                        intent_json,
                        mutation.account_id.to_string(),
                        mutation.scope,
                        mutation.idempotency_key,
                        mutation.request_fingerprint.as_slice(),
                        mutation.queue_id.to_string(),
                    ],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(PlatformError::new(
                    ErrorCode::IdempotencyConflict,
                    "Queue mutation reservation is no longer owned",
                ));
            }
            Ok(())
        })
    }

    /// List a bounded batch of restart-recoverable running Queue mutation intents.
    pub fn list_running_mutations(
        &self,
        limit: u32,
    ) -> Result<Vec<RunningQueueMutation>, PlatformError> {
        if limit == 0 || limit > 1000 {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "Queue mutation reconcile limit is invalid",
            ));
        }
        self.db.with_read(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT account_id, scope, idempotency_key, request_fingerprint,
                            queue_id, response_json
                     FROM control_idempotency
                     WHERE state = 'running' AND queue_id IS NOT NULL
                       AND response_json IS NOT NULL
                       AND (scope LIKE 'queue.patch:%' OR scope LIKE 'queue.delete:%')
                     ORDER BY created_at_ms, account_id, scope, idempotency_key LIMIT ?1",
                )
                .map_err(|_| db_error())?;
            let rows = statement
                .query_map([i64::from(limit)], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Vec<u8>>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, Vec<u8>>(5)?,
                    ))
                })
                .map_err(|_| db_error())?;
            let mut output = Vec::new();
            for row in rows {
                let (account, scope, key, fingerprint, queue, intent_json) =
                    row.map_err(|_| db_error())?;
                let request_fingerprint: [u8; 32] =
                    fingerprint.try_into().map_err(|_| invariant())?;
                output.push(RunningQueueMutation {
                    account_id: AccountId::from_str(&account).map_err(|_| invariant())?,
                    scope,
                    idempotency_key: key,
                    request_fingerprint,
                    queue_id: QueueId::from_str(&queue).map_err(|_| invariant())?,
                    intent_json,
                });
            }
            Ok(output)
        })
    }

    /// Complete the exact running create reservation after startup reconciliation.
    pub fn complete_reconciled_create(
        &self,
        queue: &QueueRecord,
        response: &[u8],
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE control_idempotency
                     SET state = 'complete', response_json = ?1
                     WHERE account_id = ?2 AND scope = 'queue.create'
                       AND queue_id = ?3 AND state = 'running'",
                    params![response, queue.account_id.to_string(), queue.id.to_string()],
                )
                .map_err(|_| db_error())?;
            if changed > 1 {
                return Err(invariant());
            }
            Ok(())
        })
    }

    /// Complete the create lifecycle only after the exact scheduler projection exists.
    pub fn mark_ready(
        &self,
        account_id: AccountId,
        queue_id: QueueId,
        now_ms: i64,
    ) -> Result<QueueRecord, PlatformError> {
        self.db.with_immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE queues SET state = 'ready', availability = 'healthy',
                            availability_code = NULL, updated_at_ms = ?1
                     WHERE id = ?2 AND account_id = ?3 AND state = 'creating'
                       AND lifecycle_generation = 1 AND config_generation = 1",
                    params![now_ms, queue_id.to_string(), account_id.to_string()],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(not_ready());
            }
            read_queue_tx(tx, account_id, queue_id)
        })
    }

    /// Rename a healthy ready Queue without changing either generation.
    pub fn rename(
        &self,
        account_id: AccountId,
        queue_id: QueueId,
        name: &str,
        request_id: RequestId,
        now_ms: i64,
    ) -> Result<QueueRecord, PlatformError> {
        validate_name(name)?;
        self.db.with_immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE queues SET name = ?1, updated_at_ms = ?2
                     WHERE id = ?3 AND account_id = ?4 AND state = 'ready'
                       AND availability = 'healthy'",
                    params![name, now_ms, queue_id.to_string(), account_id.to_string()],
                )
                .map_err(|error| {
                    if error.to_string().contains("UNIQUE") {
                        PlatformError::new(
                            ErrorCode::QueueNameConflict,
                            "live Queue name conflicts",
                        )
                    } else {
                        db_error()
                    }
                })?;
            if changed != 1 {
                return Err(not_ready());
            }
            audit(tx, account_id, "queue.rename", queue_id, request_id, now_ms)?;
            read_queue_tx(tx, account_id, queue_id)
        })
    }

    /// Persist a new Queue config generation after the scheduler accepting fence is installed.
    pub fn write_config_pending(
        &self,
        account_id: AccountId,
        queue_id: QueueId,
        expected_generation: u64,
        config: QueueConfig,
        now_ms: i64,
    ) -> Result<QueueRecord, PlatformError> {
        let config = config.validate()?;
        let next = expected_generation.checked_add(1).ok_or_else(invariant)?;
        self.db.with_immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE queues SET config_generation = ?1, delivery_delay_seconds = ?2,
                            retention_seconds = ?3, max_message_bytes = ?4,
                            max_batch_messages = ?5, max_batch_bytes = ?6,
                            max_backlog_bytes = ?7, availability = 'degraded',
                            availability_code = 'QUEUE_CONFIG_PENDING', updated_at_ms = ?8
                     WHERE id = ?9 AND account_id = ?10 AND state = 'ready'
                       AND availability = 'healthy' AND config_generation = ?11",
                    params![
                        i64::try_from(next).map_err(|_| invariant())?,
                        i64::from(config.delivery_delay_seconds),
                        i64::from(config.retention_seconds),
                        i64::try_from(config.max_message_bytes).map_err(|_| invariant())?,
                        i64::from(config.max_batch_messages),
                        i64::try_from(config.max_batch_bytes).map_err(|_| invariant())?,
                        i64::try_from(config.max_backlog_bytes).map_err(|_| invariant())?,
                        now_ms,
                        queue_id.to_string(),
                        account_id.to_string(),
                        i64::try_from(expected_generation).map_err(|_| invariant())?,
                    ],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(PlatformError::new(
                    ErrorCode::QueueConfigPending,
                    "Queue config generation is stale or unavailable",
                ));
            }
            read_queue_tx(tx, account_id, queue_id)
        })
    }

    /// Mark an exact projected config generation healthy.
    pub fn mark_config_healthy(
        &self,
        account_id: AccountId,
        queue_id: QueueId,
        config_generation: u64,
        request_id: RequestId,
        now_ms: i64,
    ) -> Result<QueueRecord, PlatformError> {
        self.db.with_immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE queues SET availability = 'healthy', availability_code = NULL,
                            updated_at_ms = ?1
                     WHERE id = ?2 AND account_id = ?3 AND state = 'ready'
                       AND availability = 'degraded'
                       AND availability_code = 'QUEUE_CONFIG_PENDING'
                       AND config_generation = ?4",
                    params![
                        now_ms,
                        queue_id.to_string(),
                        account_id.to_string(),
                        i64::try_from(config_generation).map_err(|_| invariant())?,
                    ],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(PlatformError::new(
                    ErrorCode::QueueConfigPending,
                    "Queue config projection did not converge",
                ));
            }
            audit(
                tx,
                account_id,
                "queue.configure",
                queue_id,
                request_id,
                now_ms,
            )?;
            read_queue_tx(tx, account_id, queue_id)
        })
    }

    /// Fence a healthy unreferenced Queue before scheduler deletion.
    pub fn begin_delete(
        &self,
        account_id: AccountId,
        queue_id: QueueId,
        expected_generation: u64,
        now_ms: i64,
    ) -> Result<QueueRecord, PlatformError> {
        self.db.with_immediate(|tx| {
            let referenced: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM queue_referrers WHERE queue_id = ?1)",
                    [queue_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(|_| db_error())?;
            if referenced {
                return Err(PlatformError::new(
                    ErrorCode::QueueReferenced,
                    "Queue still has a live referrer",
                ));
            }
            let changed = tx
                .execute(
                    "UPDATE queues SET state = 'deleting', availability = 'degraded',
                            availability_code = 'QUEUE_DELETE_PENDING', updated_at_ms = ?1
                     WHERE id = ?2 AND account_id = ?3 AND state = 'ready'
                       AND availability = 'healthy' AND lifecycle_generation = ?4",
                    params![
                        now_ms,
                        queue_id.to_string(),
                        account_id.to_string(),
                        i64::try_from(expected_generation).map_err(|_| invariant())?,
                    ],
                )
                .map_err(|error| {
                    if error.to_string().contains("referenced") {
                        PlatformError::new(ErrorCode::QueueReferenced, "Queue is referenced")
                    } else {
                        db_error()
                    }
                })?;
            if changed != 1 {
                return Err(not_ready());
            }
            read_queue_tx(tx, account_id, queue_id)
        })
    }

    /// Finish an exact Queue tombstone after scheduler state has been removed.
    pub fn mark_tombstoned(
        &self,
        account_id: AccountId,
        queue_id: QueueId,
        request_id: RequestId,
        now_ms: i64,
    ) -> Result<QueueRecord, PlatformError> {
        self.db.with_immediate(|tx| {
            let changed = tx
                .execute(
                    "UPDATE queues SET state = 'tombstoned', availability = 'degraded',
                            availability_code = 'QUEUE_DELETED', deleted_at_ms = ?1,
                            updated_at_ms = ?1
                     WHERE id = ?2 AND account_id = ?3 AND state = 'deleting'",
                    params![now_ms, queue_id.to_string(), account_id.to_string()],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(not_ready());
            }
            audit(tx, account_id, "queue.delete", queue_id, request_id, now_ms)?;
            read_queue_tx(tx, account_id, queue_id)
        })
    }

    /// Read immutable Queue producer bindings for descriptor reconstruction.
    pub fn deployment_bindings(
        &self,
        deployment_id: DeploymentId,
    ) -> Result<Vec<QueueProducerBindingRecord>, PlatformError> {
        self.db
            .with_read(|conn| read_deployment_bindings_conn(conn, deployment_id))
    }

    /// Authorize one private producer call from immutable deployment authority.
    pub fn authorize(
        &self,
        binding_id: BindingId,
        deployment_id: DeploymentId,
        descriptor_sha256: &[u8; 32],
    ) -> Result<AuthorizedQueueBinding, PlatformError> {
        self.db.with_read(|conn| {
            let row = conn
                .query_row(
                    "SELECT b.id, b.deployment_id, b.name, b.queue_id,
                            b.queue_lifecycle_generation, b.capability_version,
                            b.descriptor_sha256, b.created_at_ms,
                            q.id, q.account_id, q.name, q.state, q.availability,
                            q.availability_code, q.lifecycle_generation, q.config_generation,
                            q.delivery_delay_seconds, q.retention_seconds, q.max_message_bytes,
                            q.max_batch_messages, q.max_batch_bytes, q.max_backlog_bytes,
                            q.created_at_ms, q.updated_at_ms, q.deleted_at_ms,
                            w.account_id, d.state,
                            EXISTS(SELECT 1 FROM queue_referrers r
                              WHERE r.queue_id = b.queue_id
                                AND r.referrer_kind = 'producer_binding'
                                AND r.referrer_id = b.id)
                     FROM queue_producer_bindings b
                     JOIN worker_deployments d ON d.id = b.deployment_id
                     JOIN workers w ON w.id = d.worker_id
                     JOIN queues q ON q.id = b.queue_id
                     WHERE b.id = ?1 AND b.deployment_id = ?2",
                    params![binding_id.to_string(), deployment_id.to_string()],
                    |row| {
                        let binding = map_binding_offset(row, 0)?;
                        let queue = map_queue_offset(row, 8)?;
                        let account: String = row.get(25)?;
                        let deployment_state: String = row.get(26)?;
                        let referrer: bool = row.get(27)?;
                        Ok((binding, queue, account, deployment_state, referrer))
                    },
                )
                .optional()
                .map_err(|_| invariant())?;
            let Some((binding, queue, account, deployment_state, referrer)) = row else {
                return Err(not_found());
            };
            let account_id = AccountId::from_str(&account).map_err(|_| invariant())?;
            if binding.descriptor_sha256 != *descriptor_sha256
                || binding.capability_version != QUEUE_PRODUCER_CAPABILITY_VERSION
                || deployment_state != DeploymentState::Ready.as_str()
                || account_id != queue.account_id
                || binding.queue_id != queue.id
                || binding.queue_lifecycle_generation != queue.lifecycle_generation
                || !referrer
            {
                return Err(invariant());
            }
            if queue.state != QueueState::Ready {
                return Err(not_ready());
            }
            if queue.availability != QueueAvailability::Healthy {
                return Err(
                    if queue.availability_code.as_deref() == Some("QUEUE_CONFIG_PENDING") {
                        PlatformError::new(ErrorCode::QueueConfigPending, "Queue config is pending")
                    } else {
                        PlatformError::new(
                            ErrorCode::QueueStorageUnavailable,
                            "Queue is unavailable",
                        )
                    },
                );
            }
            Ok(AuthorizedQueueBinding {
                binding,
                queue,
                account_id,
            })
        })
    }
}

/// Insert Queue producer bindings inside the deployment staging transaction.
pub(crate) fn insert_staging_bindings(
    tx: &Transaction<'_>,
    deployment_id: DeploymentId,
    bindings: &[NewQueueProducerBinding],
    now_ms: i64,
) -> Result<(), PlatformError> {
    for binding in bindings {
        tx.execute(
            "INSERT INTO queue_producer_bindings
             (id, deployment_id, name, queue_id, queue_lifecycle_generation,
              capability_version, descriptor_sha256, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                binding.id.to_string(),
                deployment_id.to_string(),
                binding.name,
                binding.queue_id.to_string(),
                i64::try_from(binding.queue_lifecycle_generation).map_err(|_| invariant())?,
                i64::from(binding.capability_version),
                binding.descriptor_sha256.as_slice(),
                now_ms,
            ],
        )
        .map_err(|_| invariant())?;
    }
    Ok(())
}

fn insert_creating_tx(
    tx: &Transaction<'_>,
    account_id: AccountId,
    queue_id: QueueId,
    name: &str,
    config: QueueConfig,
    now_ms: i64,
) -> Result<QueueRecord, PlatformError> {
    let account: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM accounts WHERE id = ?1 AND deleted_at_ms IS NULL)",
            [account_id.to_string()],
            |row| row.get(0),
        )
        .map_err(|_| db_error())?;
    if !account {
        return Err(PlatformError::new(
            ErrorCode::AccountNotFound,
            "Queue account was not found",
        ));
    }
    tx.execute(
        "INSERT INTO queues
         (id, account_id, name, state, availability, availability_code,
          lifecycle_generation, config_generation, delivery_delay_seconds,
          retention_seconds, max_message_bytes, max_batch_messages, max_batch_bytes,
          max_backlog_bytes, created_at_ms, updated_at_ms, deleted_at_ms)
         VALUES (?1, ?2, ?3, 'creating', 'degraded', 'QUEUE_PROJECTION_PENDING',
                 1, 1, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10, NULL)",
        params![
            queue_id.to_string(),
            account_id.to_string(),
            name,
            i64::from(config.delivery_delay_seconds),
            i64::from(config.retention_seconds),
            i64::try_from(config.max_message_bytes).map_err(|_| invariant())?,
            i64::from(config.max_batch_messages),
            i64::try_from(config.max_batch_bytes).map_err(|_| invariant())?,
            i64::try_from(config.max_backlog_bytes).map_err(|_| invariant())?,
            now_ms,
        ],
    )
    .map_err(|error| {
        if error.to_string().contains("UNIQUE") {
            PlatformError::new(ErrorCode::QueueNameConflict, "live Queue name conflicts")
        } else {
            db_error()
        }
    })?;
    read_queue_tx(tx, account_id, queue_id)
}

pub(crate) fn read_deployment_bindings_conn(
    conn: &rusqlite::Connection,
    deployment_id: DeploymentId,
) -> Result<Vec<QueueProducerBindingRecord>, PlatformError> {
    let mut statement = conn
        .prepare(
            "SELECT b.id, b.deployment_id, b.name, b.queue_id,
                    b.queue_lifecycle_generation, b.capability_version,
                    b.descriptor_sha256, b.created_at_ms,
                    q.state, q.availability, q.availability_code,
                    q.lifecycle_generation, q.account_id, w.account_id,
                    EXISTS(SELECT 1 FROM queue_referrers r
                      WHERE r.queue_id = b.queue_id
                        AND r.referrer_kind = 'producer_binding'
                        AND r.referrer_id = b.id)
             FROM queue_producer_bindings b
             JOIN queues q ON q.id = b.queue_id
             JOIN worker_deployments d ON d.id = b.deployment_id
             JOIN workers w ON w.id = d.worker_id
             WHERE b.deployment_id = ?1 ORDER BY b.name, b.id",
        )
        .map_err(|_| db_error())?;
    let rows = statement
        .query_map([deployment_id.to_string()], |row| {
            let binding = map_binding_offset(row, 0)?;
            let state: String = row.get(8)?;
            let availability: String = row.get(9)?;
            let availability_code: Option<String> = row.get(10)?;
            let generation: i64 = row.get(11)?;
            let queue_account: String = row.get(12)?;
            let worker_account: String = row.get(13)?;
            let referrer: bool = row.get(14)?;
            if state != "ready"
                || !((availability == "healthy" && availability_code.is_none())
                    || (availability == "degraded"
                        && availability_code.as_deref() == Some("QUEUE_CONFIG_PENDING")))
                || u64::try_from(generation).ok() != Some(binding.queue_lifecycle_generation)
                || queue_account != worker_account
                || !referrer
            {
                return Err(rusqlite::Error::InvalidQuery);
            }
            Ok(binding)
        })
        .map_err(|_| db_error())?;
    collect(rows)
}

fn read_queue_conn(
    conn: &rusqlite::Connection,
    account_id: AccountId,
    queue_id: QueueId,
) -> Result<Option<QueueRecord>, PlatformError> {
    conn.query_row(
        "SELECT id, account_id, name, state, availability, availability_code,
                lifecycle_generation, config_generation, delivery_delay_seconds,
                retention_seconds, max_message_bytes, max_batch_messages, max_batch_bytes,
                max_backlog_bytes, created_at_ms, updated_at_ms, deleted_at_ms
         FROM queues WHERE id = ?1 AND account_id = ?2",
        params![queue_id.to_string(), account_id.to_string()],
        map_queue,
    )
    .optional()
    .map_err(|_| db_error())
}

fn read_queue_tx(
    tx: &Transaction<'_>,
    account_id: AccountId,
    queue_id: QueueId,
) -> Result<QueueRecord, PlatformError> {
    tx.query_row(
        "SELECT id, account_id, name, state, availability, availability_code,
                lifecycle_generation, config_generation, delivery_delay_seconds,
                retention_seconds, max_message_bytes, max_batch_messages, max_batch_bytes,
                max_backlog_bytes, created_at_ms, updated_at_ms, deleted_at_ms
         FROM queues WHERE id = ?1 AND account_id = ?2",
        params![queue_id.to_string(), account_id.to_string()],
        map_queue,
    )
    .map_err(|_| invariant())
}

fn map_queue(row: &rusqlite::Row<'_>) -> rusqlite::Result<QueueRecord> {
    map_queue_offset(row, 0)
}

fn map_queue_offset(row: &rusqlite::Row<'_>, offset: usize) -> rusqlite::Result<QueueRecord> {
    let id: String = row.get(offset)?;
    let account: String = row.get(offset + 1)?;
    let state: String = row.get(offset + 3)?;
    let availability: String = row.get(offset + 4)?;
    let lifecycle: i64 = row.get(offset + 6)?;
    let generation: i64 = row.get(offset + 7)?;
    let delay: i64 = row.get(offset + 8)?;
    let retention: i64 = row.get(offset + 9)?;
    let message_bytes: i64 = row.get(offset + 10)?;
    let batch_messages: i64 = row.get(offset + 11)?;
    let batch_bytes: i64 = row.get(offset + 12)?;
    let backlog_bytes: i64 = row.get(offset + 13)?;
    Ok(QueueRecord {
        id: QueueId::from_str(&id).map_err(|_| rusqlite::Error::InvalidQuery)?,
        account_id: AccountId::from_str(&account).map_err(|_| rusqlite::Error::InvalidQuery)?,
        name: row.get(offset + 2)?,
        state: QueueState::from_str(&state).map_err(|_| rusqlite::Error::InvalidQuery)?,
        availability: QueueAvailability::from_str(&availability)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        availability_code: row.get(offset + 5)?,
        lifecycle_generation: u64::try_from(lifecycle)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        config_generation: u64::try_from(generation).map_err(|_| rusqlite::Error::InvalidQuery)?,
        config: QueueConfig {
            delivery_delay_seconds: u32::try_from(delay)
                .map_err(|_| rusqlite::Error::InvalidQuery)?,
            retention_seconds: u32::try_from(retention)
                .map_err(|_| rusqlite::Error::InvalidQuery)?,
            max_message_bytes: u64::try_from(message_bytes)
                .map_err(|_| rusqlite::Error::InvalidQuery)?,
            max_batch_messages: u32::try_from(batch_messages)
                .map_err(|_| rusqlite::Error::InvalidQuery)?,
            max_batch_bytes: u64::try_from(batch_bytes)
                .map_err(|_| rusqlite::Error::InvalidQuery)?,
            max_backlog_bytes: u64::try_from(backlog_bytes)
                .map_err(|_| rusqlite::Error::InvalidQuery)?,
        }
        .validate()
        .map_err(|_| rusqlite::Error::InvalidQuery)?,
        created_at_ms: row.get(offset + 14)?,
        updated_at_ms: row.get(offset + 15)?,
        deleted_at_ms: row.get(offset + 16)?,
    })
}

fn map_binding_offset(
    row: &rusqlite::Row<'_>,
    offset: usize,
) -> rusqlite::Result<QueueProducerBindingRecord> {
    let id: String = row.get(offset)?;
    let deployment: String = row.get(offset + 1)?;
    let queue: String = row.get(offset + 3)?;
    let generation: i64 = row.get(offset + 4)?;
    let capability: i64 = row.get(offset + 5)?;
    let digest: Vec<u8> = row.get(offset + 6)?;
    Ok(QueueProducerBindingRecord {
        id: BindingId::from_str(&id).map_err(|_| rusqlite::Error::InvalidQuery)?,
        deployment_id: DeploymentId::from_str(&deployment)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        name: row.get(offset + 2)?,
        queue_id: QueueId::from_str(&queue).map_err(|_| rusqlite::Error::InvalidQuery)?,
        queue_lifecycle_generation: u64::try_from(generation)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        capability_version: u32::try_from(capability).map_err(|_| rusqlite::Error::InvalidQuery)?,
        descriptor_sha256: digest
            .as_slice()
            .try_into()
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        created_at_ms: row.get(offset + 7)?,
    })
}
