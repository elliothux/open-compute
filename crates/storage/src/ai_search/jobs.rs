//! Durable indexing job claims, fencing, activation, and recovery.

use super::*;

impl AiSearchStore {
    /// Claim the oldest due job after first recovering expired claims.
    pub fn claim_due_job(
        &self,
        now_ms: i64,
        lease_ms: u64,
    ) -> Result<Option<AiSearchJobClaim>, PlatformError> {
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
                "UPDATE index_jobs SET state='cancelled', claim_token=NULL,
                   claim_until_ms=NULL, ended_at_ms=?1, updated_at_ms=?1
                 WHERE state='cancelling' AND claim_until_ms<=?1",
                [now_ms],
            )
            .map_err(sql_error)?;
        transaction
            .execute(
                "UPDATE index_jobs SET state='retry_wait', claim_token=NULL,
                   claim_until_ms=NULL, next_attempt_at_ms=?1, updated_at_ms=?1
                 WHERE state='claimed' AND claim_until_ms<=?1",
                [now_ms],
            )
            .map_err(sql_error)?;
        transaction
            .execute(
                "UPDATE index_job_items SET state='queued', updated_at_ms=?1
                 WHERE state='claimed' AND job_id IN
                   (SELECT id FROM index_jobs WHERE state='retry_wait')",
                [now_ms],
            )
            .map_err(sql_error)?;
        transaction
            .execute(
                "UPDATE item_generations SET state='queued'
                 WHERE state='claimed' AND (item_id, generation) IN
                   (SELECT ji.item_id, ji.item_generation FROM index_job_items ji
                    JOIN index_jobs j ON j.id=ji.job_id WHERE j.state='retry_wait')",
                [],
            )
            .map_err(sql_error)?;
        transaction
            .execute(
                "UPDATE items SET status='queued', updated_at_ms=?1
                 WHERE status='running' AND id IN
                   (SELECT ji.item_id FROM index_job_items ji
                    JOIN index_jobs j ON j.id=ji.job_id WHERE j.state='retry_wait')",
                [now_ms],
            )
            .map_err(sql_error)?;
        let candidate: Option<(String, i64, i64, i64)> = transaction
            .query_row(
                "SELECT id, attempt, config_generation, index_generation
                 FROM index_jobs
                 WHERE state IN ('queued','retry_wait') AND next_attempt_at_ms<=?1
                   AND cancel_requested=0
                 ORDER BY next_attempt_at_ms, created_at_ms, id LIMIT 1",
                [now_ms],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .map_err(sql_error)?;
        let Some((job_id, attempt, config_generation, index_generation)) = candidate else {
            transaction.commit().map_err(sql_error)?;
            return Ok(None);
        };
        let updated = transaction
            .execute(
                "UPDATE index_jobs SET state='claimed', claim_token=?2,
                   claim_until_ms=?3, attempt=attempt+1, started_at_ms=COALESCE(started_at_ms, ?1),
                   updated_at_ms=?1
                 WHERE id=?4 AND state IN ('queued','retry_wait') AND cancel_requested=0",
                params![now_ms, token, claim_until_ms, job_id],
            )
            .map_err(sql_error)?;
        if updated != 1 {
            return Err(invariant_error());
        }
        transaction
            .execute(
                "UPDATE index_job_items SET state='claimed', updated_at_ms=?2
                 WHERE job_id=?1 AND state='queued'",
                params![job_id, now_ms],
            )
            .map_err(sql_error)?;
        transaction
            .execute(
                "UPDATE item_generations SET state='claimed'
                 WHERE (item_id, generation) IN
                   (SELECT item_id, item_generation FROM index_job_items WHERE job_id=?1)
                   AND state='queued'",
                [job_id.as_str()],
            )
            .map_err(sql_error)?;
        transaction
            .execute(
                "UPDATE items SET status='running', updated_at_ms=?2
                 WHERE id IN (SELECT item_id FROM index_job_items WHERE job_id=?1)
                   AND status='queued'",
                params![job_id, now_ms],
            )
            .map_err(sql_error)?;
        let item = transaction
            .query_row(
                "SELECT i.id, i.key, ji.item_generation, g.object_key, g.object_sha256,
                   g.object_size, g.content_type, i.metadata_json, ji.next_batch_ordinal
                 FROM index_job_items ji JOIN items i ON i.id=ji.item_id
                 JOIN item_generations g
                   ON g.item_id=ji.item_id AND g.generation=ji.item_generation
                 WHERE ji.job_id=?1 ORDER BY i.id LIMIT 1",
                [job_id.as_str()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Vec<u8>>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, Vec<u8>>(7)?,
                        row.get::<_, i64>(8)?,
                    ))
                },
            )
            .map_err(sql_error)?;
        let claimed_item = ClaimedAiSearchItem {
            item_id: item.0,
            key: item.1,
            generation: to_u64(item.2)?,
            object_key: item.3,
            object_sha256: item.4.try_into().map_err(|_| invariant_error())?,
            object_size: to_u64(item.5)?,
            content_type: item.6,
            metadata_json: item.7,
        };
        append_item_log(&transaction, &claimed_item.item_id, "running", now_ms)?;
        append_job_log(&transaction, &job_id, "claimed", 0, now_ms)?;
        transaction.commit().map_err(sql_error)?;
        Ok(Some(AiSearchJobClaim {
            job_id,
            claim_token: token,
            attempt: u32::try_from(attempt + 1).map_err(|_| invariant_error())?,
            claim_until_ms,
            config_generation: to_u64(config_generation)?,
            index_generation: to_u64(index_generation)?,
            next_batch_ordinal: u32::try_from(item.8).map_err(|_| invariant_error())?,
            item: claimed_item,
        }))
    }

    /// Mark cancellation durably. An already issued provider request may finish,
    /// but its claim can no longer activate item generations.
    pub fn request_cancel(&self, job_id: &str, now_ms: i64) -> Result<bool, PlatformError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        let target: Option<(i64, i64)> = transaction
            .query_row(
                "SELECT j.config_generation, j.index_generation FROM index_jobs j
                   WHERE j.id=?1",
                [job_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(sql_error)?;
        let updated = transaction
            .execute(
                "UPDATE index_jobs SET cancel_requested=1,
                   state=CASE WHEN state='claimed' THEN 'cancelling' ELSE 'cancelled' END,
                   ended_at_ms=CASE WHEN state IN ('queued','retry_wait') THEN ?2 ELSE ended_at_ms END,
                   updated_at_ms=?2
                 WHERE id=?1 AND state NOT IN ('completed','error','cancelled','outdated')",
                params![job_id, now_ms],
            )
            .map_err(sql_error)?;
        if updated == 1 {
            transaction
                .execute(
                    "UPDATE index_job_items SET state='cancelled', updated_at_ms=?2
                     WHERE job_id=?1 AND state NOT IN ('completed','error','outdated','cancelled')",
                    params![job_id, now_ms],
                )
                .map_err(sql_error)?;
            if let Some((config_generation, index_generation)) = target {
                abort_full_reindex(
                    &transaction,
                    config_generation,
                    index_generation,
                    Some(job_id),
                    now_ms,
                )?;
            }
            transaction
                .execute(
                    "UPDATE item_generations SET state='cancelled'
                     WHERE (item_id, generation) IN
                       (SELECT item_id, item_generation FROM index_job_items WHERE job_id=?1)
                       AND state NOT IN ('completed','error','outdated','cancelled')",
                    [job_id],
                )
                .map_err(sql_error)?;
            transaction
                .execute(
                    "UPDATE items SET status='skipped', updated_at_ms=?2
                     WHERE id IN (SELECT item_id FROM index_job_items WHERE job_id=?1)
                       AND status IN ('queued','running')",
                    params![job_id, now_ms],
                )
                .map_err(sql_error)?;
            append_job_log(&transaction, job_id, "cancelled", 1, now_ms)?;
            prune_terminal_jobs(&transaction)?;
            prune_generation_history(&transaction)?;
        }
        transaction.commit().map_err(sql_error)?;
        Ok(updated == 1)
    }

    /// Acknowledge that the owner of a cancelling claim has stopped using it.
    pub fn acknowledge_cancel(
        &self,
        claim: &AiSearchJobClaim,
        now_ms: i64,
    ) -> Result<bool, PlatformError> {
        let updated = self
            .lock()?
            .execute(
                "UPDATE index_jobs SET state='cancelled', claim_token=NULL,
                   claim_until_ms=NULL, ended_at_ms=?3, updated_at_ms=?3
                 WHERE id=?1 AND state='cancelling' AND claim_token=?2",
                params![claim.job_id, claim.claim_token, now_ms],
            )
            .map_err(sql_error)?;
        Ok(updated == 1)
    }

    /// Extend one current claim lease without changing its token or attempt.
    pub fn renew_claim(
        &self,
        claim: &AiSearchJobClaim,
        now_ms: i64,
        lease_ms: u64,
    ) -> Result<bool, PlatformError> {
        let claim_until_ms = now_ms
            .checked_add(to_i64(lease_ms)?)
            .ok_or_else(limit_error)?;
        let updated = self
            .lock()?
            .execute(
                "UPDATE index_jobs SET claim_until_ms=?3, updated_at_ms=?2
                 WHERE id=?1 AND state='claimed' AND claim_token=?4
                   AND claim_until_ms>?2 AND cancel_requested=0",
                params![claim.job_id, now_ms, claim_until_ms, claim.claim_token],
            )
            .map_err(sql_error)?;
        Ok(updated == 1)
    }

    /// Record a transient or terminal failure only when the claim token remains current.
    pub fn fail_claim(
        &self,
        claim: &AiSearchJobClaim,
        transient: bool,
        next_attempt_at_ms: i64,
        now_ms: i64,
    ) -> Result<bool, PlatformError> {
        let state = if transient { "retry_wait" } else { "error" };
        let item_state = if transient { "queued" } else { "error" };
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        let updated = transaction
            .execute(
                "UPDATE index_jobs SET state=?4, claim_token=NULL, claim_until_ms=NULL,
                   next_attempt_at_ms=?5, ended_at_ms=CASE WHEN ?4='error' THEN ?3 ELSE NULL END,
                   updated_at_ms=?3
                 WHERE id=?1 AND state='claimed' AND claim_token=?2 AND cancel_requested=0",
                params![
                    claim.job_id,
                    claim.claim_token,
                    now_ms,
                    state,
                    next_attempt_at_ms
                ],
            )
            .map_err(sql_error)?;
        if updated == 1 {
            transaction
                .execute(
                    "UPDATE index_job_items SET state=?3, updated_at_ms=?2
                     WHERE job_id=?1 AND state NOT IN ('completed','outdated','cancelled')",
                    params![claim.job_id, now_ms, item_state],
                )
                .map_err(sql_error)?;
            transaction
                .execute(
                    "UPDATE item_generations SET state=?3,
                       completed_at_ms=CASE WHEN ?3='error' THEN ?2 ELSE NULL END
                     WHERE (item_id, generation) IN
                       (SELECT item_id, item_generation FROM index_job_items WHERE job_id=?1)
                       AND state NOT IN ('completed','outdated','cancelled')",
                    params![claim.job_id, now_ms, item_state],
                )
                .map_err(sql_error)?;
            transaction
                .execute(
                    "UPDATE items SET status=?3, updated_at_ms=?2
                     WHERE id IN (SELECT item_id FROM index_job_items WHERE job_id=?1)
                       AND status IN ('queued','running')",
                    params![claim.job_id, now_ms, item_state],
                )
                .map_err(sql_error)?;
            if !transient {
                abort_full_reindex(
                    &transaction,
                    to_i64(claim.config_generation)?,
                    to_i64(claim.index_generation)?,
                    Some(&claim.job_id),
                    now_ms,
                )?;
            }
            append_item_log(
                &transaction,
                &claim.item.item_id,
                if transient { "retry_wait" } else { "error" },
                now_ms,
            )?;
            append_job_log(
                &transaction,
                &claim.job_id,
                if transient { "retry_wait" } else { "error" },
                i64::from(!transient),
                now_ms,
            )?;
            prune_terminal_jobs(&transaction)?;
            prune_generation_history(&transaction)?;
        }
        transaction.commit().map_err(sql_error)?;
        Ok(updated == 1)
    }
}

fn abort_full_reindex(
    transaction: &rusqlite::Transaction<'_>,
    config_generation: i64,
    index_generation: i64,
    terminal_job_id: Option<&str>,
    now_ms: i64,
) -> Result<(), PlatformError> {
    let state: (i64, bool) = transaction
        .query_row(
            "SELECT active_index_generation,
                    previous_model_contract_sha256 IS NOT NULL
               FROM instance_meta WHERE singleton=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(sql_error)?;
    if index_generation == state.0 || !state.1 {
        return Ok(());
    }
    transaction
        .execute(
            "DELETE FROM chunks_fts_porter WHERE chunk_id IN
               (SELECT id FROM chunks WHERE index_generation=?1)",
            [index_generation],
        )
        .map_err(sql_error)?;
    transaction
        .execute(
            "DELETE FROM chunks_fts_trigram WHERE chunk_id IN
               (SELECT id FROM chunks WHERE index_generation=?1)",
            [index_generation],
        )
        .map_err(sql_error)?;
    transaction
        .execute(
            "DELETE FROM chunks WHERE index_generation=?1",
            [index_generation],
        )
        .map_err(sql_error)?;
    transaction
        .execute(
            "INSERT OR IGNORE INTO object_gc
             (object_key, object_sha256, object_size, state, attempt,
              next_attempt_at_ms, created_at_ms, updated_at_ms)
             SELECT g.object_key, g.object_sha256, g.object_size,
                    'queued', 0, ?2, ?2, ?2
               FROM item_generations g WHERE g.index_generation=?1
                 AND NOT EXISTS (
                   SELECT 1 FROM items i JOIN item_generations retained
                     ON retained.item_id=i.id
                    WHERE retained.object_key=g.object_key
                      AND retained.object_sha256=g.object_sha256
                      AND retained.object_size=g.object_size
                      AND retained.index_generation!=?1
                      AND retained.generation IN (i.active_generation, i.desired_generation))",
            params![index_generation, now_ms],
        )
        .map_err(sql_error)?;
    transaction
        .execute(
            "UPDATE index_job_items SET state='outdated', updated_at_ms=?3
               WHERE index_generation=?1 AND job_id!=COALESCE(?2, '')
                 AND state NOT IN ('completed','error','cancelled','outdated')",
            params![index_generation, terminal_job_id, now_ms],
        )
        .map_err(sql_error)?;
    transaction
        .execute(
            "UPDATE index_jobs SET state='outdated', claim_token=NULL,
                 claim_until_ms=NULL, ended_at_ms=?4, updated_at_ms=?4
               WHERE config_generation=?1 AND index_generation=?2
                 AND id!=COALESCE(?3, '')
                 AND state NOT IN ('completed','error','cancelled','outdated')",
            params![config_generation, index_generation, terminal_job_id, now_ms],
        )
        .map_err(sql_error)?;
    transaction
        .execute(
            "UPDATE item_generations SET state='outdated'
               WHERE index_generation=?1 AND state NOT IN ('error','cancelled','outdated')",
            [index_generation],
        )
        .map_err(sql_error)?;
    transaction
        .execute(
            "UPDATE items SET desired_generation=COALESCE(active_generation, desired_generation),
                 status=CASE WHEN active_generation IS NULL THEN 'error' ELSE 'completed' END,
                 updated_at_ms=?1
               WHERE EXISTS (SELECT 1 FROM item_generations generation
                 WHERE generation.item_id=items.id
                   AND generation.generation=items.desired_generation
                   AND generation.index_generation=?2)",
            params![now_ms, index_generation],
        )
        .map_err(sql_error)?;
    let restored = transaction
        .execute(
            "UPDATE instance_meta SET
                 transition_model_contract_sha256=model_contract_sha256,
                 model_contract_sha256=previous_model_contract_sha256,
                 model_contract_json=previous_model_contract_json,
                 public_config_json=previous_public_config_json,
                 dimensions=previous_dimensions,
                 vector_enabled=previous_vector_enabled,
                 keyword_enabled=previous_keyword_enabled,
                 previous_model_contract_sha256=NULL,
                 previous_model_contract_json=NULL,
                 previous_public_config_json=NULL,
                 previous_dimensions=NULL,
                 previous_vector_enabled=NULL,
                 previous_keyword_enabled=NULL,
                 updated_at_ms=?1
               WHERE singleton=1 AND previous_model_contract_sha256 IS NOT NULL",
            [now_ms],
        )
        .map_err(sql_error)?;
    if restored != 1 {
        return Err(invariant_error());
    }
    Ok(())
}
