//! Bounded completed-history retention and expired transfer collection.

use super::helpers::{ensure_account_database, map_snapshot, map_transfer, to_i64};
use super::*;

impl D1SnapshotRepository<'_> {
    /// Reject a new transfer body before the bounded set of unexpired terminal
    /// download/upload evidence can grow beyond the per-database limit.
    pub fn ensure_transfer_file_capacity(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        max_files: u32,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        if max_files == 0 || now_ms < 0 {
            return Err(invariant());
        }
        self.db.with_read(|conn| {
            ensure_account_database(conn, account_id, resource_id)?;
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM d1_transfer_sessions
                     WHERE resource_id = ?1
                       AND state IN ('complete', 'failed', 'expired')
                       AND token_expires_at_ms > ?2
                       AND file_key IS NOT NULL",
                    params![resource_id.to_string(), now_ms],
                    |row| row.get(0),
                )
                .map_err(|_| invariant())?;
            if count < i64::from(max_files) {
                Ok(())
            } else {
                Err(transfer_capacity())
            }
        })
    }

    /// Remove completed checkpoints outside the bounded window when no
    /// transfer or restore intent still references them.
    pub fn prune_completed_snapshots(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        keep_latest: u32,
        protected_session_versions: [Option<u64>; 2],
    ) -> Result<Vec<D1SnapshotRecord>, PlatformError> {
        if keep_latest == 0 {
            return Err(invariant());
        }
        let protected = protected_session_versions
            .map(|version| version.map(to_i64).transpose())
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;
        self.db.with_immediate(|tx| {
            ensure_account_database(tx, account_id, resource_id)?;
            let count: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM d1_snapshots WHERE resource_id = ?1",
                    [resource_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(|_| invariant())?;
            let remove_count = count.saturating_sub(i64::from(keep_latest)).max(0);
            if remove_count == 0 {
                return Ok(Vec::new());
            }
            let removed = {
                let mut statement = tx
                    .prepare(
                        "SELECT s.resource_id, s.session_version, s.snapshot_key, s.sha256,
                                s.size_bytes, s.created_at_ms
                         FROM d1_snapshots s
                         WHERE s.resource_id = ?1
                           AND (?2 IS NULL OR s.session_version != ?2)
                           AND (?3 IS NULL OR s.session_version != ?3)
                           AND NOT EXISTS (
                             SELECT 1 FROM d1_transfer_sessions t
                             WHERE t.resource_id = s.resource_id
                               AND (t.at_session_version = s.session_version
                                 OR t.result_session_version = s.session_version)
                           )
                           AND NOT EXISTS (
                             SELECT 1 FROM d1_restore_intents r
                             WHERE r.resource_id = s.resource_id
                               AND (r.source_session_version = s.session_version
                                 OR r.previous_session_version = s.session_version)
                           )
                         ORDER BY s.session_version
                         LIMIT ?4",
                    )
                    .map_err(|_| invariant())?;
                statement
                    .query_map(
                        params![
                            resource_id.to_string(),
                            protected[0],
                            protected[1],
                            remove_count
                        ],
                        map_snapshot,
                    )
                    .map_err(|_| invariant())?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| invariant())?
            };
            if i64::try_from(removed.len()).map_err(|_| invariant())? != remove_count {
                return Err(history_capacity());
            }
            for snapshot in &removed {
                if tx
                    .execute(
                        "DELETE FROM d1_snapshots
                         WHERE resource_id = ?1 AND session_version = ?2",
                        params![resource_id.to_string(), to_i64(snapshot.session_version)?],
                    )
                    .map_err(|_| invariant())?
                    != 1
                {
                    return Err(invariant());
                }
            }
            Ok(removed)
        })
    }

    /// Check that one more checkpoint can be admitted without exceeding the
    /// bounded retained-history count or deleting protected/operation evidence.
    pub fn ensure_completed_snapshot_capacity(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        max_snapshots: u32,
        protected_session_versions: [Option<u64>; 2],
    ) -> Result<(), PlatformError> {
        if max_snapshots == 0 {
            return Err(invariant());
        }
        let protected = protected_session_versions
            .map(|version| version.map(to_i64).transpose())
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;
        self.db.with_read(|conn| {
            ensure_account_database(conn, account_id, resource_id)?;
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM d1_snapshots WHERE resource_id = ?1",
                    [resource_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(|_| invariant())?;
            if count < i64::from(max_snapshots) {
                return Ok(());
            }
            let prunable: bool = conn
                .query_row(
                    "SELECT EXISTS(
                       SELECT 1 FROM d1_snapshots s
                       WHERE s.resource_id = ?1
                         AND (?2 IS NULL OR s.session_version != ?2)
                         AND (?3 IS NULL OR s.session_version != ?3)
                         AND NOT EXISTS (
                           SELECT 1 FROM d1_transfer_sessions t
                           WHERE t.resource_id = s.resource_id
                             AND (t.at_session_version = s.session_version
                               OR t.result_session_version = s.session_version)
                         )
                         AND NOT EXISTS (
                           SELECT 1 FROM d1_restore_intents r
                           WHERE r.resource_id = s.resource_id
                             AND (r.source_session_version = s.session_version
                               OR r.previous_session_version = s.session_version)
                         )
                     )",
                    params![resource_id.to_string(), protected[0], protected[1]],
                    |row| row.get(0),
                )
                .map_err(|_| invariant())?;
            if prunable {
                Ok(())
            } else {
                Err(history_capacity())
            }
        })
    }

    /// Delete terminal transfer authority after its URL capability expires so
    /// completed operation evidence cannot pin checkpoint capacity forever.
    pub fn prune_expired_terminal_transfers(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        now_ms: i64,
    ) -> Result<Vec<D1TransferRecord>, PlatformError> {
        if now_ms < 0 {
            return Err(invariant());
        }
        self.db.with_immediate(|tx| {
            ensure_account_database(tx, account_id, resource_id)?;
            let removed = {
                let mut statement = tx
                    .prepare(
                        "SELECT s.id, s.resource_id, s.kind, s.state, s.at_session_version,
                                s.result_session_version, s.filename, s.file_key, s.etag_md5,
                                s.sha256, s.size_bytes, s.token_fingerprint, s.token_action,
                                s.token_expires_at_ms, s.num_queries, s.duration_ms, s.rows_read,
                                s.rows_written, s.result_size_after, s.created_at_ms,
                                s.updated_at_ms, s.completed_at_ms, s.error_code
                         FROM d1_transfer_sessions s
                         WHERE s.resource_id = ?1
                           AND s.state IN ('complete', 'failed', 'expired')
                           AND s.token_expires_at_ms <= ?2
                         ORDER BY s.token_expires_at_ms, s.id",
                    )
                    .map_err(|_| invariant())?;
                statement
                    .query_map(params![resource_id.to_string(), now_ms], map_transfer)
                    .map_err(|_| invariant())?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| invariant())?
            };
            for transfer in &removed {
                if tx
                    .execute(
                        "DELETE FROM d1_transfer_sessions
                         WHERE id = ?1 AND resource_id = ?2
                           AND state IN ('complete', 'failed', 'expired')
                           AND token_expires_at_ms <= ?3",
                        params![transfer.id, resource_id.to_string(), now_ms],
                    )
                    .map_err(|_| invariant())?
                    != 1
                {
                    return Err(invariant());
                }
            }
            Ok(removed)
        })
    }
}

fn transfer_capacity() -> PlatformError {
    PlatformError::new(
        ErrorCode::D1DatabaseFull,
        "D1 transfer file retention reached its per-database limit",
    )
}
