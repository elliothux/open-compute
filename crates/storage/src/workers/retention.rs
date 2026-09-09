use super::*;

impl<'a> WorkerRepository<'a> {
    /// Register a typed version referrer. Future products must use this table.
    pub fn add_version_referrer(
        &self,
        version_id: VersionId,
        kind: &str,
        ref_id: &str,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        validate_referrer(kind, ref_id)?;
        self.db.with_immediate(|tx| {
            let state: Option<String> = tx
                .query_row(
                    "SELECT state FROM worker_versions WHERE id = ?1",
                    [version_id.to_string()],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|_| db_error())?;
            if !state.is_some_and(|state| matches!(state.as_str(), "ready" | "rejected")) {
                return Err(PlatformError::new(
                    ErrorCode::VersionNotReady,
                    "version cannot accept a referrer in its current state",
                ));
            }
            tx.execute(
                "INSERT OR IGNORE INTO version_referrers
                 (version_id, kind, ref_id, created_at_ms) VALUES (?1, ?2, ?3, ?4)",
                params![version_id.to_string(), kind, ref_id, now_ms],
            )
            .map_err(|_| db_error())?;
            Ok(())
        })
    }

    /// Remove a typed version referrer after its owner no longer needs replay/readback.
    pub fn remove_version_referrer(
        &self,
        version_id: VersionId,
        kind: &str,
        ref_id: &str,
    ) -> Result<(), PlatformError> {
        validate_referrer(kind, ref_id)?;
        self.db.with_immediate(|tx| {
            tx.execute(
                "DELETE FROM version_referrers
                 WHERE version_id = ?1 AND kind = ?2 AND ref_id = ?3",
                params![version_id.to_string(), kind, ref_id],
            )
            .map_err(|_| db_error())?;
            Ok(())
        })
    }

    /// Enumerate every registered non-memory referrer for one version.
    pub fn version_referrers(
        &self,
        version_id: VersionId,
    ) -> Result<Vec<VersionReferrer>, PlatformError> {
        self.db.with_read(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT version_id, kind, ref_id, created_at_ms
                     FROM version_referrers WHERE version_id = ?1
                     ORDER BY kind, ref_id",
                )
                .map_err(|_| db_error())?;
            let rows = stmt
                .query_map([version_id.to_string()], |row| {
                    let id: String = row.get(0)?;
                    Ok(VersionReferrer {
                        version_id: VersionId::from_str(&id)
                            .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        kind: row.get(1)?,
                        ref_id: row.get(2)?,
                        created_at_ms: row.get(3)?,
                    })
                })
                .map_err(|_| db_error())?;
            collect_rows(rows)
        })
    }

    /// Fence a non-active version in `SQLite` before waiting on in-memory pins.
    pub fn begin_version_delete(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
        version_id: VersionId,
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            let worker = require_live_worker(tx, account_id, worker_id)?;
            require_tenant_worker(&worker)?;
            if worker.active_version_id == Some(version_id) {
                return Err(PlatformError::new(
                    ErrorCode::VersionActive,
                    "active version cannot be deleted",
                ));
            }
            let referenced: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM version_referrers WHERE version_id = ?1)",
                    [version_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(|_| db_error())?;
            if referenced {
                return Err(PlatformError::new(
                    ErrorCode::VersionReferenced,
                    "version still has registered referrers",
                ));
            }
            let state: Option<String> = tx
                .query_row(
                    "SELECT state FROM worker_versions WHERE id = ?1 AND worker_id = ?2",
                    params![version_id.to_string(), worker_id.to_string()],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|_| db_error())?;
            if state.as_deref() == Some("deleting") {
                return Ok(());
            }
            let changed = tx
                .execute(
                    "UPDATE worker_versions SET state = 'deleting'
                 WHERE id = ?1 AND worker_id = ?2 AND state IN ('ready', 'rejected')",
                    params![version_id.to_string(), worker_id.to_string()],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(version_not_found());
            }
            Ok(())
        })
    }

    /// Finish a deleting version after its process-local pins drained.
    pub fn finalize_version_delete(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
        version_id: VersionId,
        request_id: RequestId,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            read_worker_tx(tx, account_id, worker_id)?;
            let deleting: bool = tx
                .query_row(
                    "SELECT EXISTS(
                        SELECT 1 FROM worker_versions
                        WHERE id = ?1 AND worker_id = ?2 AND state = 'deleting'
                    )",
                    params![version_id.to_string(), worker_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(|_| db_error())?;
            if !deleting {
                return Err(version_not_found());
            }
            let referenced: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM version_referrers WHERE version_id = ?1)",
                    [version_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(|_| db_error())?;
            if referenced {
                return Err(PlatformError::new(
                    ErrorCode::VersionReferenced,
                    "version acquired a registered referrer while deleting",
                ));
            }
            tx.execute(
                "DELETE FROM version_services WHERE version_id = ?1",
                [version_id.to_string()],
            )
            .map_err(|_| db_error())?;
            tx.execute(
                "DELETE FROM version_cron_declarations WHERE version_id = ?1",
                [version_id.to_string()],
            )
            .map_err(|_| db_error())?;
            tx.execute(
                "DELETE FROM version_cron_configs WHERE version_id = ?1",
                [version_id.to_string()],
            )
            .map_err(|_| db_error())?;
            tx.execute(
                "DELETE FROM version_queue_consumers WHERE version_id = ?1",
                [version_id.to_string()],
            )
            .map_err(|_| db_error())?;
            tx.execute(
                "DELETE FROM queue_producer_bindings WHERE version_id = ?1",
                [version_id.to_string()],
            )
            .map_err(|_| db_error())?;
            tx.execute(
                "DELETE FROM workflow_bindings WHERE version_id = ?1",
                [version_id.to_string()],
            )
            .map_err(|_| db_error())?;
            tx.execute(
                "DELETE FROM version_bindings WHERE version_id = ?1",
                [version_id.to_string()],
            )
            .map_err(|_| db_error())?;
            tx.execute(
                "DELETE FROM version_vars WHERE version_id = ?1",
                [version_id.to_string()],
            )
            .map_err(|_| db_error())?;
            tx.execute(
                "DELETE FROM version_secrets WHERE version_id = ?1",
                [version_id.to_string()],
            )
            .map_err(|_| db_error())?;
            crate::assets::delete_version_assets(tx, version_id)?;
            let changed = tx
                .execute(
                    "UPDATE worker_versions SET state = 'tombstoned', deleted_at_ms = ?1
                 WHERE id = ?2 AND worker_id = ?3 AND state = 'deleting'",
                    params![now_ms, version_id.to_string(), worker_id.to_string()],
                )
                .map_err(|_| db_error())?;
            if changed != 1 {
                return Err(version_not_found());
            }
            audit(
                tx,
                account_id,
                "version.delete",
                "version",
                &version_id.to_string(),
                request_id,
                br#"{"state":"tombstoned"}"#,
                now_ms,
            )?;
            Ok(())
        })
    }

    /// Tombstone synchronously when the caller has already proven no pins exist.
    pub fn tombstone_version(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
        version_id: VersionId,
        request_id: RequestId,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        self.begin_version_delete(account_id, worker_id, version_id)?;
        self.finalize_version_delete(account_id, worker_id, version_id, request_id, now_ms)
    }

    /// List crash-recovery candidates left in `deleting`.
    pub fn deleting_versions(&self) -> Result<Vec<VersionId>, PlatformError> {
        self.db.with_read(|conn| {
            let mut stmt = conn
                .prepare("SELECT id FROM worker_versions WHERE state = 'deleting' ORDER BY id")
                .map_err(|_| db_error())?;
            let rows = stmt
                .query_map([], |row| {
                    let id: String = row.get(0)?;
                    VersionId::from_str(&id).map_err(|_| rusqlite::Error::InvalidQuery)
                })
                .map_err(|_| db_error())?;
            collect_rows(rows)
        })
    }

    /// Re-enter a bounded batch of committed `deleting` rows after process restart.
    pub fn recover_deleting_versions(
        &self,
        request_id: RequestId,
        now_ms: i64,
        limit: u32,
    ) -> Result<u32, PlatformError> {
        if limit == 0 || limit > 10_000 {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "delete recovery batch is invalid",
            ));
        }
        self.db.with_immediate(|tx| {
            let candidates = {
                let mut stmt = tx
                    .prepare(
                        "SELECT d.id, d.worker_id, w.account_id
                         FROM worker_versions d JOIN workers w ON w.id = d.worker_id
                         WHERE d.state = 'deleting'
                           AND NOT EXISTS (
                             SELECT 1 FROM version_referrers r WHERE r.version_id = d.id
                           )
                         ORDER BY d.id LIMIT ?1",
                    )
                    .map_err(|_| db_error())?;
                let rows = stmt
                    .query_map([i64::from(limit)], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    })
                    .map_err(|_| db_error())?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(row.map_err(|_| db_error())?);
                }
                out
            };
            let mut recovered = 0_u32;
            for (version, _worker, account) in candidates {
                tx.execute(
                    "DELETE FROM version_services WHERE version_id = ?1",
                    [&version],
                )
                .map_err(|_| db_error())?;
                tx.execute(
                    "DELETE FROM version_cron_declarations WHERE version_id = ?1",
                    [&version],
                )
                .map_err(|_| db_error())?;
                tx.execute(
                    "DELETE FROM version_cron_configs WHERE version_id = ?1",
                    [&version],
                )
                .map_err(|_| db_error())?;
                tx.execute(
                    "DELETE FROM version_queue_consumers WHERE version_id = ?1",
                    [&version],
                )
                .map_err(|_| db_error())?;
                tx.execute(
                    "DELETE FROM queue_producer_bindings WHERE version_id = ?1",
                    [&version],
                )
                .map_err(|_| db_error())?;
                tx.execute(
                    "DELETE FROM workflow_bindings WHERE version_id = ?1",
                    [&version],
                )
                .map_err(|_| db_error())?;
                tx.execute(
                    "DELETE FROM version_bindings WHERE version_id = ?1",
                    [&version],
                )
                .map_err(|_| db_error())?;
                tx.execute("DELETE FROM version_vars WHERE version_id = ?1", [&version])
                    .map_err(|_| db_error())?;
                tx.execute(
                    "DELETE FROM version_secrets WHERE version_id = ?1",
                    [&version],
                )
                .map_err(|_| db_error())?;
                let version_id = VersionId::from_str(&version).map_err(|_| invariant())?;
                crate::assets::delete_version_assets(tx, version_id)?;
                let changed = tx
                    .execute(
                        "UPDATE worker_versions
                         SET state = 'tombstoned', deleted_at_ms = ?1
                         WHERE id = ?2 AND state = 'deleting'",
                        params![now_ms, version],
                    )
                    .map_err(|_| db_error())?;
                if changed == 1 {
                    let account_id = AccountId::from_str(&account).map_err(|_| invariant())?;
                    audit(
                        tx,
                        account_id,
                        "version.delete.recover",
                        "version",
                        &version,
                        request_id,
                        br#"{"state":"tombstoned"}"#,
                        now_ms,
                    )?;
                    recovered = recovered.saturating_add(1);
                }
            }
            Ok(recovered)
        })
    }

    /// Remove expired idempotency rows and their registered version refs atomically.
    pub fn prune_expired_idempotency(&self, now_ms: i64, limit: u32) -> Result<u32, PlatformError> {
        if limit == 0 || limit > 10_000 {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "idempotency prune batch is invalid",
            ));
        }
        self.db.with_immediate(|tx| {
            let expired = {
                let mut stmt = tx
                    .prepare(
                        "SELECT account_id, scope, idempotency_key, version_id
                         FROM control_idempotency
                         WHERE expires_at_ms <= ?1 ORDER BY expires_at_ms LIMIT ?2",
                    )
                    .map_err(|_| db_error())?;
                let rows = stmt
                    .query_map(params![now_ms, i64::from(limit)], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, Option<String>>(3)?,
                        ))
                    })
                    .map_err(|_| db_error())?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(row.map_err(|_| db_error())?);
                }
                out
            };
            let mut pruned = 0_u32;
            for (account, scope, key, version) in expired {
                if let Some(version) = version {
                    let account_id = AccountId::from_str(&account).map_err(|_| invariant())?;
                    tx.execute(
                        "DELETE FROM version_referrers
                         WHERE version_id = ?1 AND kind = 'control_idempotency' AND ref_id = ?2",
                        params![version, idempotency_ref_id(account_id, &scope, &key)],
                    )
                    .map_err(|_| db_error())?;
                }
                pruned = pruned.saturating_add(
                    u32::try_from(
                        tx.execute(
                            "DELETE FROM control_idempotency
                             WHERE account_id = ?1 AND scope = ?2 AND idempotency_key = ?3
                               AND expires_at_ms <= ?4",
                            params![account, scope, key, now_ms],
                        )
                        .map_err(|_| db_error())?,
                    )
                    .unwrap_or(0),
                );
            }
            Ok(pruned)
        })
    }

    /// Select a bounded retention batch without mutating any row.
    pub fn retention_candidates(
        &self,
        now_ms: i64,
        min_age_ms: u64,
        retain_ready: u32,
        retain_rejected: u32,
        limit: u32,
    ) -> Result<Vec<RetentionCandidate>, PlatformError> {
        if retain_ready == 0
            || retain_rejected == 0
            || limit == 0
            || limit > 10_000
            || min_age_ms > i64::MAX as u64
        {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "version retention policy is invalid",
            ));
        }
        let cutoff = now_ms.saturating_sub(i64::try_from(min_age_ms).map_err(|_| invariant())?);
        self.db.with_read(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT d.id, d.worker_id, w.account_id
                     FROM worker_versions d JOIN workers w ON w.id = d.worker_id
                     WHERE d.state IN ('ready', 'rejected')
                       AND d.created_at_ms <= ?1
                       AND NOT EXISTS (
                         SELECT 1 FROM worker_deployments active
                         WHERE active.id=w.active_deployment_id AND active.version_id=d.id
                       )
                       AND NOT EXISTS (
                         SELECT 1 FROM version_referrers r WHERE r.version_id = d.id
                       )
                       AND (
                         (d.state = 'ready' AND (
                           SELECT count(*) FROM worker_versions newer
                           WHERE newer.worker_id = d.worker_id AND newer.state = 'ready'
                             AND newer.version_number > d.version_number
                         ) >= ?2)
                         OR
                         (d.state = 'rejected' AND (
                           SELECT count(*) FROM worker_versions newer
                           WHERE newer.worker_id = d.worker_id AND newer.state = 'rejected'
                             AND newer.version_number > d.version_number
                         ) >= ?3)
                       )
                     ORDER BY d.created_at_ms, d.id LIMIT ?4",
                )
                .map_err(|_| db_error())?;
            let rows = stmt
                .query_map(
                    params![
                        cutoff,
                        i64::from(retain_ready),
                        i64::from(retain_rejected),
                        i64::from(limit)
                    ],
                    |row| {
                        let version: String = row.get(0)?;
                        let worker: String = row.get(1)?;
                        let account: String = row.get(2)?;
                        Ok(RetentionCandidate {
                            account_id: AccountId::from_str(&account)
                                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                            worker_id: WorkerId::from_str(&worker)
                                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                            version_id: VersionId::from_str(&version)
                                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        })
                    },
                )
                .map_err(|_| db_error())?;
            collect_rows(rows)
        })
    }

    /// Return all artifact references still retained by non-tombstoned versions.
    pub fn referenced_artifacts(&self) -> Result<Vec<([u8; 32], u64)>, PlatformError> {
        self.db.with_read(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT DISTINCT r.sha256, r.size
                     FROM version_object_refs r
                     JOIN worker_versions d ON d.id = r.version_id
                     WHERE d.state != 'tombstoned'
                     UNION
                     SELECT DISTINCT o.sha256, o.size
                     FROM version_upload_objects o
                     JOIN version_uploads u ON u.id = o.session_id
                     WHERE o.verified = 1 AND u.status IN ('open', 'finalizing')",
                )
                .map_err(|_| db_error())?;
            let rows = stmt
                .query_map([], |row| {
                    let digest: Vec<u8> = row.get(0)?;
                    let size: i64 = row.get(1)?;
                    Ok((digest, size))
                })
                .map_err(|_| db_error())?;
            let mut out = Vec::new();
            for row in rows {
                let (digest, size) = row.map_err(|_| db_error())?;
                out.push((
                    array32(&digest).map_err(|_| invariant())?,
                    u64::try_from(size).map_err(|_| db_error())?,
                ));
            }
            Ok(out)
        })
    }
}
