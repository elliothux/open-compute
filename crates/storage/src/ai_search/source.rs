//! Durable R2 source scheduling, catalog reconciliation, and job fencing.

use super::*;

const MANUAL_SYNC_COOLDOWN_MS: i64 = 30_000;

impl AiSearchStore {
    /// Reset the next scheduled scan after an accepted interval update.
    pub fn update_r2_sync_interval(
        &self,
        sync_interval_seconds: u32,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        let next_due = now_ms
            .checked_add(
                i64::from(sync_interval_seconds)
                    .checked_mul(1_000)
                    .ok_or_else(limit_error)?,
            )
            .ok_or_else(limit_error)?;
        let connection = self.lock()?;
        if connection
            .execute(
                "UPDATE source_sync_state SET next_due_at_ms=?1 WHERE singleton=1",
                [next_due],
            )
            .map_err(sql_error)?
            != 1
        {
            return Err(invariant_error());
        }
        Ok(())
    }

    /// Initialize one R2 source schedule and its durable initial sync.
    pub fn initialize_r2_source(
        &self,
        job_id: &str,
        sync_interval_seconds: u32,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        validate_identity(job_id)?;
        let interval_ms = i64::from(sync_interval_seconds)
            .checked_mul(1_000)
            .ok_or_else(limit_error)?;
        let next_due = now_ms.checked_add(interval_ms).ok_or_else(limit_error)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        transaction
            .execute(
                "INSERT INTO source_sync_state(singleton, next_due_at_ms) VALUES(1, ?1)",
                [next_due],
            )
            .map_err(sql_error)?;
        insert_reconcile_job(&transaction, job_id, "initial", now_ms)?;
        transaction.commit().map_err(sql_error)
    }

    /// Whether a complete scan has already observed this source/materialization contract.
    pub fn r2_source_config_observed(
        &self,
        config_sha256: [u8; 32],
    ) -> Result<bool, PlatformError> {
        self.lock()?
            .query_row(
                "SELECT observed_config_sha256=?1 FROM source_sync_state WHERE singleton=1",
                [config_sha256.as_slice()],
                |row| row.get::<_, Option<bool>>(0),
            )
            .optional()
            .map_err(sql_error)
            .map(|value| value.flatten().unwrap_or(false))
    }

    /// Compare and enqueue one explicitly requested R2 item revision.
    pub fn enqueue_r2_item_generation(
        &self,
        job_id: &str,
        candidate: &AiSearchR2Candidate,
        now_ms: i64,
    ) -> Result<bool, PlatformError> {
        validate_identity(job_id)?;
        validate_r2_candidate(candidate)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        require_r2_source(&transaction)?;
        let current: Option<(i64, String, String)> = transaction
            .query_row(
                "SELECT i.desired_generation, i.status, g.r2_object_version
                   FROM items i JOIN item_generations g ON g.item_id=i.id
                    AND g.generation=i.desired_generation
                  WHERE i.id=?1 AND i.source='r2' AND i.key=?2",
                params![candidate.item_id, candidate.key],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(sql_error)?;
        let (config_generation, index_generation): (i64, i64) = transaction
            .query_row(
                "SELECT config_generation, active_index_generation
                   FROM instance_meta WHERE singleton=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(sql_error)?;
        let generation =
            if let Some((desired, status, version)) = current {
                if version == candidate.object_version && status != "error" {
                    transaction
                        .execute(
                            "UPDATE items SET metadata_json=?2, updated_at_ms=?3 WHERE id=?1",
                            params![candidate.item_id, candidate.metadata_json, now_ms],
                        )
                        .map_err(sql_error)?;
                    transaction.commit().map_err(sql_error)?;
                    return Ok(false);
                }
                desired.checked_add(1).ok_or_else(limit_error)?
            } else {
                let item_count: i64 = transaction
                    .query_row("SELECT COUNT(*) FROM items", [], |row| row.get(0))
                    .map_err(sql_error)?;
                if item_count >= MAX_ITEMS_PER_INSTANCE {
                    return Err(quota_error());
                }
                transaction
                .execute(
                    "INSERT INTO items(id, source, key, status, desired_generation, metadata_json,
                       created_at_ms, updated_at_ms) VALUES(?1, 'r2', ?2, 'queued', 1, ?3, ?4, ?4)",
                    params![candidate.item_id, candidate.key, candidate.metadata_json, now_ms],
                )
                .map_err(sql_error)?;
                1
            };
        insert_r2_generation(
            &transaction,
            job_id,
            None,
            "user",
            candidate,
            generation,
            config_generation,
            index_generation,
            now_ms,
        )?;
        transaction.commit().map_err(sql_error)?;
        Ok(true)
    }

    /// Enqueue or coalesce a manual R2 sync, enforcing Cloudflare's 30-second interval.
    pub fn enqueue_manual_r2_reconcile(
        &self,
        job_id: &str,
        now_ms: i64,
    ) -> Result<String, PlatformError> {
        validate_identity(job_id)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        require_r2_source(&transaction)?;
        if let Some(active) = active_reconcile_id(&transaction)? {
            transaction.commit().map_err(sql_error)?;
            return Ok(active);
        }
        let most_recent: Option<i64> = transaction
            .query_row(
                "SELECT MAX(created_at_ms) FROM index_jobs
                  WHERE kind='reconcile' AND source='user'",
                [],
                |row| row.get(0),
            )
            .map_err(sql_error)?;
        if most_recent
            .is_some_and(|created| now_ms.saturating_sub(created) < MANUAL_SYNC_COOLDOWN_MS)
        {
            return Err(PlatformError::new(
                ErrorCode::ResourceUnavailable,
                "AI Search source sync is rate limited",
            ));
        }
        insert_reconcile_job(&transaction, job_id, "user", now_ms)?;
        transaction.commit().map_err(sql_error)?;
        Ok(job_id.to_owned())
    }

    /// Enqueue or coalesce a config-triggered reconcile without applying the
    /// user-trigger cooldown to an already-authorized configuration mutation.
    pub fn enqueue_config_r2_reconcile(
        &self,
        job_id: &str,
        now_ms: i64,
    ) -> Result<String, PlatformError> {
        validate_identity(job_id)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        require_r2_source(&transaction)?;
        if let Some(active) = active_reconcile_id(&transaction)? {
            transaction.commit().map_err(sql_error)?;
            return Ok(active);
        }
        insert_reconcile_job(&transaction, job_id, "user", now_ms)?;
        transaction.commit().map_err(sql_error)?;
        Ok(job_id.to_owned())
    }

    /// Enqueue one missed scheduled sync without creating a catch-up storm.
    pub fn enqueue_due_r2_reconcile(
        &self,
        job_id: &str,
        now_ms: i64,
    ) -> Result<Option<String>, PlatformError> {
        validate_identity(job_id)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        let due: Option<i64> = transaction
            .query_row(
                "SELECT next_due_at_ms FROM source_sync_state WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql_error)?;
        if due.is_none_or(|due| due > now_ms) {
            transaction.commit().map_err(sql_error)?;
            return Ok(None);
        }
        if let Some(active) = active_reconcile_id(&transaction)? {
            transaction.commit().map_err(sql_error)?;
            return Ok(Some(active));
        }
        insert_reconcile_job(&transaction, job_id, "schedule", now_ms)?;
        transaction.commit().map_err(sql_error)?;
        Ok(Some(job_id.to_owned()))
    }

    /// Claim the oldest due source reconcile after recovering expired leases.
    pub fn claim_due_r2_reconcile(
        &self,
        now_ms: i64,
        lease_ms: u64,
        claim_scheduled: bool,
    ) -> Result<Option<AiSearchR2ReconcileClaim>, PlatformError> {
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
                "UPDATE index_jobs SET state='retry_wait', claim_token=NULL,
                   claim_until_ms=NULL, next_attempt_at_ms=?1, updated_at_ms=?1
                 WHERE kind='reconcile' AND state='claimed' AND claim_until_ms<=?1",
                [now_ms],
            )
            .map_err(sql_error)?;
        let candidate: Option<(String, i64)> = transaction
            .query_row(
                "SELECT id, attempt FROM index_jobs
                  WHERE kind='reconcile' AND state IN ('queued','retry_wait')
                    AND next_attempt_at_ms<=?1 AND cancel_requested=0
                    AND (?2 OR source!='schedule')
                  ORDER BY next_attempt_at_ms, created_at_ms, id LIMIT 1",
                params![now_ms, claim_scheduled],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(sql_error)?;
        let Some((job_id, attempt)) = candidate else {
            transaction.commit().map_err(sql_error)?;
            return Ok(None);
        };
        if transaction
            .execute(
                "UPDATE index_jobs SET state='claimed', claim_token=?2,
                   claim_until_ms=?3, attempt=attempt+1,
                   started_at_ms=COALESCE(started_at_ms, ?1), updated_at_ms=?1
                 WHERE id=?4 AND kind='reconcile'
                   AND state IN ('queued','retry_wait') AND cancel_requested=0",
                params![now_ms, token, claim_until_ms, job_id],
            )
            .map_err(sql_error)?
            != 1
        {
            return Err(invariant_error());
        }
        append_job_log(&transaction, &job_id, "claimed", 0, now_ms)?;
        transaction.commit().map_err(sql_error)?;
        Ok(Some(AiSearchR2ReconcileClaim {
            job_id,
            claim_token: token,
            attempt: u32::try_from(attempt + 1).map_err(|_| invariant_error())?,
            claim_until_ms,
        }))
    }

    /// Whether a claimed reconcile has already committed its catalog diff.
    pub fn r2_reconcile_has_children(&self, job_id: &str) -> Result<bool, PlatformError> {
        validate_identity(job_id)?;
        self.lock()?
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM index_jobs WHERE parent_reconcile_id=?1)",
                [job_id],
                |row| row.get(0),
            )
            .map_err(sql_error)
    }

    /// Atomically apply one complete, consistently observed R2 inventory.
    pub fn apply_r2_reconcile(
        &self,
        claim: &AiSearchR2ReconcileClaim,
        candidates: &[AiSearchR2Candidate],
        log_messages: &[String],
        force_generation: bool,
        source_config_sha256: [u8; 32],
        now_ms: i64,
    ) -> Result<bool, PlatformError> {
        if candidates.len() > usize::try_from(MAX_ITEMS_PER_INSTANCE).map_err(|_| limit_error())? {
            return Err(quota_error());
        }
        for candidate in candidates {
            validate_r2_candidate(candidate)?;
        }
        if log_messages.len() > 4
            || log_messages
                .iter()
                .any(|message| message.is_empty() || message.len() > 128)
        {
            return Err(limit_error());
        }
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        if !reconcile_fence(&transaction, claim, now_ms)? {
            transaction.commit().map_err(sql_error)?;
            return Ok(false);
        }
        let keys = candidates
            .iter()
            .map(|item| item.key.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        let existing = {
            let mut statement = transaction
                .prepare("SELECT key FROM items WHERE source='r2' ORDER BY key")
                .map_err(sql_error)?;
            let rows = statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(sql_error)?;
            let mut values = Vec::new();
            for row in rows {
                values.push(row.map_err(sql_error)?);
            }
            values
        };
        for key in existing {
            if !keys.contains(key.as_str()) {
                transaction
                    .execute(
                        "DELETE FROM chunks_fts_porter WHERE chunk_id IN (
                           SELECT c.id FROM chunks c JOIN items i ON i.id=c.item_id
                            WHERE i.source='r2' AND i.key=?1)",
                        [&key],
                    )
                    .map_err(sql_error)?;
                transaction
                    .execute(
                        "DELETE FROM chunks_fts_trigram WHERE chunk_id IN (
                           SELECT c.id FROM chunks c JOIN items i ON i.id=c.item_id
                            WHERE i.source='r2' AND i.key=?1)",
                        [&key],
                    )
                    .map_err(sql_error)?;
                transaction
                    .execute("DELETE FROM items WHERE source='r2' AND key=?1", [&key])
                    .map_err(sql_error)?;
            }
        }
        let config_generation: i64 = transaction
            .query_row(
                "SELECT config_generation FROM instance_meta WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .map_err(sql_error)?;
        let index_generation: i64 = transaction
            .query_row(
                "SELECT active_index_generation FROM instance_meta WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .map_err(sql_error)?;
        for (ordinal, candidate) in candidates.iter().enumerate() {
            let current: Option<(String, i64, Option<i64>, String, String)> = transaction
                .query_row(
                    "SELECT i.id, i.desired_generation, i.active_generation, i.status,
                            g.r2_object_version
                       FROM items i JOIN item_generations g ON g.item_id=i.id
                        AND g.generation=i.desired_generation
                      WHERE i.source='r2' AND i.key=?1",
                    [candidate.key.as_str()],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                        ))
                    },
                )
                .optional()
                .map_err(sql_error)?;
            if let Some((item_id, desired, _active, status, version)) = current {
                if item_id != candidate.item_id {
                    return Err(invariant_error());
                }
                if version == candidate.object_version && status != "error" && !force_generation {
                    transaction
                        .execute(
                            "UPDATE items SET metadata_json=?2, updated_at_ms=?3 WHERE id=?1",
                            params![candidate.item_id, candidate.metadata_json, now_ms],
                        )
                        .map_err(sql_error)?;
                    continue;
                }
                insert_r2_generation(
                    &transaction,
                    &format!("{}-r2-{ordinal}", claim.job_id),
                    Some(&claim.job_id),
                    "schedule",
                    candidate,
                    desired.checked_add(1).ok_or_else(limit_error)?,
                    config_generation,
                    index_generation,
                    now_ms,
                )?;
            } else {
                transaction.execute(
                    "INSERT INTO items(id, source, key, status, desired_generation, metadata_json,
                       created_at_ms, updated_at_ms) VALUES(?1, 'r2', ?2, 'queued', 1, ?3, ?4, ?4)",
                    params![candidate.item_id, candidate.key, candidate.metadata_json, now_ms],
                ).map_err(sql_error)?;
                insert_r2_generation(
                    &transaction,
                    &format!("{}-r2-{ordinal}", claim.job_id),
                    Some(&claim.job_id),
                    "schedule",
                    candidate,
                    1,
                    config_generation,
                    index_generation,
                    now_ms,
                )?;
            }
        }
        transaction
            .execute(
                "UPDATE source_sync_state SET observed_config_sha256=?1 WHERE singleton=1",
                [source_config_sha256.as_slice()],
            )
            .map_err(sql_error)?;
        for message in log_messages {
            append_job_log(&transaction, &claim.job_id, message, 0, now_ms)?;
        }
        append_job_log(&transaction, &claim.job_id, "catalog_reconciled", 0, now_ms)?;
        transaction.commit().map_err(sql_error)?;
        Ok(true)
    }

    /// Finish, fail, or release a reconcile after its child indexing work settles.
    pub fn settle_r2_reconcile(
        &self,
        claim: &AiSearchR2ReconcileClaim,
        sync_interval_seconds: u32,
        retry: bool,
        now_ms: i64,
    ) -> Result<bool, PlatformError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        if !reconcile_fence(&transaction, claim, now_ms)? {
            transaction.commit().map_err(sql_error)?;
            return Ok(false);
        }
        let child_active: bool = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM index_jobs WHERE parent_reconcile_id=?1
               AND state NOT IN ('completed','error','cancelled','outdated'))",
                [&claim.job_id],
                |row| row.get(0),
            )
            .map_err(sql_error)?;
        let child_error: bool = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM index_jobs WHERE parent_reconcile_id=?1
               AND state IN ('error','cancelled'))",
                [&claim.job_id],
                |row| row.get(0),
            )
            .map_err(sql_error)?;
        let retry = retry || child_active;
        let (state, next_attempt, ended) = if retry {
            ("retry_wait", now_ms.saturating_add(1_000), None)
        } else if child_error {
            ("error", now_ms, Some(now_ms))
        } else {
            ("completed", now_ms, Some(now_ms))
        };
        if transaction
            .execute(
                "UPDATE index_jobs SET state=?3, claim_token=NULL, claim_until_ms=NULL,
               next_attempt_at_ms=?4, ended_at_ms=?5, updated_at_ms=?2
             WHERE id=?1 AND kind='reconcile' AND state='claimed' AND claim_token=?6",
                params![
                    claim.job_id,
                    now_ms,
                    state,
                    next_attempt,
                    ended,
                    claim.claim_token
                ],
            )
            .map_err(sql_error)?
            != 1
        {
            return Ok(false);
        }
        if state == "completed" {
            let interval = i64::from(sync_interval_seconds)
                .checked_mul(1_000)
                .ok_or_else(limit_error)?;
            let current: i64 = transaction
                .query_row(
                    "SELECT next_due_at_ms FROM source_sync_state WHERE singleton=1",
                    [],
                    |row| row.get(0),
                )
                .map_err(sql_error)?;
            let mut next = current;
            while next <= now_ms {
                next = next.checked_add(interval).ok_or_else(limit_error)?;
            }
            transaction
                .execute(
                    "UPDATE source_sync_state SET next_due_at_ms=?1 WHERE singleton=1",
                    [next],
                )
                .map_err(sql_error)?;
        }
        append_job_log(
            &transaction,
            &claim.job_id,
            state,
            i64::from(state == "error"),
            now_ms,
        )?;
        prune_terminal_jobs(&transaction)?;
        transaction.commit().map_err(sql_error)?;
        Ok(true)
    }
}

fn insert_reconcile_job(
    transaction: &rusqlite::Transaction<'_>,
    job_id: &str,
    source: &str,
    now_ms: i64,
) -> Result<(), PlatformError> {
    transaction
        .execute(
            "INSERT INTO index_jobs(id, kind, source, state, config_generation, index_generation,
           attempt, next_attempt_at_ms, cancel_requested, created_at_ms, updated_at_ms)
         SELECT ?1, 'reconcile', ?2, 'queued', config_generation, active_index_generation,
           0, ?3, 0, ?3, ?3 FROM instance_meta WHERE singleton=1",
            params![job_id, source, now_ms],
        )
        .map_err(sql_error)?;
    append_job_log(transaction, job_id, "queued", 0, now_ms)
}

#[allow(
    clippy::too_many_arguments,
    reason = "generation/job fields are one atomic SQLite persistence boundary"
)]
fn insert_r2_generation(
    transaction: &rusqlite::Transaction<'_>,
    job_id: &str,
    parent_reconcile_id: Option<&str>,
    job_source: &str,
    candidate: &AiSearchR2Candidate,
    generation: i64,
    config_generation: i64,
    index_generation: i64,
    now_ms: i64,
) -> Result<(), PlatformError> {
    validate_identity(job_id)?;
    transaction
        .execute(
            "INSERT INTO item_generations(item_id, generation, index_generation, state,
           object_key, object_sha256, r2_object_version, r2_etag, r2_uploaded_at_ms,
           object_size, content_type, created_at_ms)
         VALUES(?1, ?2, ?3, 'queued', NULL, NULL, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                candidate.item_id,
                generation,
                index_generation,
                candidate.object_version,
                candidate.etag,
                candidate.uploaded_at_ms,
                to_i64(candidate.object_size)?,
                candidate.content_type,
                now_ms
            ],
        )
        .map_err(sql_error)?;
    transaction
        .execute(
            "UPDATE items SET desired_generation=?2, status='queued', metadata_json=?3,
           updated_at_ms=?4 WHERE id=?1",
            params![
                candidate.item_id,
                generation,
                candidate.metadata_json,
                now_ms
            ],
        )
        .map_err(sql_error)?;
    transaction
        .execute(
            "INSERT INTO index_jobs(id, kind, source, parent_reconcile_id, description, state,
           config_generation, index_generation, attempt, next_attempt_at_ms, cancel_requested,
           created_at_ms, updated_at_ms)
         VALUES(?1, 'index', ?2, ?3, 'R2 source item', 'queued', ?4, ?5,
           0, ?6, 0, ?6, ?6)",
            params![
                job_id,
                job_source,
                parent_reconcile_id,
                config_generation,
                index_generation,
                now_ms
            ],
        )
        .map_err(sql_error)?;
    transaction
        .execute(
            "INSERT INTO index_job_items(job_id, item_id, item_generation, index_generation,
           state, next_batch_ordinal, updated_at_ms)
         VALUES(?1, ?2, ?3, ?4, 'queued', 0, ?5)",
            params![
                job_id,
                candidate.item_id,
                generation,
                index_generation,
                now_ms
            ],
        )
        .map_err(sql_error)?;
    append_item_log(transaction, &candidate.item_id, "queued", now_ms)?;
    append_job_log(transaction, job_id, "queued", 0, now_ms)
}

fn active_reconcile_id(
    transaction: &rusqlite::Transaction<'_>,
) -> Result<Option<String>, PlatformError> {
    transaction
        .query_row(
            "SELECT id FROM index_jobs WHERE kind='reconcile'
          AND state IN ('queued','claimed','retry_wait','cancelling') LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(sql_error)
}

fn require_r2_source(transaction: &rusqlite::Transaction<'_>) -> Result<(), PlatformError> {
    let exists: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM source_sync_state WHERE singleton=1)",
            [],
            |row| row.get(0),
        )
        .map_err(sql_error)?;
    if exists {
        Ok(())
    } else {
        Err(invariant_error())
    }
}

fn reconcile_fence(
    transaction: &rusqlite::Transaction<'_>,
    claim: &AiSearchR2ReconcileClaim,
    now_ms: i64,
) -> Result<bool, PlatformError> {
    transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM index_jobs WHERE id=?1 AND kind='reconcile'
           AND state='claimed' AND claim_token=?2 AND claim_until_ms>?3
           AND cancel_requested=0)",
            params![claim.job_id, claim.claim_token, now_ms],
            |row| row.get(0),
        )
        .map_err(sql_error)
}

fn validate_r2_candidate(candidate: &AiSearchR2Candidate) -> Result<(), PlatformError> {
    validate_identity(&candidate.item_id)?;
    if candidate.key.is_empty()
        || candidate.key.len() > 1_024
        || candidate.key.contains('\0')
        || candidate.object_version.is_empty()
        || candidate.object_version.len() > 128
        || candidate.etag.is_empty()
        || candidate.etag.len() > 512
        || candidate.object_size == 0
        || candidate.content_type.is_empty()
        || candidate.content_type.len() > 128
        || !canonical_json_object(&candidate.metadata_json, 65_536)
    {
        return Err(limit_error());
    }
    Ok(())
}
