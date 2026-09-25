use super::*;

impl<'a> WorkerRepository<'a> {
    /// Reserve or replay a secret-safe control idempotency key.
    #[allow(
        clippy::too_many_arguments,
        reason = "SQLite boundary inputs mirror authoritative persisted fields"
    )]
    pub fn reserve_idempotency(
        &self,
        instance_id: InstanceId,
        scope: &str,
        key: &str,
        fingerprint_key_id: &str,
        fingerprint: &[u8; 32],
        now_ms: i64,
        expires_at_ms: i64,
    ) -> Result<IdempotencyReservation, PlatformError> {
        self.db.with_immediate(|tx| {
            require_instance(tx, instance_id)?;
            let existing: Option<(String, Vec<u8>, Option<Vec<u8>>)> = tx
                .query_row(
                    "SELECT state, request_fingerprint, response_json
                 FROM control_idempotency
                 WHERE scope = ?1 AND idempotency_key = ?2",
                    params![scope, key],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
                .map_err(|_| db_error())?;
            if let Some((state, stored, response)) = existing {
                if stored.as_slice() != fingerprint {
                    return Err(PlatformError::new(
                        ErrorCode::IdempotencyConflict,
                        "idempotency key fingerprint does not match",
                    ));
                }
                return match (state.as_str(), response) {
                    ("complete", Some(body)) => Ok(IdempotencyReservation::Complete(body)),
                    ("running", _) => Ok(IdempotencyReservation::Running),
                    ("failed", Some(body)) => Ok(IdempotencyReservation::Failed(body)),
                    _ => Err(PlatformError::new(
                        ErrorCode::Internal,
                        "idempotency row is in an invalid state",
                    )),
                };
            }
            tx.execute(
                "INSERT INTO control_idempotency
                 (scope, idempotency_key, fingerprint_key_id,
                  request_fingerprint, response_json, state, created_at_ms, expires_at_ms)
                 VALUES (?1, ?2, ?3, ?4, NULL, 'running', ?5, ?6)",
                params![
                    scope,
                    key,
                    fingerprint_key_id,
                    fingerprint.as_slice(),
                    now_ms,
                    expires_at_ms
                ],
            )
            .map_err(|_| db_error())?;
            Ok(IdempotencyReservation::Reserved)
        })
    }

    /// Persist the canonical response for an owned idempotency reservation.
    pub fn complete_idempotency(
        &self,
        instance_id: InstanceId,
        scope: &str,
        key: &str,
        fingerprint: &[u8; 32],
        response: &[u8],
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            require_instance(tx, instance_id)?;
            let changed = tx
                .execute(
                    "UPDATE control_idempotency SET state = 'complete', response_json = ?1
                 WHERE scope = ?2 AND idempotency_key = ?3
                   AND state = 'running' AND request_fingerprint = ?4",
                    params![response, scope, key, fingerprint.as_slice()],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(PlatformError::new(
                    ErrorCode::IdempotencyConflict,
                    "idempotency reservation is no longer owned",
                ));
            }
            Ok(())
        })
    }

    /// Complete an idempotent Queue mutation and retain its exact Queue identity.
    pub fn complete_idempotency_with_queue_ref(
        &self,
        instance_id: InstanceId,
        scope: &str,
        key: &str,
        fingerprint: &[u8; 32],
        response: &[u8],
        queue_id: open_compute_core::QueueId,
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            require_instance(tx, instance_id)?;
            let changed = tx
                .execute(
                    "UPDATE control_idempotency SET state = 'complete', response_json = ?1,
                            queue_id = ?5
                     WHERE scope = ?2 AND idempotency_key = ?3
                       AND state = 'running' AND request_fingerprint = ?4
                       AND (queue_id IS NULL OR queue_id = ?5)",
                    params![
                        response,
                        scope,
                        key,
                        fingerprint.as_slice(),
                        queue_id.to_string(),
                    ],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(PlatformError::new(
                    ErrorCode::IdempotencyConflict,
                    "Queue idempotency reservation is no longer owned",
                ));
            }
            Ok(())
        })
    }

    /// Complete an idempotent response and register its version readback ref atomically.
    #[allow(
        clippy::too_many_arguments,
        reason = "SQLite boundary inputs mirror authoritative persisted fields"
    )]
    pub fn complete_idempotency_with_version_ref(
        &self,
        instance_id: InstanceId,
        scope: &str,
        key: &str,
        fingerprint: &[u8; 32],
        response: &[u8],
        version_id: VersionId,
        ref_id: &str,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        validate_referrer("control_idempotency", ref_id)?;
        if ref_id != idempotency_ref_id(instance_id, scope, key) {
            return Err(invariant());
        }
        self.db.with_immediate(|tx| {
            require_instance(tx, instance_id)?;
            let changed = tx
                .execute(
                    "UPDATE control_idempotency SET state = 'complete', response_json = ?1,
                            version_id = ?5
                     WHERE scope = ?2 AND idempotency_key = ?3
                       AND state = 'running' AND request_fingerprint = ?4",
                    params![
                        response,
                        scope,
                        key,
                        fingerprint.as_slice(),
                        version_id.to_string()
                    ],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(PlatformError::new(
                    ErrorCode::IdempotencyConflict,
                    "idempotency reservation is no longer owned",
                ));
            }
            tx.execute(
                "INSERT OR IGNORE INTO version_referrers
                 (version_id, kind, ref_id, created_at_ms)
                 VALUES (?1, 'control_idempotency', ?2, ?3)",
                params![version_id.to_string(), ref_id, now_ms],
            )
            .map_err(|_| db_error())?;
            Ok(())
        })
    }

    /// Persist a stable secret-safe failure for deterministic replay.
    pub fn fail_idempotency(
        &self,
        instance_id: InstanceId,
        scope: &str,
        key: &str,
        fingerprint: &[u8; 32],
        response: &[u8],
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            require_instance(tx, instance_id)?;
            let changed = tx
                .execute(
                    "UPDATE control_idempotency SET state = 'failed', response_json = ?1
                 WHERE scope = ?2 AND idempotency_key = ?3
                   AND state = 'running' AND request_fingerprint = ?4",
                    params![response, scope, key, fingerprint.as_slice()],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(PlatformError::new(
                    ErrorCode::IdempotencyConflict,
                    "idempotency reservation is no longer owned",
                ));
            }
            Ok(())
        })
    }
}
