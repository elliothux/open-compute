//! Independent Queue catalog and immutable producer-binding authority.

use crate::catalog_page::{CatalogColumns, build_catalog_sql, record_catalog_cursor};
use crate::{
    CatalogCursor, CatalogDirection, CatalogListPage, CatalogSort, ControlDb,
    IdempotencyReservation, VersionState,
};
use open_compute_core::{
    AccountId, BindingId, ErrorCode, PlatformError, QueueId, RequestId, VersionId,
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

mod repository;

/// Insert Queue producer bindings inside the version staging transaction.
pub(crate) fn insert_staging_bindings(
    tx: &Transaction<'_>,
    version_id: VersionId,
    bindings: &[NewQueueProducerBinding],
    now_ms: i64,
) -> Result<(), PlatformError> {
    for binding in bindings {
        tx.execute(
            "INSERT INTO queue_producer_bindings
             (id, version_id, name, queue_id, queue_lifecycle_generation,
              capability_version, descriptor_sha256, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                binding.id.to_string(),
                version_id.to_string(),
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
          lifecycle_generation, config_generation, delivery_paused, delivery_delay_seconds,
          retention_seconds, max_message_bytes, max_batch_messages, max_batch_bytes,
          max_backlog_bytes, created_at_ms, updated_at_ms, deleted_at_ms)
         VALUES (?1, ?2, ?3, 'creating', 'degraded', 'QUEUE_PROJECTION_PENDING',
                 1, 1, 0, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10, NULL)",
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

pub(crate) fn read_version_bindings_conn(
    conn: &rusqlite::Connection,
    version_id: VersionId,
) -> Result<Vec<QueueProducerBindingRecord>, PlatformError> {
    let mut statement = conn
        .prepare(
            "SELECT b.id, b.version_id, b.name, b.queue_id,
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
             JOIN worker_versions d ON d.id = b.version_id
             JOIN workers w ON w.id = d.worker_id
             WHERE b.version_id = ?1 ORDER BY b.name, b.id",
        )
        .map_err(|_| db_error())?;
    let rows = statement
        .query_map([version_id.to_string()], |row| {
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
                lifecycle_generation, config_generation, delivery_paused, delivery_delay_seconds,
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
                lifecycle_generation, config_generation, delivery_paused, delivery_delay_seconds,
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
    let delay: i64 = row.get(offset + 9)?;
    let retention: i64 = row.get(offset + 10)?;
    let message_bytes: i64 = row.get(offset + 11)?;
    let batch_messages: i64 = row.get(offset + 12)?;
    let batch_bytes: i64 = row.get(offset + 13)?;
    let backlog_bytes: i64 = row.get(offset + 14)?;
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
        delivery_paused: row.get(offset + 8)?,
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
        created_at_ms: row.get(offset + 15)?,
        updated_at_ms: row.get(offset + 16)?,
        deleted_at_ms: row.get(offset + 17)?,
    })
}

fn map_binding_offset(
    row: &rusqlite::Row<'_>,
    offset: usize,
) -> rusqlite::Result<QueueProducerBindingRecord> {
    let id: String = row.get(offset)?;
    let version: String = row.get(offset + 1)?;
    let queue: String = row.get(offset + 3)?;
    let generation: i64 = row.get(offset + 4)?;
    let capability: i64 = row.get(offset + 5)?;
    let digest: Vec<u8> = row.get(offset + 6)?;
    Ok(QueueProducerBindingRecord {
        id: BindingId::from_str(&id).map_err(|_| rusqlite::Error::InvalidQuery)?,
        version_id: VersionId::from_str(&version).map_err(|_| rusqlite::Error::InvalidQuery)?,
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
