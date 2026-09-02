//! Durable deterministic chunk staging and generation activation.

use super::*;

impl AiSearchStore {
    /// Atomically stage one deterministic chunk batch and advance its durable
    /// resume ordinal under the exact job claim fence.
    pub fn stage_item_generation_batch(
        &self,
        claim: &AiSearchJobClaim,
        expected_ordinal: u32,
        chunks: &[StagedAiSearchChunk<'_>],
        now_ms: i64,
    ) -> Result<bool, PlatformError> {
        self.stage_item_generation_batch_with_logical_limit(
            claim,
            expected_ordinal,
            chunks,
            now_ms,
            MAX_INDEX_LOGICAL_BYTES,
        )
    }

    pub(super) fn stage_item_generation_batch_with_logical_limit(
        &self,
        claim: &AiSearchJobClaim,
        expected_ordinal: u32,
        chunks: &[StagedAiSearchChunk<'_>],
        now_ms: i64,
        max_index_logical_bytes: i64,
    ) -> Result<bool, PlatformError> {
        if chunks.is_empty() {
            return Err(limit_error());
        }
        validate_chunk_batch(
            chunks,
            expected_ordinal,
            self.vector_enabled,
            self.dimensions,
        )?;
        let next_ordinal = usize::try_from(expected_ordinal)
            .map_err(|_| limit_error())?
            .checked_add(chunks.len())
            .and_then(|value| u32::try_from(value).ok())
            .ok_or_else(limit_error)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        let fenced: bool = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM index_jobs j
                   JOIN index_job_items ji ON ji.job_id=j.id
                   JOIN instance_meta m ON m.singleton=1
                   JOIN items i ON i.id=ji.item_id
                  WHERE j.id=?1 AND j.state='claimed' AND j.claim_token=?2
                    AND j.claim_until_ms>?3 AND j.cancel_requested=0
                    AND j.config_generation=m.config_generation
                    AND j.config_generation=?4 AND j.index_generation=?5
                    AND ji.item_id=?6 AND ji.item_generation=?7
                    AND ji.next_batch_ordinal=?8 AND i.desired_generation=?7)",
                params![
                    claim.job_id,
                    claim.claim_token,
                    now_ms,
                    to_i64(claim.config_generation)?,
                    to_i64(claim.index_generation)?,
                    claim.item.item_id,
                    to_i64(claim.item.generation)?,
                    i64::from(expected_ordinal)
                ],
                |row| row.get(0),
            )
            .map_err(sql_error)?;
        if !fenced {
            transaction.commit().map_err(sql_error)?;
            return Ok(false);
        }
        let retained_chunks: i64 = transaction
            .query_row(
                "SELECT COUNT(*) FROM chunks WHERE index_generation=?1
                   AND (item_id!=?2 OR item_generation=?3)",
                params![
                    to_i64(claim.index_generation)?,
                    claim.item.item_id,
                    to_i64(claim.item.generation)?
                ],
                |row| row.get(0),
            )
            .map_err(sql_error)?;
        if retained_chunks
            .checked_add(i64::try_from(chunks.len()).map_err(|_| limit_error())?)
            .is_none_or(|count| count > MAX_CHUNKS_PER_INSTANCE)
        {
            return Err(quota_error());
        }
        let retained_logical_bytes: i64 = transaction
            .query_row(
                "SELECT COALESCE(SUM(length(CAST(text AS BLOB))
                       + COALESCE(length(embedding_f32le), 0)
                       + length(metadata_json)), 0)
                   FROM chunks WHERE index_generation=?1
                     AND (item_id!=?2 OR item_generation=?3)",
                params![
                    to_i64(claim.index_generation)?,
                    claim.item.item_id,
                    to_i64(claim.item.generation)?
                ],
                |row| row.get(0),
            )
            .map_err(sql_error)?;
        let new_logical_bytes = chunks.iter().try_fold(0_i64, |total, chunk| {
            let bytes = chunk
                .text
                .len()
                .checked_add(chunk.embedding_f32le.map_or(0, <[u8]>::len))
                .and_then(|bytes| bytes.checked_add(chunk.metadata_json.len()))
                .ok_or_else(limit_error)?;
            total
                .checked_add(i64::try_from(bytes).map_err(|_| limit_error())?)
                .ok_or_else(limit_error)
        })?;
        if retained_logical_bytes
            .checked_add(new_logical_bytes)
            .is_none_or(|bytes| bytes > max_index_logical_bytes)
        {
            return Err(quota_error());
        }
        let existing_text_bytes: i64 = transaction
            .query_row(
                "SELECT COALESCE(SUM(length(CAST(text AS BLOB))), 0) FROM chunks
                   WHERE item_id=?1 AND item_generation=?2",
                params![claim.item.item_id, to_i64(claim.item.generation)?],
                |row| row.get(0),
            )
            .map_err(sql_error)?;
        let new_text_bytes = chunks.iter().try_fold(0_i64, |total, chunk| {
            total
                .checked_add(i64::try_from(chunk.text.len()).map_err(|_| limit_error())?)
                .ok_or_else(limit_error)
        })?;
        if existing_text_bytes
            .checked_add(new_text_bytes)
            .is_none_or(|bytes| {
                bytes > i64::try_from(MAX_EXTRACTED_TEXT_BYTES_PER_ITEM).unwrap_or(i64::MAX)
            })
        {
            return Err(quota_error());
        }
        for chunk in chunks {
            insert_chunk(&transaction, claim, chunk)?;
        }
        let advanced = transaction
            .execute(
                "UPDATE index_job_items SET next_batch_ordinal=?4, updated_at_ms=?5
                   WHERE job_id=?1 AND item_id=?2 AND item_generation=?3
                     AND next_batch_ordinal=?6",
                params![
                    claim.job_id,
                    claim.item.item_id,
                    to_i64(claim.item.generation)?,
                    i64::from(next_ordinal),
                    now_ms,
                    i64::from(expected_ordinal)
                ],
            )
            .map_err(sql_error)?;
        if advanced != 1 {
            return Err(invariant_error());
        }
        transaction.commit().map_err(sql_error)?;
        Ok(true)
    }

    /// Activate only after every deterministic ordinal has been durably staged.
    pub fn complete_staged_item_generation(
        &self,
        claim: &AiSearchJobClaim,
        total_chunks: u32,
        now_ms: i64,
    ) -> Result<bool, PlatformError> {
        let complete: bool = self
            .lock()?
            .query_row(
                "SELECT ji.next_batch_ordinal=?3
                    AND (SELECT COUNT(*) FROM chunks c
                          WHERE c.item_id=ji.item_id
                            AND c.item_generation=ji.item_generation)=?3
                   FROM index_job_items ji WHERE ji.job_id=?1 AND ji.item_id=?2",
                params![claim.job_id, claim.item.item_id, i64::from(total_chunks)],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql_error)?
            .unwrap_or(false);
        if !complete {
            return Ok(false);
        }
        self.activate_item_generation(
            claim,
            &claim.item.item_id,
            claim.item.generation,
            &[],
            now_ms,
        )
    }

    /// Stage all chunks and atomically activate one item generation, guarded by
    /// the exact job token and instance/item/index generations.
    pub fn activate_item_generation(
        &self,
        claim: &AiSearchJobClaim,
        item_id: &str,
        item_generation: u64,
        chunks: &[StagedAiSearchChunk<'_>],
        now_ms: i64,
    ) -> Result<bool, PlatformError> {
        self.validate_chunks(chunks)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        let fence: Option<i64> = transaction
            .query_row(
                "SELECT 1 FROM index_jobs j JOIN instance_meta m ON m.singleton=1
                 JOIN index_job_items ji ON ji.job_id=j.id
                 JOIN items i ON i.id=ji.item_id
                 WHERE j.id=?1 AND j.state='claimed' AND j.claim_token=?2
                   AND j.claim_until_ms>?3 AND j.cancel_requested=0
                   AND j.config_generation=m.config_generation
                   AND j.config_generation=?4 AND j.index_generation=?5
                   AND ji.item_id=?6 AND ji.item_generation=?7
                   AND i.desired_generation=?7",
                params![
                    claim.job_id,
                    claim.claim_token,
                    now_ms,
                    to_i64(claim.config_generation)?,
                    to_i64(claim.index_generation)?,
                    item_id,
                    to_i64(item_generation)?
                ],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql_error)?;
        if fence.is_none() {
            transaction.commit().map_err(sql_error)?;
            return Ok(false);
        }
        let existing_chunks: i64 = transaction
            .query_row(
                "SELECT COUNT(*) FROM chunks
                  WHERE index_generation=?1
                    AND item_id!=?2",
                params![to_i64(claim.index_generation)?, item_id],
                |row| row.get(0),
            )
            .map_err(sql_error)?;
        let new_chunk_count = i64::try_from(chunks.len()).map_err(|_| limit_error())?;
        if existing_chunks
            .checked_add(new_chunk_count)
            .is_none_or(|count| count > MAX_CHUNKS_PER_INSTANCE)
        {
            return Err(quota_error());
        }
        let existing_logical_bytes: i64 = transaction
            .query_row(
                "SELECT COALESCE(SUM(length(CAST(text AS BLOB))
                       + COALESCE(length(embedding_f32le), 0)
                       + length(metadata_json)), 0)
                   FROM chunks WHERE index_generation=?1
                    AND item_id!=?2",
                params![to_i64(claim.index_generation)?, item_id],
                |row| row.get(0),
            )
            .map_err(sql_error)?;
        let new_logical_bytes = chunks.iter().try_fold(0_i64, |total, chunk| {
            let bytes = chunk
                .text
                .len()
                .checked_add(chunk.embedding_f32le.map_or(0, <[u8]>::len))
                .and_then(|value| value.checked_add(chunk.metadata_json.len()))
                .ok_or_else(limit_error)?;
            total
                .checked_add(i64::try_from(bytes).map_err(|_| limit_error())?)
                .ok_or_else(limit_error)
        })?;
        if existing_logical_bytes
            .checked_add(new_logical_bytes)
            .is_none_or(|bytes| bytes > MAX_INDEX_LOGICAL_BYTES)
        {
            return Err(quota_error());
        }
        for chunk in chunks {
            insert_chunk(&transaction, claim, chunk)?;
        }
        if !chunks.is_empty() {
            transaction
                .execute(
                    "UPDATE index_job_items SET next_batch_ordinal=?3, updated_at_ms=?4
                       WHERE job_id=?1 AND item_id=?2 AND next_batch_ordinal=0",
                    params![
                        claim.job_id,
                        item_id,
                        i64::try_from(chunks.len()).map_err(|_| limit_error())?,
                        now_ms
                    ],
                )
                .map_err(sql_error)?;
        }
        transaction
            .execute(
                "UPDATE item_generations SET state='completed', completed_at_ms=?3
                 WHERE item_id=?1 AND generation=?2",
                params![item_id, to_i64(item_generation)?, now_ms],
            )
            .map_err(sql_error)?;
        transaction
            .execute(
                "UPDATE index_job_items SET state='completed', updated_at_ms=?3
                 WHERE job_id=?1 AND item_id=?2",
                params![claim.job_id, item_id, now_ms],
            )
            .map_err(sql_error)?;
        transaction
            .execute(
                "UPDATE index_jobs SET state='completed', claim_token=NULL,
                   claim_until_ms=NULL, ended_at_ms=?2, updated_at_ms=?2
                 WHERE id=?1 AND NOT EXISTS
                   (SELECT 1 FROM index_job_items WHERE job_id=?1 AND state!='completed')",
                params![claim.job_id, now_ms],
            )
            .map_err(sql_error)?;
        let active_index_generation: i64 = transaction
            .query_row(
                "SELECT active_index_generation FROM instance_meta WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .map_err(sql_error)?;
        let claim_index_generation = to_i64(claim.index_generation)?;
        let promote_full_index = claim_index_generation != active_index_generation
            && !transaction
                .query_row(
                    "SELECT EXISTS(
                       SELECT 1 FROM index_jobs
                        WHERE config_generation=?1 AND index_generation=?2
                          AND state!='completed')",
                    params![to_i64(claim.config_generation)?, claim_index_generation],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(sql_error)?;
        if claim_index_generation == active_index_generation {
            transaction
                .execute(
                    "UPDATE items SET active_generation=?2, status='completed', updated_at_ms=?3
                      WHERE id=?1 AND desired_generation=?2",
                    params![item_id, to_i64(item_generation)?, now_ms],
                )
                .map_err(sql_error)?;
            transaction
                .execute(
                    "UPDATE instance_meta SET active_epoch=active_epoch+1,
                       updated_at_ms=?1 WHERE singleton=1",
                    [now_ms],
                )
                .map_err(sql_error)?;
        } else if promote_full_index {
            transaction
                .execute(
                    "UPDATE instance_meta SET active_index_generation=?1,
                       previous_model_contract_sha256=NULL,
                       transition_model_contract_sha256=NULL,
                       previous_model_contract_json=NULL,
                       previous_public_config_json=NULL,
                       previous_dimensions=NULL, previous_vector_enabled=NULL,
                       previous_keyword_enabled=NULL,
                       active_epoch=active_epoch+1, updated_at_ms=?2
                      WHERE singleton=1",
                    params![claim_index_generation, now_ms],
                )
                .map_err(sql_error)?;
            transaction
                .execute(
                    "UPDATE items SET active_generation=desired_generation,
                       status='completed', updated_at_ms=?1",
                    [now_ms],
                )
                .map_err(sql_error)?;
        }
        if claim_index_generation == active_index_generation || promote_full_index {
            transaction
                .execute(
                    "UPDATE item_generations SET state='outdated'
                      WHERE generation NOT IN (
                        SELECT active_generation FROM items WHERE items.id=item_generations.item_id)
                        AND state NOT IN ('error','cancelled','outdated')",
                    [],
                )
                .map_err(sql_error)?;
            transaction
                .execute(
                    "INSERT OR IGNORE INTO object_gc
                     (object_key, object_sha256, object_size, state, attempt,
                      next_attempt_at_ms, created_at_ms, updated_at_ms)
                     SELECT DISTINCT g.object_key, g.object_sha256, g.object_size,
                       'queued', 0, ?1, ?1, ?1 FROM item_generations g
                      WHERE NOT EXISTS (
                        SELECT 1 FROM items i JOIN item_generations retained
                          ON retained.item_id=i.id
                         WHERE retained.object_key=g.object_key
                           AND retained.object_sha256=g.object_sha256
                           AND retained.object_size=g.object_size
                           AND retained.generation IN (i.active_generation, i.desired_generation))",
                    [now_ms],
                )
                .map_err(sql_error)?;
            transaction
                .execute(
                    "DELETE FROM chunks_fts_porter WHERE chunk_id IN (
                       SELECT c.id FROM chunks c JOIN items i ON i.id=c.item_id
                        WHERE c.item_generation!=i.active_generation)",
                    [],
                )
                .map_err(sql_error)?;
            transaction
                .execute(
                    "DELETE FROM chunks_fts_trigram WHERE chunk_id IN (
                       SELECT c.id FROM chunks c JOIN items i ON i.id=c.item_id
                        WHERE c.item_generation!=i.active_generation)",
                    [],
                )
                .map_err(sql_error)?;
            transaction
                .execute(
                    "DELETE FROM chunks WHERE EXISTS (
                       SELECT 1 FROM items i WHERE i.id=chunks.item_id
                         AND chunks.item_generation!=i.active_generation)",
                    [],
                )
                .map_err(sql_error)?;
            transaction
                .execute(
                    "DELETE FROM chunks_fts_porter WHERE chunk_id IN
                       (SELECT id FROM chunks WHERE index_generation!=?1)",
                    [claim_index_generation],
                )
                .map_err(sql_error)?;
            transaction
                .execute(
                    "DELETE FROM chunks_fts_trigram WHERE chunk_id IN
                       (SELECT id FROM chunks WHERE index_generation!=?1)",
                    [claim_index_generation],
                )
                .map_err(sql_error)?;
            transaction
                .execute(
                    "DELETE FROM chunks WHERE index_generation!=?1",
                    [claim_index_generation],
                )
                .map_err(sql_error)?;
        }
        append_item_log(&transaction, item_id, "completed", now_ms)?;
        append_job_log(&transaction, &claim.job_id, "completed", 0, now_ms)?;
        prune_terminal_jobs(&transaction)?;
        prune_generation_history(&transaction)?;
        transaction.commit().map_err(sql_error)?;
        Ok(true)
    }
}

fn insert_chunk(
    transaction: &rusqlite::Transaction<'_>,
    claim: &AiSearchJobClaim,
    chunk: &StagedAiSearchChunk<'_>,
) -> Result<(), PlatformError> {
    transaction
        .execute(
            "INSERT INTO chunks
             (id, item_id, item_generation, index_generation, ordinal,
              start_byte, end_byte, text, embedding_f32le, vector_norm, metadata_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                chunk.chunk_id,
                claim.item.item_id,
                to_i64(claim.item.generation)?,
                to_i64(claim.index_generation)?,
                i64::from(chunk.ordinal),
                to_i64(chunk.start_byte)?,
                to_i64(chunk.end_byte)?,
                chunk.text,
                chunk.embedding_f32le,
                chunk.vector_norm,
                chunk.metadata_json
            ],
        )
        .map_err(sql_error)?;
    transaction
        .execute(
            "INSERT INTO chunks_fts_porter(chunk_id, text) VALUES (?1, ?2)",
            params![chunk.chunk_id, chunk.text],
        )
        .map_err(sql_error)?;
    transaction
        .execute(
            "INSERT INTO chunks_fts_trigram(chunk_id, text) VALUES (?1, ?2)",
            params![chunk.chunk_id, chunk.text],
        )
        .map_err(sql_error)?;
    Ok(())
}
