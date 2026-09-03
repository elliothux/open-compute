//! Internal database mapping and state-transition helpers.

use super::*;

impl D1SnapshotRepository<'_> {
    /// Find a prior transfer by its stable account/database/kind/filename tuple.
    pub fn transfer_by_filename(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        kind: D1TransferKind,
        filename: &str,
    ) -> Result<Option<D1TransferRecord>, PlatformError> {
        self.db.with_read(|conn| {
            ensure_account_database(conn, account_id, resource_id)?;
            let id: Option<String> = conn
                .query_row(
                    "SELECT id FROM d1_transfer_sessions
                     WHERE resource_id = ?1 AND kind = ?2 AND filename = ?3",
                    params![resource_id.to_string(), kind.as_str(), filename],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|_| invariant())?;
            id.map(|id| read_transfer(conn, account_id, &id))
                .transpose()
        })
    }
}

pub(super) fn transition_file(
    db: &ControlDb,
    account_id: AccountId,
    session_id: &str,
    kind: D1TransferKind,
    from: D1TransferState,
    to: D1TransferState,
    file_key: &str,
    etag_md5: Option<&[u8; 16]>,
    sha256: &[u8; 32],
    size_bytes: u64,
    now_ms: i64,
) -> Result<D1TransferRecord, PlatformError> {
    if size_bytes == 0 {
        return Err(invariant());
    }
    db.with_immediate(|tx| {
        let current = read_transfer(tx, account_id, session_id)?;
        if current.kind != kind || now_ms < current.updated_at_ms {
            return Err(invariant());
        }
        if current.state == to {
            if current.file_key.as_deref() == Some(file_key)
                && current.etag_md5.as_ref() == etag_md5
                && current.sha256 == Some(*sha256)
                && current.size_bytes == Some(size_bytes)
                && current.completed_at_ms == (to == D1TransferState::Complete).then_some(now_ms)
            {
                return Ok(current);
            }
            return Err(idempotency_conflict());
        }
        if current.state != from
            || now_ms >= current.token_expires_at_ms
            || current.etag_md5.as_ref() != etag_md5
        {
            return Err(invariant());
        }
        tx.execute(
            "UPDATE d1_transfer_sessions SET state = ?1, file_key = ?2, sha256 = ?3,
                 size_bytes = ?4, updated_at_ms = ?5,
                 completed_at_ms = CASE WHEN ?1 = 'complete' THEN ?5 ELSE NULL END
             WHERE id = ?6 AND state = ?7",
            params![
                to.as_str(),
                file_key,
                sha256.as_slice(),
                to_i64(size_bytes)?,
                now_ms,
                session_id,
                from.as_str()
            ],
        )
        .map_err(|_| invariant())?;
        read_transfer(tx, account_id, session_id)
    })
}

pub(super) fn finish_transfer(
    db: &ControlDb,
    account_id: AccountId,
    session_id: &str,
    failure: Option<ErrorCode>,
    now_ms: i64,
) -> Result<D1TransferRecord, PlatformError> {
    db.with_immediate(|tx| {
        let current = read_transfer(tx, account_id, session_id)?;
        let target = if failure.is_some() {
            D1TransferState::Failed
        } else {
            D1TransferState::Expired
        };
        if current.state == target {
            if current.completed_at_ms == Some(now_ms)
                && current.error_code.as_deref() == failure.map(ErrorCode::as_str)
            {
                return Ok(current);
            }
            return Err(idempotency_conflict());
        }
        if matches!(
            current.state,
            D1TransferState::Complete | D1TransferState::Failed | D1TransferState::Expired
        ) || now_ms < current.updated_at_ms
            || failure.is_none() && now_ms < current.token_expires_at_ms
        {
            return Err(invariant());
        }
        tx.execute(
            "UPDATE d1_transfer_sessions
             SET state = ?1, updated_at_ms = ?2, completed_at_ms = ?2, error_code = ?3
             WHERE id = ?4",
            params![
                target.as_str(),
                now_ms,
                failure.map(ErrorCode::as_str),
                session_id
            ],
        )
        .map_err(|_| invariant())?;
        read_transfer(tx, account_id, session_id)
    })
}

pub(super) fn ensure_account_database(
    conn: &rusqlite::Connection,
    account_id: AccountId,
    resource_id: ResourceId,
) -> Result<(), PlatformError> {
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM d1_databases d JOIN resources r
             ON r.id = d.resource_id WHERE d.resource_id = ?1 AND r.account_id = ?2
             AND r.kind = 'd1_database' AND r.state != 'tombstoned')",
            params![resource_id.to_string(), account_id.to_string()],
            |row| row.get(0),
        )
        .map_err(|_| invariant())?;
    if exists { Ok(()) } else { Err(not_found()) }
}

pub(super) fn read_snapshot(
    conn: &rusqlite::Connection,
    resource_id: ResourceId,
    session_version: u64,
) -> Result<D1SnapshotRecord, PlatformError> {
    read_snapshot_optional(conn, resource_id, session_version)?.ok_or_else(not_found)
}

pub(super) fn read_snapshot_optional(
    conn: &rusqlite::Connection,
    resource_id: ResourceId,
    session_version: u64,
) -> Result<Option<D1SnapshotRecord>, PlatformError> {
    conn.query_row(
        "SELECT resource_id, session_version, snapshot_key, sha256, size_bytes, created_at_ms
         FROM d1_snapshots WHERE resource_id = ?1 AND session_version = ?2",
        params![resource_id.to_string(), to_i64(session_version)?],
        map_snapshot,
    )
    .optional()
    .map_err(|_| invariant())
}

pub(super) fn read_latest_snapshot(
    conn: &rusqlite::Connection,
    resource_id: ResourceId,
) -> Result<Option<D1SnapshotRecord>, PlatformError> {
    conn.query_row(
        "SELECT resource_id, session_version, snapshot_key, sha256, size_bytes, created_at_ms
         FROM d1_snapshots WHERE resource_id = ?1 ORDER BY session_version DESC LIMIT 1",
        [resource_id.to_string()],
        map_snapshot,
    )
    .optional()
    .map_err(|_| invariant())
}

pub(super) fn map_snapshot(row: &rusqlite::Row<'_>) -> rusqlite::Result<D1SnapshotRecord> {
    let resource: String = row.get(0)?;
    let version: i64 = row.get(1)?;
    let digest: Vec<u8> = row.get(3)?;
    let size: i64 = row.get(4)?;
    Ok(D1SnapshotRecord {
        resource_id: ResourceId::from_str(&resource).map_err(|_| rusqlite::Error::InvalidQuery)?,
        session_version: u64::try_from(version).map_err(|_| rusqlite::Error::InvalidQuery)?,
        snapshot_key: row.get(2)?,
        sha256: digest
            .try_into()
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        size_bytes: u64::try_from(size).map_err(|_| rusqlite::Error::InvalidQuery)?,
        created_at_ms: row.get(5)?,
    })
}

pub(super) fn read_transfer(
    conn: &rusqlite::Connection,
    account_id: AccountId,
    session_id: &str,
) -> Result<D1TransferRecord, PlatformError> {
    conn.query_row(
        "SELECT s.id, s.resource_id, s.kind, s.state, s.at_session_version,
                s.result_session_version, s.filename, s.file_key, s.etag_md5, s.sha256,
                s.size_bytes, s.token_fingerprint, s.token_action, s.token_expires_at_ms,
                s.num_queries, s.created_at_ms, s.updated_at_ms, s.completed_at_ms, s.error_code
         FROM d1_transfer_sessions s JOIN resources r ON r.id = s.resource_id
         WHERE s.id = ?1 AND r.account_id = ?2",
        params![session_id, account_id.to_string()],
        map_transfer,
    )
    .optional()
    .map_err(|_| invariant())?
    .ok_or_else(not_found)
}

pub(super) fn read_active_transfer(
    conn: &rusqlite::Connection,
    resource_id: ResourceId,
) -> Result<Option<D1TransferRecord>, PlatformError> {
    conn.query_row(
        "SELECT id, resource_id, kind, state, at_session_version,
                result_session_version, filename, file_key, etag_md5, sha256,
                size_bytes, token_fingerprint, token_action, token_expires_at_ms,
                num_queries, created_at_ms, updated_at_ms, completed_at_ms, error_code
         FROM d1_transfer_sessions WHERE resource_id = ?1
           AND state IN ('preparing', 'uploading', 'uploaded', 'ingesting')",
        [resource_id.to_string()],
        map_transfer,
    )
    .optional()
    .map_err(|_| invariant())
}

pub(super) fn map_transfer(row: &rusqlite::Row<'_>) -> rusqlite::Result<D1TransferRecord> {
    let resource: String = row.get(1)?;
    let kind: String = row.get(2)?;
    let state: String = row.get(3)?;
    let at_version: i64 = row.get(4)?;
    let result_version: Option<i64> = row.get(5)?;
    let etag: Option<Vec<u8>> = row.get(8)?;
    let sha256: Option<Vec<u8>> = row.get(9)?;
    let size: Option<i64> = row.get(10)?;
    let token_fingerprint: Vec<u8> = row.get(11)?;
    let action: String = row.get(12)?;
    let num_queries: Option<i64> = row.get(14)?;
    Ok(D1TransferRecord {
        id: row.get(0)?,
        resource_id: ResourceId::from_str(&resource).map_err(|_| rusqlite::Error::InvalidQuery)?,
        kind: D1TransferKind::from_str(&kind).map_err(|_| rusqlite::Error::InvalidQuery)?,
        state: D1TransferState::from_str(&state).map_err(|_| rusqlite::Error::InvalidQuery)?,
        at_session_version: u64::try_from(at_version).map_err(|_| rusqlite::Error::InvalidQuery)?,
        result_session_version: result_version
            .map(|value| u64::try_from(value).map_err(|_| rusqlite::Error::InvalidQuery))
            .transpose()?,
        filename: row.get(6)?,
        file_key: row.get(7)?,
        etag_md5: etag
            .map(|value| value.try_into().map_err(|_| rusqlite::Error::InvalidQuery))
            .transpose()?,
        sha256: sha256
            .map(|value| value.try_into().map_err(|_| rusqlite::Error::InvalidQuery))
            .transpose()?,
        size_bytes: size
            .map(|value| u64::try_from(value).map_err(|_| rusqlite::Error::InvalidQuery))
            .transpose()?,
        token_fingerprint: token_fingerprint
            .try_into()
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        token_action: D1TransferAction::from_str(&action)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        token_expires_at_ms: row.get(13)?,
        num_queries: num_queries
            .map(|value| u64::try_from(value).map_err(|_| rusqlite::Error::InvalidQuery))
            .transpose()?,
        created_at_ms: row.get(15)?,
        updated_at_ms: row.get(16)?,
        completed_at_ms: row.get(17)?,
        error_code: row.get(18)?,
    })
}

pub(super) fn read_restore(
    conn: &rusqlite::Connection,
    resource_id: ResourceId,
) -> Result<D1RestoreIntent, PlatformError> {
    read_restore_optional(conn, resource_id)?.ok_or_else(not_found)
}

pub(super) fn read_restore_optional(
    conn: &rusqlite::Connection,
    resource_id: ResourceId,
) -> Result<Option<D1RestoreIntent>, PlatformError> {
    conn.query_row(
        "SELECT id, resource_id, source_session_version, previous_session_version,
                result_session_version, request_fingerprint, created_at_ms
         FROM d1_restore_intents WHERE resource_id = ?1",
        [resource_id.to_string()],
        |row| {
            let resource: String = row.get(1)?;
            let source: i64 = row.get(2)?;
            let previous: i64 = row.get(3)?;
            let result: i64 = row.get(4)?;
            let fingerprint: Vec<u8> = row.get(5)?;
            Ok(D1RestoreIntent {
                id: row.get(0)?,
                resource_id: ResourceId::from_str(&resource)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                source_session_version: u64::try_from(source)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                previous_session_version: u64::try_from(previous)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                result_session_version: u64::try_from(result)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                request_fingerprint: fingerprint
                    .try_into()
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                created_at_ms: row.get(6)?,
            })
        },
    )
    .optional()
    .map_err(|_| invariant())
}

pub(super) fn validate_uuid(value: &str) -> Result<(), PlatformError> {
    if uuid::Uuid::parse_str(value)
        .ok()
        .is_none_or(|id| id.hyphenated().to_string() != value)
    {
        return Err(invariant());
    }
    Ok(())
}

pub(super) fn validate_filename(value: &str) -> Result<(), PlatformError> {
    if value.is_empty()
        || value.len() > 255
        || value.contains('/')
        || value.contains('\0')
        || value == "."
        || value == ".."
    {
        return Err(invariant());
    }
    Ok(())
}

pub(super) fn validate_key(value: &str) -> Result<(), PlatformError> {
    if value.is_empty() || value.len() > 512 || value.contains("..") || value.contains('\0') {
        return Err(invariant());
    }
    Ok(())
}

pub(super) fn to_i64(value: u64) -> Result<i64, PlatformError> {
    i64::try_from(value).map_err(|_| invariant())
}

pub(super) fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceInvariantViolation,
        "D1 snapshot authority invariant failed",
    )
}

pub(super) fn not_found() -> PlatformError {
    PlatformError::new(ErrorCode::ResourceNotFound, "D1 history was not found")
}

pub(super) fn idempotency_conflict() -> PlatformError {
    PlatformError::new(
        ErrorCode::IdempotencyConflict,
        "D1 operation replay does not match durable authority",
    )
}

pub(super) fn busy() -> PlatformError {
    PlatformError::new(
        ErrorCode::D1Overloaded,
        "D1 database already has an active fenced operation",
    )
}
