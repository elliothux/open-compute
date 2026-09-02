//! Durable source upload intents and exact object garbage collection.

use super::{
    AiSearchObjectGcClaim, AiSearchStore, NewAiSearchItemGeneration, append_item_log,
    append_job_log, enforce_enqueue_quotas, invariant_error, limit_error, prune_generation_history,
    prune_terminal_jobs, sql_error, to_i64, to_u64, validate_identity, validate_item,
};
use open_compute_core::{ErrorCode, PlatformError};
use rand::TryRngCore as _;
use rusqlite::{OptionalExtension as _, TransactionBehavior, params};

impl AiSearchStore {
    /// Reserve an upload intent before any object-store I/O occurs.
    pub fn reserve_ingest_intent(
        &self,
        intent_id: &str,
        item_id: &str,
        object_key: &str,
        object_sha256: [u8; 32],
        object_size: u64,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        validate_identity(intent_id)?;
        validate_identity(item_id)?;
        validate_object(object_key, object_size)?;
        self.lock()?
            .execute(
                "INSERT INTO ingest_intents
                 (id, item_id, object_key, object_sha256, object_size,
                  state, created_at_ms, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'uploading', ?6, ?6)",
                params![
                    intent_id,
                    item_id,
                    object_key,
                    object_sha256,
                    to_i64(object_size)?,
                    now_ms
                ],
            )
            .map_err(sql_error)?;
        Ok(())
    }

    /// Record the exact immutable object after a successful create-only upload.
    pub fn mark_ingest_uploaded(
        &self,
        intent_id: &str,
        object_key: &str,
        object_sha256: [u8; 32],
        object_size: u64,
        now_ms: i64,
    ) -> Result<bool, PlatformError> {
        validate_identity(intent_id)?;
        validate_object(object_key, object_size)?;
        let updated = self
            .lock()?
            .execute(
                "UPDATE ingest_intents SET state='uploaded', updated_at_ms=?5
                 WHERE id=?1 AND state='uploading' AND object_key=?2
                   AND object_sha256=?3 AND object_size=?4",
                params![
                    intent_id,
                    object_key,
                    object_sha256,
                    to_i64(object_size)?,
                    now_ms
                ],
            )
            .map_err(sql_error)?;
        Ok(updated == 1)
    }

    /// Verify an uploaded intent and atomically commit its catalog generation and job.
    pub fn commit_uploaded_ingest(
        &self,
        intent_id: &str,
        job_id: &str,
        item: &NewAiSearchItemGeneration<'_>,
    ) -> Result<(), PlatformError> {
        validate_identity(intent_id)?;
        validate_identity(job_id)?;
        validate_item(item)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        let intent: Option<(String, String, Vec<u8>, i64)> = transaction
            .query_row(
                "SELECT item_id, object_key, object_sha256, object_size
                   FROM ingest_intents WHERE id=?1 AND state='uploaded'",
                [intent_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .map_err(sql_error)?;
        let Some((intent_item, object_key, object_sha256, object_size)) = intent else {
            return Err(invariant_error());
        };
        if intent_item != item.item_id
            || object_key != item.object_key
            || object_sha256.as_slice() != item.object_sha256
            || to_u64(object_size)? != item.object_size
        {
            return Err(invariant_error());
        }
        enforce_enqueue_quotas(&transaction, item)?;
        prune_terminal_jobs(&transaction)?;
        let config_generation: i64 = transaction
            .query_row(
                "SELECT config_generation FROM instance_meta WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .map_err(sql_error)?;
        transaction
            .execute(
                "INSERT INTO index_jobs
                 (id, source, state, config_generation, index_generation, attempt,
                  next_attempt_at_ms, cancel_requested, created_at_ms, updated_at_ms)
                 VALUES (?1, 'user', 'queued', ?2, ?3, 0, ?4, 0, ?4, ?4)",
                params![
                    job_id,
                    config_generation,
                    to_i64(item.index_generation)?,
                    item.now_ms
                ],
            )
            .map_err(sql_error)?;
        transaction
            .execute(
                "INSERT INTO items
                 (id, source, key, status, desired_generation, metadata_json,
                  created_at_ms, updated_at_ms)
                 VALUES (?1, ?2, ?3, 'queued', ?4, ?5, ?6, ?6)
                 ON CONFLICT(source, key) DO UPDATE SET
                   status='queued', desired_generation=excluded.desired_generation,
                   metadata_json=excluded.metadata_json, updated_at_ms=excluded.updated_at_ms",
                params![
                    item.item_id,
                    item.source,
                    item.key,
                    to_i64(item.generation)?,
                    item.metadata_json,
                    item.now_ms
                ],
            )
            .map_err(sql_error)?;
        let stored_id: String = transaction
            .query_row(
                "SELECT id FROM items WHERE source=?1 AND key=?2",
                params![item.source, item.key],
                |row| row.get(0),
            )
            .map_err(sql_error)?;
        if stored_id != item.item_id {
            return Err(invariant_error());
        }
        transaction
            .execute(
                "INSERT INTO item_generations
                 (item_id, generation, index_generation, state, object_key,
                  object_sha256, object_size, content_type, created_at_ms)
                 VALUES (?1, ?2, ?3, 'queued', ?4, ?5, ?6, ?7, ?8)",
                params![
                    item.item_id,
                    to_i64(item.generation)?,
                    to_i64(item.index_generation)?,
                    item.object_key,
                    item.object_sha256,
                    to_i64(item.object_size)?,
                    item.content_type,
                    item.now_ms
                ],
            )
            .map_err(sql_error)?;
        transaction
            .execute(
                "INSERT INTO index_job_items
                 (job_id, item_id, item_generation, index_generation, state,
                  next_batch_ordinal, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4, 'queued', 0, ?5)",
                params![
                    job_id,
                    item.item_id,
                    to_i64(item.generation)?,
                    to_i64(item.index_generation)?,
                    item.now_ms
                ],
            )
            .map_err(sql_error)?;
        let updated = transaction
            .execute(
                "UPDATE ingest_intents SET state='committed', updated_at_ms=?2
                 WHERE id=?1 AND state='uploaded'",
                params![intent_id, item.now_ms],
            )
            .map_err(sql_error)?;
        if updated != 1 {
            return Err(invariant_error());
        }
        append_item_log(&transaction, item.item_id, "queued", item.now_ms)?;
        append_job_log(&transaction, job_id, "queued", 0, item.now_ms)?;
        queue_unreferenced_objects(&transaction, item.now_ms)?;
        prune_ingest_intents(&transaction)?;
        prune_generation_history(&transaction)?;
        transaction.commit().map_err(sql_error)
    }

    /// Convert stale crash residues into abandoned evidence and exact GC work.
    pub fn reconcile_abandoned_ingests(
        &self,
        stale_before_ms: i64,
        now_ms: i64,
    ) -> Result<u64, PlatformError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        transaction
            .execute(
                "INSERT OR IGNORE INTO object_gc
                 (object_key, object_sha256, object_size, state, attempt,
                  next_attempt_at_ms, created_at_ms, updated_at_ms)
                 SELECT object_key, object_sha256, object_size, 'queued', 0, ?2, ?2, ?2
                   FROM ingest_intents ii
                  WHERE ii.state IN ('uploading','uploaded') AND ii.updated_at_ms<=?1
                    AND NOT EXISTS (
                      SELECT 1 FROM items i JOIN item_generations g ON g.item_id=i.id
                       WHERE g.object_key=ii.object_key AND g.object_sha256=ii.object_sha256
                         AND g.object_size=ii.object_size
                         AND g.generation IN (i.active_generation, i.desired_generation))",
                params![stale_before_ms, now_ms],
            )
            .map_err(sql_error)?;
        let abandoned = transaction
            .execute(
                "UPDATE ingest_intents SET state='abandoned', updated_at_ms=?2
                 WHERE state IN ('uploading','uploaded') AND updated_at_ms<=?1",
                params![stale_before_ms, now_ms],
            )
            .map_err(sql_error)?;
        prune_ingest_intents(&transaction)?;
        transaction.commit().map_err(sql_error)?;
        u64::try_from(abandoned).map_err(|_| limit_error())
    }

    /// Delete one item catalog row and atomically queue every now-unreferenced object.
    pub fn delete_item_and_enqueue_gc(
        &self,
        item_id: &str,
        now_ms: i64,
    ) -> Result<bool, PlatformError> {
        validate_identity(item_id)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        transaction
            .execute(
                "UPDATE index_jobs SET state='outdated', claim_token=NULL,
                   claim_until_ms=NULL, ended_at_ms=?2, updated_at_ms=?2
                 WHERE id IN (SELECT job_id FROM index_job_items WHERE item_id=?1)
                   AND state NOT IN ('completed','error','cancelled','outdated')",
                params![item_id, now_ms],
            )
            .map_err(sql_error)?;
        transaction
            .execute(
                "INSERT OR IGNORE INTO object_gc
                 (object_key, object_sha256, object_size, state, attempt,
                  next_attempt_at_ms, created_at_ms, updated_at_ms)
                 SELECT object_key, object_sha256, object_size, 'queued', 0, ?2, ?2, ?2
                   FROM ingest_intents WHERE item_id=?1 AND state IN ('uploading','uploaded')",
                params![item_id, now_ms],
            )
            .map_err(sql_error)?;
        transaction
            .execute(
                "UPDATE ingest_intents SET state='abandoned', updated_at_ms=?2
                 WHERE item_id=?1 AND state IN ('uploading','uploaded')",
                params![item_id, now_ms],
            )
            .map_err(sql_error)?;
        transaction
            .execute(
                "INSERT OR IGNORE INTO object_gc
                 (object_key, object_sha256, object_size, state, attempt,
                  next_attempt_at_ms, created_at_ms, updated_at_ms)
                 SELECT DISTINCT g.object_key, g.object_sha256, g.object_size,
                   'queued', 0, ?2, ?2, ?2
                 FROM item_generations g WHERE g.item_id=?1
                   AND NOT EXISTS (
                     SELECT 1 FROM items i2 JOIN item_generations g2 ON g2.item_id=i2.id
                      WHERE i2.id!=?1 AND g2.object_key=g.object_key
                        AND g2.object_sha256=g.object_sha256 AND g2.object_size=g.object_size
                        AND g2.generation IN (i2.active_generation, i2.desired_generation))",
                params![item_id, now_ms],
            )
            .map_err(sql_error)?;
        transaction
            .execute(
                "DELETE FROM chunks_fts_porter
                 WHERE chunk_id IN (SELECT id FROM chunks WHERE item_id=?1)",
                [item_id],
            )
            .map_err(sql_error)?;
        transaction
            .execute(
                "DELETE FROM chunks_fts_trigram
                 WHERE chunk_id IN (SELECT id FROM chunks WHERE item_id=?1)",
                [item_id],
            )
            .map_err(sql_error)?;
        let deleted = transaction
            .execute("DELETE FROM items WHERE id=?1", [item_id])
            .map_err(sql_error)?;
        transaction.commit().map_err(sql_error)?;
        Ok(deleted == 1)
    }

    /// Fence all instance work, remove the catalog, and retain exact GC authority.
    /// The database must remain owned until `pending_object_gc_count` reaches zero.
    pub fn prepare_instance_delete_and_enqueue_gc(
        &self,
        now_ms: i64,
    ) -> Result<u64, PlatformError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        transaction
            .execute(
                "UPDATE index_jobs SET state='outdated', claim_token=NULL,
                   claim_until_ms=NULL, ended_at_ms=?1, updated_at_ms=?1
                 WHERE state NOT IN ('completed','error','cancelled','outdated')",
                [now_ms],
            )
            .map_err(sql_error)?;
        transaction
            .execute(
                "INSERT OR IGNORE INTO object_gc
                 (object_key, object_sha256, object_size, state, attempt,
                  next_attempt_at_ms, created_at_ms, updated_at_ms)
                 SELECT object_key, object_sha256, object_size, 'queued', 0, ?1, ?1, ?1
                   FROM ingest_intents WHERE state IN ('uploading','uploaded')",
                [now_ms],
            )
            .map_err(sql_error)?;
        transaction
            .execute(
                "UPDATE ingest_intents SET state='abandoned', updated_at_ms=?1
                 WHERE state IN ('uploading','uploaded')",
                [now_ms],
            )
            .map_err(sql_error)?;
        transaction
            .execute(
                "INSERT OR IGNORE INTO object_gc
                 (object_key, object_sha256, object_size, state, attempt,
                  next_attempt_at_ms, created_at_ms, updated_at_ms)
                 SELECT DISTINCT object_key, object_sha256, object_size,
                   'queued', 0, ?1, ?1, ?1 FROM item_generations",
                [now_ms],
            )
            .map_err(sql_error)?;
        transaction
            .execute("DELETE FROM chunks_fts_porter", [])
            .map_err(sql_error)?;
        transaction
            .execute("DELETE FROM chunks_fts_trigram", [])
            .map_err(sql_error)?;
        let deleted = transaction
            .execute("DELETE FROM items", [])
            .map_err(sql_error)?;
        transaction.commit().map_err(sql_error)?;
        u64::try_from(deleted).map_err(|_| limit_error())
    }

    /// Return the exact number of object deletions that still own remote cleanup work.
    pub fn pending_object_gc_count(&self) -> Result<u64, PlatformError> {
        let count: i64 = self
            .lock()?
            .query_row("SELECT COUNT(*) FROM object_gc", [], |row| row.get(0))
            .map_err(sql_error)?;
        to_u64(count)
    }

    /// Claim the oldest due exact object deletion after recovering expired leases.
    pub fn claim_due_object_gc(
        &self,
        now_ms: i64,
        lease_ms: u64,
    ) -> Result<Option<AiSearchObjectGcClaim>, PlatformError> {
        if lease_ms == 0 {
            return Err(limit_error());
        }
        let claim_until_ms = now_ms
            .checked_add(to_i64(lease_ms)?)
            .ok_or_else(limit_error)?;
        let mut token = [0_u8; 32];
        rand::rng().try_fill_bytes(&mut token).map_err(|_| {
            PlatformError::new(
                ErrorCode::PlatformUnavailable,
                "secure random generation failed",
            )
        })?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        transaction
            .execute(
                "UPDATE object_gc SET state='retry_wait', claim_token=NULL,
                   claim_until_ms=NULL, next_attempt_at_ms=?1, updated_at_ms=?1
                 WHERE state='claimed' AND claim_until_ms<=?1",
                [now_ms],
            )
            .map_err(sql_error)?;
        transaction
            .execute(
                "DELETE FROM object_gc WHERE EXISTS (
                   SELECT 1 FROM items i JOIN item_generations g ON g.item_id=i.id
                    WHERE g.object_key=object_gc.object_key
                      AND g.object_sha256=object_gc.object_sha256
                      AND g.object_size=object_gc.object_size
                      AND g.generation IN (i.active_generation, i.desired_generation))",
                [],
            )
            .map_err(sql_error)?;
        let candidate: Option<(String, Vec<u8>, i64, i64)> = transaction
            .query_row(
                "SELECT object_key, object_sha256, object_size, attempt FROM object_gc
                  WHERE state IN ('queued','retry_wait') AND next_attempt_at_ms<=?1
                  ORDER BY next_attempt_at_ms, created_at_ms, object_key LIMIT 1",
                [now_ms],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .map_err(sql_error)?;
        let Some((object_key, digest, size, attempt)) = candidate else {
            transaction.commit().map_err(sql_error)?;
            return Ok(None);
        };
        let updated = transaction
            .execute(
                "UPDATE object_gc SET state='claimed', claim_token=?2,
                   claim_until_ms=?3, attempt=attempt+1, updated_at_ms=?1
                 WHERE object_key=?4 AND state IN ('queued','retry_wait')",
                params![now_ms, token, claim_until_ms, object_key],
            )
            .map_err(sql_error)?;
        if updated != 1 {
            return Err(invariant_error());
        }
        transaction.commit().map_err(sql_error)?;
        Ok(Some(AiSearchObjectGcClaim {
            object_key,
            object_sha256: digest.try_into().map_err(|_| invariant_error())?,
            object_size: to_u64(size)?,
            claim_token: token,
            attempt: u32::try_from(attempt + 1).map_err(|_| invariant_error())?,
            claim_until_ms,
        }))
    }

    /// Extend one exact object deletion lease if its fence remains current.
    pub fn renew_object_gc_claim(
        &self,
        claim: &AiSearchObjectGcClaim,
        now_ms: i64,
        lease_ms: u64,
    ) -> Result<bool, PlatformError> {
        let claim_until_ms = now_ms
            .checked_add(to_i64(lease_ms)?)
            .ok_or_else(limit_error)?;
        let updated = self
            .lock()?
            .execute(
                "UPDATE object_gc SET claim_until_ms=?3, updated_at_ms=?2
                 WHERE object_key=?1 AND state='claimed' AND claim_token=?4
                   AND claim_until_ms>?2",
                params![claim.object_key, now_ms, claim_until_ms, claim.claim_token],
            )
            .map_err(sql_error)?;
        Ok(updated == 1)
    }

    /// Release one failed exact object deletion for a durable bounded-backoff retry.
    pub fn retry_object_gc_claim(
        &self,
        claim: &AiSearchObjectGcClaim,
        next_attempt_at_ms: i64,
        now_ms: i64,
    ) -> Result<bool, PlatformError> {
        let updated = self
            .lock()?
            .execute(
                "UPDATE object_gc SET state='retry_wait', claim_token=NULL,
                   claim_until_ms=NULL, next_attempt_at_ms=?3, updated_at_ms=?4
                 WHERE object_key=?1 AND state='claimed' AND claim_token=?2",
                params![
                    claim.object_key,
                    claim.claim_token,
                    next_attempt_at_ms,
                    now_ms
                ],
            )
            .map_err(sql_error)?;
        Ok(updated == 1)
    }

    /// Remove one GC authority row only after exact remote deletion succeeded.
    pub fn complete_object_gc_claim(
        &self,
        claim: &AiSearchObjectGcClaim,
    ) -> Result<bool, PlatformError> {
        let deleted = self
            .lock()?
            .execute(
                "DELETE FROM object_gc
                 WHERE object_key=?1 AND state='claimed' AND claim_token=?2",
                params![claim.object_key, claim.claim_token],
            )
            .map_err(sql_error)?;
        Ok(deleted == 1)
    }
}

pub(super) fn queue_unreferenced_objects(
    transaction: &rusqlite::Transaction<'_>,
    now_ms: i64,
) -> Result<(), PlatformError> {
    transaction
        .execute(
            "INSERT OR IGNORE INTO object_gc
             (object_key, object_sha256, object_size, state, attempt,
              next_attempt_at_ms, created_at_ms, updated_at_ms)
             SELECT DISTINCT g.object_key, g.object_sha256, g.object_size,
               'queued', 0, ?1, ?1, ?1 FROM item_generations g
              WHERE NOT EXISTS (
                SELECT 1 FROM items i JOIN item_generations active ON active.item_id=i.id
                 WHERE active.object_key=g.object_key AND active.object_sha256=g.object_sha256
                   AND active.object_size=g.object_size
                   AND active.generation IN (i.active_generation, i.desired_generation))",
            [now_ms],
        )
        .map_err(sql_error)?;
    Ok(())
}

fn prune_ingest_intents(transaction: &rusqlite::Transaction<'_>) -> Result<(), PlatformError> {
    transaction
        .execute(
            "DELETE FROM ingest_intents WHERE id IN (
               SELECT id FROM ingest_intents
                WHERE state IN ('committed','abandoned')
                ORDER BY updated_at_ms DESC, id DESC LIMIT -1 OFFSET 1000)",
            [],
        )
        .map_err(sql_error)?;
    Ok(())
}

fn validate_object(object_key: &str, object_size: u64) -> Result<(), PlatformError> {
    if object_key.is_empty()
        || object_key.len() > 1_024
        || object_key.starts_with('/')
        || object_key.bytes().any(|byte| byte.is_ascii_control())
        || object_key
            .split('/')
            .any(|segment| segment.is_empty() || matches!(segment, "." | ".."))
        || object_size == 0
        || object_size > 4_194_304
    {
        return Err(limit_error());
    }
    Ok(())
}
