//! Instance configuration transitions and full-reindex fencing.

use super::*;

impl AiSearchStore {
    /// Persist a completed user synchronization job when the built-in upload
    /// source has no external inventory to scan.
    pub fn create_completed_job(
        &self,
        job_id: &str,
        description: Option<&str>,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        validate_identity(job_id)?;
        if description
            .is_some_and(|value| value.len() > 4_096 || value.chars().any(char::is_control))
        {
            return Err(limit_error());
        }
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        transaction
            .execute(
                "INSERT INTO index_jobs
                 (id, source, description, state, config_generation, index_generation,
                  attempt, next_attempt_at_ms, cancel_requested, created_at_ms,
                  started_at_ms, ended_at_ms, updated_at_ms)
                 SELECT ?1, 'user', ?2, 'completed', config_generation,
                        active_index_generation, 0, ?3, 0, ?3, ?3, ?3, ?3
                   FROM instance_meta WHERE singleton=1",
                params![job_id, description, now_ms],
            )
            .map_err(sql_error)?;
        prune_terminal_jobs(&transaction)?;
        transaction.commit().map_err(sql_error)
    }

    /// Replace only the mutable public configuration and advance its fence.
    pub fn update_public_config(
        &self,
        expected_generation: u64,
        public_config_json: &[u8],
        now_ms: i64,
    ) -> Result<bool, PlatformError> {
        if !canonical_json_object(public_config_json, 65_536) {
            return Err(limit_error());
        }
        let updated = self
            .lock()?
            .execute(
                "UPDATE instance_meta SET public_config_json=?1,
                   config_generation=config_generation+1, updated_at_ms=?2
                 WHERE singleton=1 AND config_generation=?3",
                params![public_config_json, now_ms, to_i64(expected_generation)?],
            )
            .map_err(sql_error)?;
        Ok(updated == 1)
    }

    /// Fence the current configuration and queue every catalog item into one
    /// new full-index generation. The caller must reopen this store with the new
    /// contract before processing jobs because dimensions and index modes may change.
    pub fn begin_full_reindex(
        &self,
        expected_generation: u64,
        contract: &AiSearchInstanceStorageContract<'_>,
        job_prefix: &str,
        now_ms: i64,
    ) -> Result<bool, PlatformError> {
        validate_identity(job_prefix)?;
        let model_digest: [u8; 32] = Sha256::digest(contract.model_contract_json).into();
        if model_digest != contract.model_contract_sha256 || !valid_instance_contract(contract) {
            return Err(limit_error());
        }
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        let current: (String, i64, i64, bool) = transaction
            .query_row(
                "SELECT resource_id, config_generation, active_index_generation,
                        previous_model_contract_sha256 IS NOT NULL
                          OR transition_model_contract_sha256 IS NOT NULL
                   FROM instance_meta WHERE singleton=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .map_err(sql_error)?;
        if current.1 != to_i64(expected_generation)? {
            transaction.commit().map_err(sql_error)?;
            return Ok(false);
        }
        if current.0 != contract.resource_id {
            return Err(invariant_error());
        }
        if current.3 {
            transaction.commit().map_err(sql_error)?;
            return Ok(false);
        }
        let config_generation = current.1.checked_add(1).ok_or_else(limit_error)?;
        let index_generation = current.2.checked_add(1).ok_or_else(limit_error)?;
        transaction
            .execute(
                "UPDATE index_job_items SET state='outdated', updated_at_ms=?1
                  WHERE state NOT IN ('completed','error','cancelled','outdated')",
                [now_ms],
            )
            .map_err(sql_error)?;
        transaction
            .execute(
                "UPDATE item_generations SET state='outdated'
                  WHERE state NOT IN ('completed','error','cancelled','outdated')",
                [],
            )
            .map_err(sql_error)?;
        transaction
            .execute(
                "UPDATE index_jobs SET state='outdated', claim_token=NULL,
                   claim_until_ms=NULL, ended_at_ms=?1, updated_at_ms=?1
                  WHERE state NOT IN ('completed','error','cancelled','outdated')",
                [now_ms],
            )
            .map_err(sql_error)?;
        let items = {
            let mut statement = transaction
                .prepare(
                    "SELECT i.id, i.desired_generation, g.object_key, g.object_sha256,
                            g.object_size, g.content_type
                       FROM items i JOIN item_generations g
                         ON g.item_id=i.id AND g.generation=i.desired_generation
                      ORDER BY i.id",
                )
                .map_err(sql_error)?;
            let rows = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Vec<u8>>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                })
                .map_err(sql_error)?;
            let mut items = Vec::new();
            for row in rows {
                items.push(row.map_err(sql_error)?);
            }
            items
        };
        if i64::try_from(items.len()).map_err(|_| limit_error())? > MAX_QUEUED_JOBS_PER_INSTANCE {
            return Err(quota_error());
        }
        let queued_bytes = items.iter().try_fold(0_i64, |total, item| {
            if item.4 < 0 {
                return Err(invariant_error());
            }
            total.checked_add(item.4).ok_or_else(limit_error)
        })?;
        if queued_bytes > MAX_QUEUED_BYTES_PER_INSTANCE {
            return Err(quota_error());
        }
        prune_terminal_jobs(&transaction)?;
        for (ordinal, item) in items.iter().enumerate() {
            let generation = item.1.checked_add(1).ok_or_else(limit_error)?;
            let job_id = format!("{job_prefix}-{ordinal}");
            validate_identity(&job_id)?;
            transaction
                .execute(
                    "INSERT INTO item_generations
                     (item_id, generation, index_generation, state, object_key,
                      object_sha256, object_size, content_type, created_at_ms)
                     VALUES (?1, ?2, ?3, 'queued', ?4, ?5, ?6, ?7, ?8)",
                    params![
                        item.0,
                        generation,
                        index_generation,
                        item.2,
                        item.3,
                        item.4,
                        item.5,
                        now_ms
                    ],
                )
                .map_err(sql_error)?;
            transaction
                .execute(
                    "UPDATE items SET desired_generation=?2, status='queued', updated_at_ms=?3
                      WHERE id=?1",
                    params![item.0, generation, now_ms],
                )
                .map_err(sql_error)?;
            transaction
                .execute(
                    "INSERT INTO index_jobs
                     (id, source, description, state, config_generation, index_generation,
                      attempt, next_attempt_at_ms, cancel_requested, created_at_ms, updated_at_ms)
                     VALUES (?1, 'user', 'full reindex', 'queued', ?2, ?3, 0, ?4, 0, ?4, ?4)",
                    params![job_id, config_generation, index_generation, now_ms],
                )
                .map_err(sql_error)?;
            transaction
                .execute(
                    "INSERT INTO index_job_items
                     (job_id, item_id, item_generation, index_generation, state,
                      next_batch_ordinal, updated_at_ms)
                     VALUES (?1, ?2, ?3, ?4, 'queued', 0, ?5)",
                    params![job_id, item.0, generation, index_generation, now_ms],
                )
                .map_err(sql_error)?;
            append_item_log(&transaction, &item.0, "reindex_queued", now_ms)?;
            append_job_log(&transaction, &job_id, "queued", 0, now_ms)?;
        }
        transaction
            .execute(
                "UPDATE instance_meta SET
                   previous_model_contract_sha256=model_contract_sha256,
                   transition_model_contract_sha256=model_contract_sha256,
                   previous_model_contract_json=model_contract_json,
                   previous_public_config_json=public_config_json,
                   previous_dimensions=dimensions,
                   previous_vector_enabled=vector_enabled,
                   previous_keyword_enabled=keyword_enabled,
                   model_contract_sha256=?1, model_contract_json=?2,
                   public_config_json=?3, dimensions=?4, vector_enabled=?5,
                   keyword_enabled=?6, config_generation=?7,
                   active_index_generation=CASE WHEN ?8=0 THEN ?9 ELSE active_index_generation END,
                   updated_at_ms=?10 WHERE singleton=1",
                params![
                    contract.model_contract_sha256,
                    contract.model_contract_json,
                    contract.public_config_json,
                    i64::from(contract.dimensions),
                    contract.vector_enabled,
                    contract.keyword_enabled,
                    config_generation,
                    i64::try_from(items.len()).map_err(|_| limit_error())?,
                    index_generation,
                    now_ms
                ],
            )
            .map_err(sql_error)?;
        transaction.commit().map_err(sql_error)?;
        Ok(true)
    }

    /// Clear the cross-database reindex fence for an empty instance after the
    /// central catalog has durably advanced to the supplied model digest.
    pub fn complete_empty_reindex(
        &self,
        model_contract_sha256: [u8; 32],
        now_ms: i64,
    ) -> Result<bool, PlatformError> {
        let updated = self
            .lock()?
            .execute(
                "UPDATE instance_meta SET previous_model_contract_sha256=NULL,
                   transition_model_contract_sha256=NULL,
                   previous_model_contract_json=NULL, previous_public_config_json=NULL,
                   previous_dimensions=NULL, previous_vector_enabled=NULL,
                   previous_keyword_enabled=NULL,
                   updated_at_ms=?2 WHERE singleton=1
                   AND model_contract_sha256=?1
                   AND previous_model_contract_sha256 IS NOT NULL
                   AND NOT EXISTS(SELECT 1 FROM items)",
                params![model_contract_sha256, now_ms],
            )
            .map_err(sql_error)?;
        Ok(updated == 1)
    }

    /// Retire a catalog-only transition digest after the control catalog has
    /// been reconciled to the active instance digest.
    pub fn complete_catalog_transition(
        &self,
        model_contract_sha256: [u8; 32],
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        self.lock()?
            .execute(
                "UPDATE instance_meta SET transition_model_contract_sha256=NULL,
                   updated_at_ms=?2 WHERE singleton=1 AND model_contract_sha256=?1
                   AND previous_model_contract_sha256 IS NULL",
                params![model_contract_sha256, now_ms],
            )
            .map_err(sql_error)?;
        Ok(())
    }
}
