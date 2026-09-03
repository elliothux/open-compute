//! Completed D1 snapshot history and restart-safe transfer/restore intents.

use crate::ControlDb;
use open_compute_core::{AccountId, ErrorCode, PlatformError, ResourceId};
use rusqlite::{OptionalExtension, params};
use std::str::FromStr;

/// One completed, independently recoverable D1 state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct D1SnapshotRecord {
    /// Database identity.
    pub resource_id: ResourceId,
    /// Durable monotonic database session version.
    pub session_version: u64,
    /// Private, canonical snapshot locator.
    pub snapshot_key: String,
    /// Verified snapshot digest.
    pub sha256: [u8; 32],
    /// Verified snapshot length.
    pub size_bytes: u64,
    /// Snapshot commit timestamp.
    pub created_at_ms: i64,
}

/// D1 SQL transfer direction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum D1TransferKind {
    /// Generate a SQL dump for download.
    Export,
    /// Upload and ingest a SQL dump.
    Import,
}

impl D1TransferKind {
    /// Stable database token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Export => "export",
            Self::Import => "import",
        }
    }
}

impl FromStr for D1TransferKind {
    type Err = PlatformError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "export" => Ok(Self::Export),
            "import" => Ok(Self::Import),
            _ => Err(invariant()),
        }
    }
}

/// Capability action authorized by a transfer URL.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum D1TransferAction {
    /// Upload one SQL input.
    Upload,
    /// Download one SQL export.
    Download,
}

impl D1TransferAction {
    /// Stable database token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Upload => "upload",
            Self::Download => "download",
        }
    }
}

impl FromStr for D1TransferAction {
    type Err = PlatformError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "upload" => Ok(Self::Upload),
            "download" => Ok(Self::Download),
            _ => Err(invariant()),
        }
    }
}

/// Durable SQL transfer state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum D1TransferState {
    /// Export generation has been reserved.
    Preparing,
    /// Import is waiting for its upload.
    Uploading,
    /// Import bytes are verified and durable.
    Uploaded,
    /// Verified SQL is being applied under the database fence.
    Ingesting,
    /// Export or import completed successfully.
    Complete,
    /// Transfer failed with a sanitized stable code.
    Failed,
    /// Transfer capability expired before completion.
    Expired,
}

impl D1TransferState {
    /// Stable database token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Preparing => "preparing",
            Self::Uploading => "uploading",
            Self::Uploaded => "uploaded",
            Self::Ingesting => "ingesting",
            Self::Complete => "complete",
            Self::Failed => "failed",
            Self::Expired => "expired",
        }
    }
}

impl FromStr for D1TransferState {
    type Err = PlatformError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "preparing" => Ok(Self::Preparing),
            "uploading" => Ok(Self::Uploading),
            "uploaded" => Ok(Self::Uploaded),
            "ingesting" => Ok(Self::Ingesting),
            "complete" => Ok(Self::Complete),
            "failed" => Ok(Self::Failed),
            "expired" => Ok(Self::Expired),
            _ => Err(invariant()),
        }
    }
}

/// Immutable transfer reservation input.
#[derive(Clone, Copy, Debug)]
pub struct NewD1Transfer<'a> {
    /// Canonical host-generated session UUID.
    pub id: &'a str,
    /// Owning account.
    pub account_id: AccountId,
    /// Target database.
    pub resource_id: ResourceId,
    /// Export or import.
    pub kind: D1TransferKind,
    /// Completed snapshot anchoring this operation.
    pub at_session_version: u64,
    /// Stable SQL filename.
    pub filename: &'a str,
    /// Required import MD5 ETag; absent for exports.
    pub etag_md5: Option<&'a [u8; 16]>,
    /// Keyed fingerprint of the scoped URL capability.
    pub token_fingerprint: &'a [u8; 32],
    /// Upload for imports, download for exports.
    pub token_action: D1TransferAction,
    /// Exclusive capability expiry.
    pub token_expires_at_ms: i64,
    /// Reservation timestamp.
    pub now_ms: i64,
}

/// One restart-safe SQL transfer session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct D1TransferRecord {
    /// Session UUID.
    pub id: String,
    /// Target database.
    pub resource_id: ResourceId,
    /// Transfer direction.
    pub kind: D1TransferKind,
    /// Durable lifecycle.
    pub state: D1TransferState,
    /// Completed snapshot anchoring the operation.
    pub at_session_version: u64,
    /// Completed snapshot produced by a successful import.
    pub result_session_version: Option<u64>,
    /// Stable external SQL filename.
    pub filename: String,
    /// Private staged SQL locator.
    pub file_key: Option<String>,
    /// Import MD5 ETag.
    pub etag_md5: Option<[u8; 16]>,
    /// Verified SQL SHA-256.
    pub sha256: Option<[u8; 32]>,
    /// Verified SQL length.
    pub size_bytes: Option<u64>,
    /// Stored URL capability fingerprint, never the capability.
    pub(crate) token_fingerprint: [u8; 32],
    /// Capability action.
    pub token_action: D1TransferAction,
    /// Exclusive capability expiry.
    pub token_expires_at_ms: i64,
    /// Statements applied by a completed import.
    pub num_queries: Option<u64>,
    /// Reservation timestamp.
    pub created_at_ms: i64,
    /// Last transition timestamp.
    pub updated_at_ms: i64,
    /// Terminal transition timestamp.
    pub completed_at_ms: Option<i64>,
    /// Stable sanitized failure code.
    pub error_code: Option<String>,
}

/// One identity-preserving time-travel restore fence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct D1RestoreIntent {
    /// Host-generated operation UUID.
    pub id: String,
    /// Existing database identity retained by the restore.
    pub resource_id: ResourceId,
    /// Completed snapshot selected as the restore source.
    pub source_session_version: u64,
    /// Completed head observed before restore.
    pub previous_session_version: u64,
    /// Monotonic session version assigned to the restored state.
    pub result_session_version: u64,
    /// Request fingerprint used for exact replay checks.
    pub(crate) request_fingerprint: [u8; 32],
    /// Intent commit timestamp.
    pub created_at_ms: i64,
}

/// Repository for D1 completed history and long-running operation intents.
#[derive(Clone, Copy, Debug)]
pub struct D1SnapshotRepository<'a> {
    db: &'a ControlDb,
}

impl<'a> D1SnapshotRepository<'a> {
    /// Bind the control database.
    #[must_use]
    pub const fn new(db: &'a ControlDb) -> Self {
        Self { db }
    }

    /// Commit one fully durable snapshot, enforcing gap-free session history.
    #[allow(clippy::too_many_arguments)]
    pub fn record_completed_snapshot(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        session_version: u64,
        snapshot_key: &str,
        sha256: &[u8; 32],
        size_bytes: u64,
        now_ms: i64,
    ) -> Result<D1SnapshotRecord, PlatformError> {
        validate_key(snapshot_key)?;
        let version = to_i64(session_version)?;
        let size = to_i64(size_bytes)?;
        if size_bytes == 0 || now_ms < 0 {
            return Err(invariant());
        }
        self.db.with_immediate(|tx| {
            ensure_account_database(tx, account_id, resource_id)?;
            if let Some(existing) = read_snapshot_optional(tx, resource_id, session_version)? {
                if existing.snapshot_key == snapshot_key
                    && existing.sha256 == *sha256
                    && existing.size_bytes == size_bytes
                    && now_ms >= existing.created_at_ms
                {
                    return Ok(existing);
                }
                return Err(idempotency_conflict());
            }
            let latest: Option<(i64, i64)> = tx
                .query_row(
                    "SELECT session_version, created_at_ms FROM d1_snapshots
                     WHERE resource_id = ?1 ORDER BY session_version DESC LIMIT 1",
                    [resource_id.to_string()],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|_| invariant())?;
            match latest {
                None if session_version != 0 => return Err(invariant()),
                Some((latest_version, latest_at))
                    if u64::try_from(latest_version)
                        .ok()
                        .and_then(|value| value.checked_add(1))
                        != Some(session_version)
                        || now_ms < latest_at =>
                {
                    return Err(invariant());
                }
                _ => {}
            }
            tx.execute(
                "INSERT INTO d1_snapshots
                 (resource_id, session_version, snapshot_key, sha256, size_bytes, created_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    resource_id.to_string(),
                    version,
                    snapshot_key,
                    sha256.as_slice(),
                    size,
                    now_ms
                ],
            )
            .map_err(|_| invariant())?;
            read_snapshot(tx, resource_id, session_version)
        })
    }

    /// Read the latest completed snapshot for one account-scoped database.
    pub fn latest_snapshot(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
    ) -> Result<Option<D1SnapshotRecord>, PlatformError> {
        self.db.with_read(|conn| {
            ensure_account_database(conn, account_id, resource_id)?;
            conn.query_row(
                "SELECT resource_id, session_version, snapshot_key, sha256, size_bytes,
                        created_at_ms FROM d1_snapshots WHERE resource_id = ?1
                 ORDER BY session_version DESC LIMIT 1",
                [resource_id.to_string()],
                map_snapshot,
            )
            .optional()
            .map_err(|_| invariant())
        })
    }

    /// Resolve the nearest completed snapshot at or before a timestamp.
    pub fn snapshot_at_or_before(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        timestamp_ms: i64,
    ) -> Result<Option<D1SnapshotRecord>, PlatformError> {
        if timestamp_ms < 0 {
            return Err(invariant());
        }
        self.db.with_read(|conn| {
            ensure_account_database(conn, account_id, resource_id)?;
            conn.query_row(
                "SELECT resource_id, session_version, snapshot_key, sha256, size_bytes,
                        created_at_ms FROM d1_snapshots
                 WHERE resource_id = ?1 AND created_at_ms <= ?2
                 ORDER BY created_at_ms DESC, session_version DESC LIMIT 1",
                params![resource_id.to_string(), timestamp_ms],
                map_snapshot,
            )
            .optional()
            .map_err(|_| invariant())
        })
    }

    /// Read one exact completed snapshot.
    pub fn snapshot(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        session_version: u64,
    ) -> Result<D1SnapshotRecord, PlatformError> {
        self.db.with_read(|conn| {
            ensure_account_database(conn, account_id, resource_id)?;
            read_snapshot(conn, resource_id, session_version)
        })
    }

    /// Reserve an export or import session idempotently.
    pub fn create_transfer(
        &self,
        input: &NewD1Transfer<'_>,
    ) -> Result<D1TransferRecord, PlatformError> {
        validate_uuid(input.id)?;
        validate_filename(input.filename)?;
        let initial = match input.kind {
            D1TransferKind::Export if input.token_action == D1TransferAction::Download => {
                D1TransferState::Preparing
            }
            D1TransferKind::Import
                if input.token_action == D1TransferAction::Upload && input.etag_md5.is_some() =>
            {
                D1TransferState::Uploading
            }
            _ => return Err(invariant()),
        };
        if input.now_ms < 0 || input.token_expires_at_ms <= input.now_ms {
            return Err(invariant());
        }
        self.db.with_immediate(|tx| {
            ensure_account_database(tx, input.account_id, input.resource_id)?;
            let _ = read_snapshot(tx, input.resource_id, input.at_session_version)?;
            let existing = tx
                .query_row(
                    "SELECT id, resource_id, kind, state, at_session_version,
                            result_session_version, filename, file_key, etag_md5, sha256,
                            size_bytes, token_fingerprint, token_action, token_expires_at_ms,
                            num_queries, created_at_ms, updated_at_ms, completed_at_ms, error_code
                     FROM d1_transfer_sessions
                     WHERE resource_id = ?1 AND kind = ?2 AND filename = ?3",
                    params![
                        input.resource_id.to_string(),
                        input.kind.as_str(),
                        input.filename
                    ],
                    map_transfer,
                )
                .optional()
                .map_err(|_| invariant())?;
            if let Some(existing) = existing {
                if existing.id == input.id
                    && existing.at_session_version == input.at_session_version
                    && existing.etag_md5.as_ref() == input.etag_md5
                    && existing.token_fingerprint == *input.token_fingerprint
                    && existing.token_action == input.token_action
                    && existing.token_expires_at_ms == input.token_expires_at_ms
                    && existing.created_at_ms == input.now_ms
                {
                    return Ok(existing);
                }
                return Err(idempotency_conflict());
            }
            if read_restore_optional(tx, input.resource_id)?.is_some()
                || read_active_transfer(tx, input.resource_id)?.is_some()
            {
                return Err(busy());
            }
            tx.execute(
                "INSERT INTO d1_transfer_sessions
                 (id, resource_id, kind, state, at_session_version, result_session_version,
                  filename, file_key, etag_md5, sha256, size_bytes, token_fingerprint,
                  token_action, token_expires_at_ms, num_queries, created_at_ms,
                  updated_at_ms, completed_at_ms, error_code)
                 VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, NULL, ?7, NULL, NULL, ?8,
                         ?9, ?10, NULL, ?11, ?11, NULL, NULL)",
                params![
                    input.id,
                    input.resource_id.to_string(),
                    input.kind.as_str(),
                    initial.as_str(),
                    to_i64(input.at_session_version)?,
                    input.filename,
                    input.etag_md5.map(<[u8; 16]>::as_slice),
                    input.token_fingerprint.as_slice(),
                    input.token_action.as_str(),
                    input.token_expires_at_ms,
                    input.now_ms
                ],
            )
            .map_err(|_| invariant())?;
            read_transfer(tx, input.account_id, input.id)
        })
    }

    /// Mark an import upload verified and durable.
    #[allow(clippy::too_many_arguments)]
    pub fn complete_upload(
        &self,
        account_id: AccountId,
        session_id: &str,
        file_key: &str,
        etag_md5: &[u8; 16],
        sha256: &[u8; 32],
        size_bytes: u64,
        now_ms: i64,
    ) -> Result<D1TransferRecord, PlatformError> {
        validate_key(file_key)?;
        transition_file(
            self.db,
            account_id,
            session_id,
            D1TransferKind::Import,
            D1TransferState::Uploading,
            D1TransferState::Uploaded,
            file_key,
            Some(etag_md5),
            sha256,
            size_bytes,
            now_ms,
        )
    }

    /// Mark generated export SQL verified and ready for download.
    pub fn complete_export(
        &self,
        account_id: AccountId,
        session_id: &str,
        file_key: &str,
        sha256: &[u8; 32],
        size_bytes: u64,
        now_ms: i64,
    ) -> Result<D1TransferRecord, PlatformError> {
        validate_key(file_key)?;
        transition_file(
            self.db,
            account_id,
            session_id,
            D1TransferKind::Export,
            D1TransferState::Preparing,
            D1TransferState::Complete,
            file_key,
            None,
            sha256,
            size_bytes,
            now_ms,
        )
    }

    /// Fence an uploaded import and persist its statement count before SQL commit.
    pub fn begin_ingest(
        &self,
        account_id: AccountId,
        session_id: &str,
        num_queries: u64,
        now_ms: i64,
    ) -> Result<D1TransferRecord, PlatformError> {
        self.db.with_immediate(|tx| {
            let current = read_transfer(tx, account_id, session_id)?;
            if current.kind != D1TransferKind::Import || now_ms < current.updated_at_ms {
                return Err(invariant());
            }
            if current.state == D1TransferState::Ingesting {
                return if current.num_queries == Some(num_queries) {
                    Ok(current)
                } else {
                    Err(idempotency_conflict())
                };
            }
            if current.state != D1TransferState::Uploaded || now_ms >= current.token_expires_at_ms {
                return Err(invariant());
            }
            tx.execute(
                "UPDATE d1_transfer_sessions
                 SET state = 'ingesting', num_queries = ?1, updated_at_ms = ?2
                 WHERE id = ?3 AND state = 'uploaded'",
                params![to_i64(num_queries)?, now_ms, session_id],
            )
            .map_err(|_| invariant())?;
            read_transfer(tx, account_id, session_id)
        })
    }

    /// Complete an import only after its resulting snapshot is durable.
    pub fn complete_import(
        &self,
        account_id: AccountId,
        session_id: &str,
        result_session_version: u64,
        num_queries: u64,
        now_ms: i64,
    ) -> Result<D1TransferRecord, PlatformError> {
        self.db.with_immediate(|tx| {
            let current = read_transfer(tx, account_id, session_id)?;
            if current.kind != D1TransferKind::Import || now_ms < current.updated_at_ms {
                return Err(invariant());
            }
            if current.state == D1TransferState::Complete {
                if current.result_session_version == Some(result_session_version)
                    && current.num_queries == Some(num_queries)
                {
                    return Ok(current);
                }
                return Err(idempotency_conflict());
            }
            if current.state != D1TransferState::Ingesting
                || current.num_queries != Some(num_queries)
                || result_session_version
                    != current
                        .at_session_version
                        .checked_add(1)
                        .ok_or_else(invariant)?
            {
                return Err(invariant());
            }
            let _ = read_snapshot(tx, current.resource_id, result_session_version)?;
            tx.execute(
                "UPDATE d1_transfer_sessions
                 SET state = 'complete', result_session_version = ?1,
                     updated_at_ms = ?2, completed_at_ms = ?2
                 WHERE id = ?3 AND state = 'ingesting'",
                params![to_i64(result_session_version)?, now_ms, session_id],
            )
            .map_err(|_| invariant())?;
            read_transfer(tx, account_id, session_id)
        })
    }

    /// Read one account-scoped transfer session after restart.
    pub fn transfer(
        &self,
        account_id: AccountId,
        session_id: &str,
    ) -> Result<D1TransferRecord, PlatformError> {
        self.db
            .with_read(|conn| read_transfer(conn, account_id, session_id))
    }

    /// Read the one active export/import fence, if any.
    pub fn active_transfer(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
    ) -> Result<Option<D1TransferRecord>, PlatformError> {
        self.db.with_read(|conn| {
            ensure_account_database(conn, account_id, resource_id)?;
            read_active_transfer(conn, resource_id)
        })
    }

    /// Verify a URL capability by keyed fingerprint, action, owner, and expiry.
    pub fn authorize_transfer_token(
        &self,
        account_id: AccountId,
        resource_id: ResourceId,
        session_id: &str,
        action: D1TransferAction,
        token_fingerprint: &[u8; 32],
        now_ms: i64,
    ) -> Result<D1TransferRecord, PlatformError> {
        let record = self.transfer(account_id, session_id)?;
        if record.resource_id != resource_id
            || record.token_action != action
            || record.token_fingerprint != *token_fingerprint
            || now_ms < record.created_at_ms
            || now_ms >= record.token_expires_at_ms
            || !matches!(
                (record.kind, record.state, action),
                (
                    D1TransferKind::Import,
                    D1TransferState::Uploading | D1TransferState::Uploaded,
                    D1TransferAction::Upload
                ) | (
                    D1TransferKind::Export,
                    D1TransferState::Complete,
                    D1TransferAction::Download
                )
            )
        {
            return Err(not_found());
        }
        Ok(record)
    }

    /// Fail an active transfer and retain only sanitized terminal authority.
    pub fn fail_transfer(
        &self,
        account_id: AccountId,
        session_id: &str,
        code: ErrorCode,
        now_ms: i64,
    ) -> Result<D1TransferRecord, PlatformError> {
        finish_transfer(self.db, account_id, session_id, Some(code), now_ms)
    }

    /// Expire an active transfer after its capability deadline.
    pub fn expire_transfer(
        &self,
        account_id: AccountId,
        session_id: &str,
        now_ms: i64,
    ) -> Result<D1TransferRecord, PlatformError> {
        finish_transfer(self.db, account_id, session_id, None, now_ms)
    }

    /// Reserve the one identity-preserving restore allowed for a database.
    #[allow(clippy::too_many_arguments)]
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

mod helpers;
use helpers::*;
#[cfg(test)]
#[path = "history_tests.rs"]
mod tests;
