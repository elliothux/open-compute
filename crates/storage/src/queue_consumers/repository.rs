use super::*;

impl<'a> QueueConsumerRepository<'a> {
    /// Bind the central control database.
    #[must_use]
    pub const fn new(db: &'a ControlDb) -> Self {
        Self { db }
    }

    /// Read one Queue consumer attachment by identity.
    pub fn get(&self, id: QueueConsumerId) -> Result<QueueConsumerRecord, PlatformError> {
        self.db.with_read(|connection| {
            connection
                .query_row(
                    "SELECT id, (SELECT instance_id FROM instance_identity), queue_id, worker_id, declaration_id,
                            version_id,
                            consumer_generation, state, created_at_ms, updated_at_ms
                     FROM queue_consumers WHERE id = ?1",
                    [id],
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
                    [id],
                    map_declaration,
                )
                .map_err(|_| invariant())
        })
    }

    /// List a bounded global operator view of live attachments.
    pub fn list_live(&self, limit: u32) -> Result<Vec<QueueConsumerRecord>, PlatformError> {
        if limit == 0 {
            return Err(invariant());
        }
        self.db.with_read(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT id, (SELECT instance_id FROM instance_identity), queue_id, worker_id, declaration_id,
                            version_id, consumer_generation, state, created_at_ms, updated_at_ms
                     FROM queue_consumers
                     ORDER BY queue_id, id LIMIT ?1",
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
                .query_map([version_id], map_declaration)
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
                [declaration.id],
                map_declaration,
            )
            .map_err(|_| invariant())
        })
    }

    /// Read the attachment for a Queue, if present.
    pub fn live_for_queue(
        &self,
        queue_id: QueueId,
    ) -> Result<Option<QueueConsumerRecord>, PlatformError> {
        self.db.with_read(|connection| {
            connection
                .query_row(
                    "SELECT id, (SELECT instance_id FROM instance_identity), queue_id, worker_id, declaration_id,
                            version_id,
                            consumer_generation, state, created_at_ms, updated_at_ms
                     FROM queue_consumers WHERE queue_id = ?1",
                    [queue_id],
                    map_record,
                )
                .optional()
                .map_err(|_| invariant())
        })
    }

    /// List attachments owned by one Worker.
    pub fn live_for_worker(
        &self,
        worker_id: WorkerId,
    ) -> Result<Vec<QueueConsumerRecord>, PlatformError> {
        self.db.with_read(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT id, (SELECT instance_id FROM instance_identity), queue_id, worker_id, declaration_id,
                            version_id,
                            consumer_generation, state, created_at_ms, updated_at_ms
                     FROM queue_consumers WHERE worker_id = ?1
                     ORDER BY queue_id, id",
                )
                .map_err(|_| invariant())?;
            let rows = statement
                .query_map([worker_id], map_record)
                .map_err(|_| invariant())?;
            collect(rows)
        })
    }

    /// Attach or replace a consumer and its scheduler target atomically.
    pub fn upsert_attachment(
        &self,
        instance_id: InstanceId,
        worker_id: WorkerId,
        declaration: &QueueConsumerDeclaration,
        now_ms: i64,
    ) -> Result<QueueConsumerRecord, PlatformError> {
        self.db.with_immediate(|tx| {
            crate::worker_repository::require_instance(tx, instance_id)?;
            let current: bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM workers w JOIN worker_deployments p ON p.id=w.active_deployment_id
                JOIN version_queue_consumers d ON d.id=?1 WHERE w.id=?2 AND p.version_id=d.version_id AND w.deleted_at_ms IS NULL)",params![declaration.id,worker_id],|row|row.get(0)).map_err(|_| invariant())?;
            if !current {return Err(PlatformError::new(ErrorCode::QueueConsumerNotReady,"Queue consumer target changed"));}
            let id = tx.query_row("SELECT id FROM queue_consumers WHERE queue_id=?1", [declaration.queue_id], |row| row.get::<_,QueueConsumerId>(0))
                .optional().map_err(|_| invariant())?.unwrap_or_else(QueueConsumerId::generate);
            tx.execute("INSERT INTO queue_consumers
                (id,queue_id,worker_id,declaration_id,version_id,consumer_generation,state,created_at_ms,updated_at_ms)
                VALUES(?1,?2,?3,?4,?5,1,CASE WHEN (SELECT delivery_paused FROM queues WHERE id=?2) THEN 'paused' ELSE 'active' END,?6,?6)
                ON CONFLICT(queue_id) DO UPDATE SET worker_id=?3,declaration_id=?4,version_id=?5,
                  consumer_generation=consumer_generation+1,updated_at_ms=?6",
                params![id,declaration.queue_id,worker_id,declaration.id,declaration.version_id,now_ms]).map_err(|_| invariant())?;
            tx.execute("DELETE FROM queue_consumer_state WHERE consumer_id=?1", [id]).map_err(|_| invariant())?;
            insert_scheduler_targets_tx(tx, worker_id).map_err(|_| invariant())?;
            read_record_tx(tx, id)
        })
    }

    /// Remove a consumer without invalidating batches already claimed.
    pub fn delete(&self, id: QueueConsumerId) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            tx.execute(
                "DELETE FROM queue_consumer_state WHERE consumer_id=?1",
                [id],
            )
            .map_err(|_| invariant())?;
            tx.execute("DELETE FROM queue_consumers WHERE id=?1", [id])
                .map_err(|_| invariant())?;
            Ok(())
        })
    }

    /// Pause new claims using the operator's exact attachment revision.
    pub fn pause(
        &self,
        id: QueueConsumerId,
        generation: u64,
        request_id: RequestId,
        now_ms: i64,
    ) -> Result<bool, PlatformError> {
        self.set_state(
            id,
            generation,
            QueueConsumerState::Paused,
            "queue_consumer.pause",
            request_id,
            now_ms,
        )
    }

    /// Resume new claims using the operator's exact attachment revision.
    pub fn resume(
        &self,
        id: QueueConsumerId,
        generation: u64,
        request_id: RequestId,
        now_ms: i64,
    ) -> Result<bool, PlatformError> {
        self.set_state(
            id,
            generation,
            QueueConsumerState::Active,
            "queue_consumer.resume",
            request_id,
            now_ms,
        )
    }

    fn set_state(
        self,
        id: QueueConsumerId,
        generation: u64,
        state: QueueConsumerState,
        action: &str,
        request_id: RequestId,
        now_ms: i64,
    ) -> Result<bool, PlatformError> {
        self.db.with_immediate(|tx| {
            let changed=tx.execute("UPDATE queue_consumers SET state=?1,updated_at_ms=?2 WHERE id=?3 AND consumer_generation=?4",
                params![state,now_ms,id,as_i64(generation)?]).map_err(|_| invariant())?;
            if changed==1 {
                tx.execute("UPDATE queue_consumer_state SET state=?1,updated_at_ms=?2 WHERE consumer_id=?3",params![state,now_ms,id]).map_err(|_| invariant())?;
                audit_operator_action(tx,id,generation,action,request_id,now_ms)?;
            }
            Ok(changed==1)
        })
    }
}
