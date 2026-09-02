//! Independent SQLite authority for the bounded P0.8 scheduler projection.

use open_compute_core::{
    DurableObjectId, ErrorCode, PlatformError, ResourceId, VersionId, WorkloadSummary,
};
use rand::TryRngCore as _;
use rusqlite::{Connection, OpenFlags, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uuid::Uuid;

#[path = "scheduler/cron.rs"]
mod cron;
#[path = "scheduler/inspection.rs"]
mod inspection;
#[path = "scheduler/migrations.rs"]
mod migration_registry;
#[path = "scheduler/queue.rs"]
mod queue;
#[path = "scheduler/queue_consumer.rs"]
mod queue_consumer;
#[path = "scheduler/summary.rs"]
mod summary;
#[path = "scheduler/wake.rs"]
mod wake;
#[path = "scheduler/workflow.rs"]
mod workflow;

pub use workflow::{
    ClaimedWorkflowRun, WorkflowClaimCursor, WorkflowCompletion, WorkflowDatabaseInspection,
    WorkflowDelayResolution, WorkflowFailure, WorkflowInspection, WorkflowInstanceAction,
    WorkflowInstanceInspection, WorkflowInstanceRecord, WorkflowState, WorkflowStepAttempt,
    WorkflowStepGrant, WorkflowStepInspection, WorkflowStepOutcome, WorkflowStepResult,
    inspect_workflow_databases,
};

pub use cron::{
    ClaimedCronRun, CronCompletion, CronCompletionResult, CronRuntimeInspection,
    CronScheduleProjection, CronSlotSummary,
};
pub(crate) use inspection::inspect_scheduler_schema_version;
pub use inspection::{
    CronInspectionSummary, P23CrossDatabaseInspection, QueueConsumerInspectionSummary,
    QueueInspectionSummary, SchedulerInspection, inspect_p23_cross_database, inspect_scheduler_db,
};
use migration_registry::{SCHEDULER_MIGRATIONS, validate_registry, verify_applied};
pub use queue::{
    QueueContentType, QueueCounterMismatch, QueueDeleteBatch, QueueEnqueueRequest,
    QueueEnqueueResult, QueueMessageInput, QueueMetrics, QueueProjection,
};
pub use queue_consumer::{
    ClaimedQueueBatch, ClaimedQueueMessage, QueueCompletionAction, QueueCompletionDecision,
    QueueCompletionSummary, QueueConsumerProjection, QueueConsumerRuntimeInspection,
    QueueDlqForwardSummary,
};
use summary::{summary_connection, workload_summary_connection};
pub use wake::{SchedulerWakeFuture, SchedulerWakeSignal};

const SCHEMA_VERSION: i64 = SCHEDULER_MIGRATIONS.len() as i64;
const DATA_FORMAT: &str = "open-compute-scheduler-v1";

/// Current scheduler database schema implemented by this binary.
#[must_use]
pub const fn current_scheduler_schema_version() -> i64 {
    SCHEMA_VERSION
}

/// Ordered scheduler migration identities shipped by this binary.
#[must_use]
pub fn scheduler_migration_registry() -> Vec<(i64, &'static str, [u8; 32])> {
    SCHEDULER_MIGRATIONS
        .iter()
        .map(|migration| (migration.version, migration.name, *migration.checksum))
        .collect()
}
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SchedulerMigrationFault {
    BeforeExecution,
    BeforeMigrationRow,
    AfterCommit,
}

/// One authoritative alarm projection submitted by the object-local shim.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AlarmProjection {
    /// Owning Durable Object namespace resource.
    pub namespace_resource_id: ResourceId,
    /// Opaque object identity.
    pub object_id: DurableObjectId,
    /// Object lifecycle generation.
    pub object_generation: u64,
    /// Object-authority random mutation fence.
    pub row_token: String,
    /// Persisted Unix due time in milliseconds.
    pub due_at_ms: i64,
    /// Version observed by the trusted projection backend.
    pub target_version_id: VersionId,
    /// Worker execution generation observed by the projection backend.
    pub execution_generation: u64,
    /// Handler retry count already consumed.
    pub retry_count: u8,
}

/// One due projection claimed under a random, expiring lease.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimedJob {
    /// Random scheduler job identity.
    pub id: String,
    /// Owning namespace resource.
    pub namespace_resource_id: ResourceId,
    /// Opaque object identity.
    pub object_id: DurableObjectId,
    /// Object lifecycle generation.
    pub object_generation: u64,
    /// Object-authority mutation fence.
    pub row_token: String,
    /// Persisted due time.
    pub due_at_ms: i64,
    /// Version recorded on the projection.
    pub target_version_id: VersionId,
    /// Execution generation recorded on the projection.
    pub execution_generation: u64,
    /// Handler retry count already consumed.
    pub retry_count: u8,
    /// Random claim lease fence.
    pub claim_token: String,
    /// Persisted lease expiry.
    pub claim_until_ms: i64,
}

/// Conditional result applied after a claimed job leaves workerd.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClaimResult {
    /// Authority was consumed, absent, stale, or exhausted; remove this exact projection.
    Delete,
    /// Authority still exists at a new due time; reschedule this exact projection.
    Reschedule {
        /// New authoritative due time.
        due_at_ms: i64,
        /// New authoritative retry count.
        retry_count: u8,
        /// Stable low-cardinality failure code, when retrying.
        last_error_code: Option<&'static str>,
    },
    /// Exhaustion cleanup must run before the projection can be removed.
    MarkDiscarding {
        /// Stable failure code retained for operator inspection.
        last_error_code: &'static str,
    },
}

/// Low-cardinality state counts and lag information for health and doctor.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchedulerSummary {
    /// Scheduled row count.
    pub scheduled: u64,
    /// Claimed row count.
    pub claimed: u64,
    /// Discarding row count.
    pub discarding: u64,
    /// Oldest scheduled due time, if any.
    pub oldest_due_at_ms: Option<i64>,
    /// Number of currently expired claims.
    pub expired_claims: u64,
}

/// Single-process owner of `scheduler.sqlite` and its bounded writer lane.
#[derive(Debug)]
pub struct SchedulerStore {
    connection: Mutex<Connection>,
    wake: Arc<SchedulerWakeSignal>,
}

impl SchedulerStore {
    /// Open, migrate, and integrity-check an independently owned scheduler database.
    pub fn open(
        path: &std::path::Path,
        busy_timeout_ms: u64,
        now_ms: i64,
    ) -> Result<Self, PlatformError> {
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NOFOLLOW;
        let open_path = crate::control_db::leaf_nofollow_path(path)?;
        let connection = Connection::open_with_flags(open_path, flags).map_err(map_open_error)?;
        connection
            .busy_timeout(Duration::from_millis(busy_timeout_ms))
            .map_err(map_sql_error)?;
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(map_sql_error)?;
        connection
            .pragma_update(None, "synchronous", "FULL")
            .map_err(map_sql_error)?;
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .map_err(map_sql_error)?;
        connection
            .pragma_update(None, "trusted_schema", "OFF")
            .map_err(map_sql_error)?;
        let store = Self {
            connection: Mutex::new(connection),
            wake: Arc::new(SchedulerWakeSignal::default()),
        };
        store.migrate(
            now_ms,
            #[cfg(test)]
            None,
        )?;
        store.quick_check()?;
        store.recover_expired(now_ms, 10_000)?;
        Ok(store)
    }

    fn migrate(
        &self,
        now_ms: i64,
        #[cfg(test)] fault: Option<SchedulerMigrationFault>,
    ) -> Result<(), PlatformError> {
        validate_registry(SCHEDULER_MIGRATIONS)?;
        let mut connection = self.lock()?;
        let mut version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(map_sql_error)?;
        if version > SCHEMA_VERSION {
            return Err(PlatformError::new(
                ErrorCode::SchemaTooNew,
                "scheduler database schema is newer than this binary",
            ));
        }
        if version > 0 {
            // Check old authority before any forward DDL, not only after it has committed.
            verify_applied(&connection, version)?;
        }
        for migration in SCHEDULER_MIGRATIONS {
            if migration.version <= version {
                continue;
            }
            let tx = connection
                .transaction_with_behavior(TransactionBehavior::Exclusive)
                .map_err(map_sql_error)?;
            #[cfg(test)]
            if fault == Some(SchedulerMigrationFault::BeforeExecution) {
                return Err(corrupt());
            }
            tx.execute_batch(migration.sql).map_err(map_sql_error)?;
            if migration.version >= 5 {
                workflow::verify_operation_progress(&tx)?;
            }
            #[cfg(test)]
            if fault == Some(SchedulerMigrationFault::BeforeMigrationRow) {
                return Err(corrupt());
            }
            if migration.version == 1 {
                tx.execute(
                    "INSERT INTO scheduler_meta
                     (singleton, schema_version, data_format, created_at_ms, updated_at_ms)
                     VALUES (1, ?1, ?2, ?3, ?3)",
                    params![migration.version, DATA_FORMAT, now_ms],
                )
                .map_err(map_sql_error)?;
            } else {
                tx.execute(
                    "UPDATE scheduler_meta
                     SET schema_version = ?1, updated_at_ms = ?2
                     WHERE singleton = 1 AND schema_version = ?3",
                    params![migration.version, now_ms, migration.version - 1],
                )
                .map_err(map_sql_error)?;
            }
            tx.execute(
                "INSERT INTO scheduler_migrations
                 (version, name, checksum_sha256, applied_at_ms, app_version)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    migration.version,
                    migration.name,
                    migration.checksum.as_slice(),
                    now_ms,
                    APP_VERSION,
                ],
            )
            .map_err(map_sql_error)?;
            tx.pragma_update(None, "user_version", migration.version)
                .map_err(map_sql_error)?;
            tx.commit().map_err(map_sql_error)?;
            #[cfg(test)]
            if fault == Some(SchedulerMigrationFault::AfterCommit) {
                return Err(corrupt());
            }
            version = migration.version;
        }
        verify_applied(&connection, version)?;
        workflow::verify_operation_progress(&connection)?;
        Ok(())
    }

    /// Process-local committed-mutation notification used by the wake coordinator.
    #[must_use]
    pub fn wake_signal(&self) -> Arc<SchedulerWakeSignal> {
        self.wake.clone()
    }

    /// Run a bounded SQLite integrity check without rebuilding on failure.
    pub fn quick_check(&self) -> Result<(), PlatformError> {
        let connection = self.lock()?;
        let status: String = connection
            .pragma_query_value(None, "quick_check", |row| row.get(0))
            .map_err(map_sql_error)?;
        if status != "ok" {
            return Err(corrupt());
        }
        Ok(())
    }

    /// Insert or replace one object-generation projection, clearing any old lease.
    pub fn upsert_alarm(
        &self,
        projection: &AlarmProjection,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        validate_projection(projection)?;
        let connection = self.lock()?;
        connection
            .execute(
                "INSERT INTO scheduled_jobs
                 (id, kind, namespace_resource_id, object_id, object_generation, row_token,
                  due_at_ms, target_version_id, execution_generation, state, retry_count,
                  claim_token, claim_until_ms, last_error_code, created_at_ms, updated_at_ms)
                 VALUES (?1, 'do_alarm', ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'scheduled', ?9,
                         NULL, NULL, NULL, ?10, ?10)
                 ON CONFLICT(namespace_resource_id, object_id, object_generation) DO UPDATE SET
                   row_token = excluded.row_token,
                   due_at_ms = excluded.due_at_ms,
                   target_version_id = excluded.target_version_id,
                   execution_generation = excluded.execution_generation,
                   state = 'scheduled', retry_count = excluded.retry_count,
                   claim_token = NULL, claim_until_ms = NULL, last_error_code = NULL,
                   updated_at_ms = excluded.updated_at_ms",
                params![
                    Uuid::now_v7().hyphenated().to_string(),
                    projection.namespace_resource_id.to_string(),
                    projection.object_id.to_string(),
                    i64::try_from(projection.object_generation).map_err(|_| invalid())?,
                    projection.row_token,
                    projection.due_at_ms,
                    projection.target_version_id.to_string(),
                    i64::try_from(projection.execution_generation).map_err(|_| invalid())?,
                    i64::from(projection.retry_count),
                    now_ms,
                ],
            )
            .map_err(map_sql_error)?;
        drop(connection);
        self.wake.notify();
        Ok(())
    }

    /// Delete only the projection carrying the supplied object-authority token.
    pub fn delete_alarm_exact(
        &self,
        namespace_resource_id: ResourceId,
        object_id: DurableObjectId,
        object_generation: u64,
        row_token: &str,
    ) -> Result<bool, PlatformError> {
        validate_token(row_token)?;
        let connection = self.lock()?;
        let affected = connection
            .execute(
                "DELETE FROM scheduled_jobs
                 WHERE namespace_resource_id = ?1 AND object_id = ?2
                   AND object_generation = ?3 AND row_token = ?4",
                params![
                    namespace_resource_id.to_string(),
                    object_id.to_string(),
                    i64::try_from(object_generation).map_err(|_| invalid())?,
                    row_token,
                ],
            )
            .map_err(map_sql_error)?;
        drop(connection);
        if affected > 0 {
            self.wake.notify();
        }
        Ok(affected == 1)
    }

    /// Remove every projection for an object lifecycle generation during its delete fence.
    pub fn delete_object(
        &self,
        namespace_resource_id: ResourceId,
        object_id: DurableObjectId,
        object_generation: u64,
    ) -> Result<u64, PlatformError> {
        let connection = self.lock()?;
        let affected = connection
            .execute(
                "DELETE FROM scheduled_jobs
                 WHERE namespace_resource_id = ?1 AND object_id = ?2 AND object_generation = ?3",
                params![
                    namespace_resource_id.to_string(),
                    object_id.to_string(),
                    i64::try_from(object_generation).map_err(|_| invalid())?,
                ],
            )
            .map_err(map_sql_error)?;
        drop(connection);
        if affected > 0 {
            self.wake.notify();
        }
        u64::try_from(affected).map_err(|_| invalid())
    }

    /// Recover expired leases and claim a deterministic bounded due batch atomically.
    pub fn claim_due(
        &self,
        now_ms: i64,
        lease_ms: u64,
        batch: u32,
    ) -> Result<Vec<ClaimedJob>, PlatformError> {
        self.claim_due_with_recovery(now_ms, lease_ms, batch)
            .map(|(jobs, _)| jobs)
    }

    /// Claim a due batch and report the number of expired leases recovered in the transaction.
    pub fn claim_due_with_recovery(
        &self,
        now_ms: i64,
        lease_ms: u64,
        batch: u32,
    ) -> Result<(Vec<ClaimedJob>, u64), PlatformError> {
        if lease_ms == 0 || batch == 0 {
            return Err(invalid());
        }
        let claim_until_ms = now_ms
            .checked_add(i64::try_from(lease_ms).map_err(|_| invalid())?)
            .ok_or_else(invalid)?;
        let mut connection = self.lock()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sql_error)?;
        let recovered = tx
            .execute(
                "UPDATE scheduled_jobs
                 SET state = 'scheduled', claim_token = NULL, claim_until_ms = NULL,
                     updated_at_ms = ?1
                 WHERE id IN (
                   SELECT id FROM scheduled_jobs
                   WHERE state = 'claimed' AND claim_until_ms <= ?1
                   ORDER BY claim_until_ms, id LIMIT ?2
                 )",
                params![now_ms, i64::from(batch)],
            )
            .map_err(map_sql_error)?;
        let candidates = {
            let mut statement = tx
                .prepare(
                    "SELECT id FROM scheduled_jobs
                     WHERE state = 'scheduled' AND due_at_ms <= ?1
                     ORDER BY due_at_ms, id LIMIT ?2",
                )
                .map_err(map_sql_error)?;
            let rows = statement
                .query_map(params![now_ms, i64::from(batch)], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(map_sql_error)?;
            let mut values = Vec::new();
            for row in rows {
                values.push(row.map_err(map_sql_error)?);
            }
            values
        };
        let mut claimed = Vec::with_capacity(candidates.len());
        for id in candidates {
            let claim_token = random_token()?;
            let affected = tx
                .execute(
                    "UPDATE scheduled_jobs SET state = 'claimed', claim_token = ?1,
                            claim_until_ms = ?2, updated_at_ms = ?3
                     WHERE id = ?4 AND state = 'scheduled' AND due_at_ms <= ?3",
                    params![claim_token, claim_until_ms, now_ms, id],
                )
                .map_err(map_sql_error)?;
            if affected != 1 {
                return Err(corrupt());
            }
            claimed.push(read_claimed(&tx, &id)?);
        }
        tx.commit().map_err(map_sql_error)?;
        Ok((claimed, u64::try_from(recovered).map_err(|_| corrupt())?))
    }

    /// Recover a bounded number of expired claims without claiming new work.
    pub fn recover_expired(&self, now_ms: i64, limit: u32) -> Result<u64, PlatformError> {
        if limit == 0 {
            return Err(invalid());
        }
        let connection = self.lock()?;
        let recovered = connection
            .execute(
                "UPDATE scheduled_jobs
                 SET state = 'scheduled', claim_token = NULL, claim_until_ms = NULL,
                     updated_at_ms = ?1
                 WHERE id IN (
                   SELECT id FROM scheduled_jobs
                   WHERE state = 'claimed' AND claim_until_ms <= ?1
                   ORDER BY claim_until_ms, id LIMIT ?2
                 )",
                params![now_ms, i64::from(limit)],
            )
            .map_err(map_sql_error)?;
        drop(connection);
        if recovered > 0 {
            self.wake.notify();
        }
        u64::try_from(recovered).map_err(|_| corrupt())
    }

    /// Apply one result only while both the lease token and object row token still match.
    pub fn finish_claim(
        &self,
        job: &ClaimedJob,
        result: ClaimResult,
        now_ms: i64,
    ) -> Result<bool, PlatformError> {
        let connection = self.lock()?;
        let affected = match result {
            ClaimResult::Delete => connection.execute(
                "DELETE FROM scheduled_jobs
                 WHERE id = ?1 AND state = 'claimed' AND claim_token = ?2 AND row_token = ?3",
                params![job.id, job.claim_token, job.row_token],
            ),
            ClaimResult::Reschedule {
                due_at_ms,
                retry_count,
                last_error_code,
            } => {
                if due_at_ms <= 0 || retry_count > 6 {
                    return Err(invalid());
                }
                connection.execute(
                    "UPDATE scheduled_jobs SET state = 'scheduled', due_at_ms = ?1,
                            retry_count = ?2, claim_token = NULL, claim_until_ms = NULL,
                            last_error_code = ?3, updated_at_ms = ?4
                     WHERE id = ?5 AND state = 'claimed' AND claim_token = ?6 AND row_token = ?7",
                    params![
                        due_at_ms,
                        i64::from(retry_count),
                        last_error_code,
                        now_ms,
                        job.id,
                        job.claim_token,
                        job.row_token,
                    ],
                )
            }
            ClaimResult::MarkDiscarding { last_error_code } => connection.execute(
                "UPDATE scheduled_jobs SET state = 'discarding', claim_token = NULL,
                        claim_until_ms = NULL, last_error_code = ?1, updated_at_ms = ?2
                 WHERE id = ?3 AND state = 'claimed' AND claim_token = ?4 AND row_token = ?5",
                params![
                    last_error_code,
                    now_ms,
                    job.id,
                    job.claim_token,
                    job.row_token,
                ],
            ),
        }
        .map_err(map_sql_error)?;
        drop(connection);
        if affected > 0 {
            self.wake.notify();
        }
        Ok(affected == 1)
    }

    /// Retarget a live claim to the current version without weakening its token fences.
    pub fn retarget_claim(
        &self,
        job: &ClaimedJob,
        version_id: VersionId,
        execution_generation: u64,
        now_ms: i64,
    ) -> Result<bool, PlatformError> {
        let connection = self.lock()?;
        let affected = connection
            .execute(
                "UPDATE scheduled_jobs SET target_version_id = ?1,
                        execution_generation = ?2, updated_at_ms = ?3
                 WHERE id = ?4 AND state = 'claimed' AND claim_token = ?5 AND row_token = ?6",
                params![
                    version_id.to_string(),
                    i64::try_from(execution_generation).map_err(|_| invalid())?,
                    now_ms,
                    job.id,
                    job.claim_token,
                    job.row_token,
                ],
            )
            .map_err(map_sql_error)?;
        drop(connection);
        if affected > 0 {
            self.wake.notify();
        }
        Ok(affected == 1)
    }

    /// Remove an exhausted projection only after its exact claim entered discarding.
    pub fn finish_discarding(&self, job: &ClaimedJob) -> Result<bool, PlatformError> {
        let connection = self.lock()?;
        let affected = connection
            .execute(
                "DELETE FROM scheduled_jobs
                 WHERE id = ?1 AND state = 'discarding' AND row_token = ?2",
                params![job.id, job.row_token],
            )
            .map_err(map_sql_error)?;
        drop(connection);
        if affected > 0 {
            self.wake.notify();
        }
        Ok(affected == 1)
    }

    /// Read bounded scheduler counts for metrics, health, and doctor.
    pub fn summary(&self, now_ms: i64) -> Result<SchedulerSummary, PlatformError> {
        let connection = self.lock()?;
        summary_connection(&connection, now_ms)
    }

    /// Typed Alarm pool summary for scheduler wake and operator inspection.
    pub fn workload_summary(&self, now_ms: i64) -> Result<WorkloadSummary, PlatformError> {
        let connection = self.lock()?;
        workload_summary_connection(&connection, now_ms)
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>, PlatformError> {
        self.connection.lock().map_err(|_| unavailable())
    }
}

fn read_claimed(connection: &Connection, id: &str) -> Result<ClaimedJob, PlatformError> {
    connection
        .query_row(
            "SELECT id, namespace_resource_id, object_id, object_generation, row_token,
                    due_at_ms, target_version_id, execution_generation, retry_count,
                    claim_token, claim_until_ms
             FROM scheduled_jobs WHERE id = ?1 AND state = 'claimed'",
            [id],
            |row| {
                let namespace: String = row.get(1)?;
                let object: String = row.get(2)?;
                let object_generation: i64 = row.get(3)?;
                let version: String = row.get(6)?;
                let execution_generation: i64 = row.get(7)?;
                let retry_count: i64 = row.get(8)?;
                Ok(ClaimedJob {
                    id: row.get(0)?,
                    namespace_resource_id: ResourceId::from_str(&namespace)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    object_id: DurableObjectId::from_str(&object)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    object_generation: u64::try_from(object_generation)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    row_token: row.get(4)?,
                    due_at_ms: row.get(5)?,
                    target_version_id: VersionId::from_str(&version)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    execution_generation: u64::try_from(execution_generation)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    retry_count: u8::try_from(retry_count)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    claim_token: row.get(9)?,
                    claim_until_ms: row.get(10)?,
                })
            },
        )
        .map_err(map_sql_error)
}

fn validate_projection(projection: &AlarmProjection) -> Result<(), PlatformError> {
    validate_token(&projection.row_token)?;
    if projection.object_generation == 0 || projection.due_at_ms <= 0 || projection.retry_count > 6
    {
        return Err(invalid());
    }
    Ok(())
}

fn validate_token(token: &str) -> Result<(), PlatformError> {
    if !(16..=128).contains(&token.len())
        || token
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
    {
        return Err(invalid());
    }
    Ok(())
}

fn random_token() -> Result<String, PlatformError> {
    let mut bytes = [0_u8; 32];
    rand::rngs::OsRng
        .try_fill_bytes(&mut bytes)
        .map_err(|_| unavailable())?;
    Ok(hex::encode(bytes))
}

fn map_open_error(error: rusqlite::Error) -> PlatformError {
    map_sql_error(error)
}

#[allow(clippy::needless_pass_by_value)]
fn map_sql_error(error: rusqlite::Error) -> PlatformError {
    if let rusqlite::Error::SqliteFailure(code, _) = &error {
        return match code.code {
            rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked => {
                PlatformError::new(ErrorCode::SchedulerBusy, "scheduler database is busy")
            }
            rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase => corrupt(),
            _ => unavailable(),
        };
    }
    unavailable()
}

fn invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::SchedulerInternalProtocolError,
        "scheduler projection input is invalid",
    )
}

fn corrupt() -> PlatformError {
    PlatformError::new(
        ErrorCode::SchedulerCorrupt,
        "scheduler database integrity validation failed",
    )
}

fn unavailable() -> PlatformError {
    PlatformError::new(
        ErrorCode::SchedulerUnavailable,
        "scheduler database operation failed",
    )
}

#[cfg(test)]
#[path = "scheduler_tests.rs"]
mod tests;
