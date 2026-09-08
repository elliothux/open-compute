use super::*;

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
                            pending_worker_id,
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
                            pending_worker_id, consumer_generation, state, availability,
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
                     FROM version_queue_consumers c
                     WHERE version_id = ?1 AND origin = 'version'
                     ORDER BY queue_id, id",
                )
                .map_err(|_| invariant())?;
            let rows = statement
                .query_map([version_id.to_string()], map_declaration)
                .map_err(|_| invariant())?;
            collect(rows)
        })
    }

    /// Append an immutable API consumer generation to an active ready Worker version.
    pub fn create_api_declaration(
        &self,
        version_id: VersionId,
        declaration: &NewQueueConsumerDeclaration,
        now_ms: i64,
    ) -> Result<QueueConsumerDeclaration, PlatformError> {
        self.db.with_immediate(|tx| {
            insert_declaration(tx, version_id, declaration, DeclarationOrigin::Api, now_ms)?;
            tx.query_row(
                "SELECT id, version_id, queue_id, queue_lifecycle_generation,
                        entrypoint, max_batch_size, max_batch_timeout_seconds,
                        max_retries, retry_delay_seconds, max_concurrency,
                        dlq_queue_id, dlq_lifecycle_generation, capability_version,
                        descriptor_sha256, created_at_ms
                 FROM version_queue_consumers WHERE id = ?1",
                [declaration.id.to_string()],
                map_declaration,
            )
            .map_err(|_| invariant())
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
                            pending_worker_id,
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
                            pending_worker_id,
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
        worker_id: WorkerId,
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
                         pending_worker_id = ?4,
                         availability_code = CASE state
                           WHEN 'paused' THEN 'QUEUE_CONSUMER_DRAINING_PAUSED'
                           ELSE 'QUEUE_CONSUMER_DRAINING'
                         END,
                         updated_at_ms = ?5
                     WHERE id = ?6 AND queue_id = ?7 AND consumer_generation = ?8
                       AND state IN ('active', 'paused')",
                    params![
                        as_i64(next)?,
                        declaration.id.to_string(),
                        declaration.version_id.to_string(),
                        worker_id.to_string(),
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
                            worker_id = pending_worker_id, pending_declaration_id = NULL,
                            pending_version_id = NULL, pending_worker_id = NULL,
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
                            pending_version_id = NULL, pending_worker_id = NULL
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
