use super::*;

impl D1SnapshotRepository<'_> {
    /// Reserve the one identity-preserving restore allowed for a database.
    #[allow(
        clippy::too_many_arguments,
        reason = "SQLite boundary inputs mirror authoritative persisted fields"
    )]
    pub fn prepare_restore(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        intent_id: &str,
        source_session_version: u64,
        previous_session_version: u64,
        request_fingerprint: &[u8; 32],
        now_ms: i64,
    ) -> Result<D1RestoreIntent, PlatformError> {
        validate_uuid(intent_id)?;
        if now_ms < 0 {
            return Err(invariant());
        }
        let result_session_version = previous_session_version
            .checked_add(1)
            .ok_or_else(invariant)?;
        self.db.with_immediate(|tx| {
            ensure_account_database(tx, account_id, resource_id)?;
            let _ = read_snapshot(tx, resource_id, source_session_version)?;
            let latest = read_latest_snapshot(tx, resource_id)?.ok_or_else(invariant)?;
            if latest.session_version != previous_session_version {
                return Err(invariant());
            }
            if let Some(existing) = read_restore_optional(tx, resource_id)? {
                if existing.id == intent_id
                    && existing.source_session_version == source_session_version
                    && existing.previous_session_version == previous_session_version
                    && existing.request_fingerprint == *request_fingerprint
                {
                    return Ok(existing);
                }
                return Err(idempotency_conflict());
            }
            if read_active_transfer(tx, resource_id)?.is_some() {
                return Err(busy());
            }
            tx.execute(
                "INSERT INTO d1_restore_intents
                 (id, resource_id, source_session_version, previous_session_version,
                  result_session_version, request_fingerprint, created_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    intent_id,
                    resource_id.to_string(),
                    to_i64(source_session_version)?,
                    to_i64(previous_session_version)?,
                    to_i64(result_session_version)?,
                    request_fingerprint.as_slice(),
                    now_ms
                ],
            )
            .map_err(|_| invariant())?;
            read_restore(tx, resource_id)
        })
    }

    /// Read the pending restore fence for a database.
    pub fn pending_restore(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
    ) -> Result<Option<D1RestoreIntent>, PlatformError> {
        self.db.with_read(|conn| {
            ensure_account_database(conn, account_id, resource_id)?;
            read_restore_optional(conn, resource_id)
        })
    }

    /// Release a restore fence only after its result snapshot is complete.
    pub fn complete_restore(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        intent_id: &str,
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            ensure_account_database(tx, account_id, resource_id)?;
            let intent = read_restore(tx, resource_id)?;
            if intent.id != intent_id {
                return Err(idempotency_conflict());
            }
            let _ = read_snapshot(tx, resource_id, intent.result_session_version)?;
            if tx
                .execute(
                    "DELETE FROM d1_restore_intents WHERE resource_id = ?1 AND id = ?2",
                    params![resource_id.to_string(), intent_id],
                )
                .map_err(|_| invariant())?
                != 1
            {
                return Err(invariant());
            }
            Ok(())
        })
    }
}
