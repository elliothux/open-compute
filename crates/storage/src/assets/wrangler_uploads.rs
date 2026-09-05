//! Durable fixed-Wrangler Static Assets upload sessions.

use crate::ControlDb;
use open_compute_core::{AccountId, ErrorCode, PlatformError};
use rusqlite::{OptionalExtension, params};
use std::collections::BTreeMap;
use std::str::FromStr;

/// One manifest entry declared by the upload-session request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewAssetUploadEntry {
    /// Canonical URL path.
    pub path: String,
    /// Fixed Wrangler BLAKE3-derived 128-bit content token.
    pub wrangler_hash: String,
    /// Declared original byte length.
    pub size: u64,
}

/// One durable manifest entry and its verified content-addressed object.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssetUploadEntry {
    /// Canonical URL path.
    pub path: String,
    /// Fixed Wrangler upload token.
    pub wrangler_hash: String,
    /// Exact original byte length.
    pub size: u64,
    /// Response media type frozen from the upload part.
    pub content_type: Option<String>,
    /// SHA-256 of uploaded original bytes.
    pub artifact_sha256: Option<[u8; 32]>,
}

/// Restart-safe upload-session authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssetUploadSession {
    /// Opaque session identity.
    pub id: String,
    /// Owning account.
    pub account_id: AccountId,
    /// Target Script name, including before the Script authority is created.
    pub script_name: String,
    /// Whether every manifest object has been verified.
    pub complete: bool,
    /// Version-upload request holding or having consumed this session.
    pub reservation_id: Option<String>,
    /// Expiration time.
    pub expires_at_ms: i64,
    /// Canonically path-sorted entries.
    pub entries: Vec<AssetUploadEntry>,
}

/// SQLite owner for fixed-Wrangler upload state.
#[derive(Clone, Copy, Debug)]
pub struct AssetUploadRepository<'a> {
    db: &'a ControlDb,
}

impl<'a> AssetUploadRepository<'a> {
    /// Bind the repository to the control database.
    #[must_use]
    pub const fn new(db: &'a ControlDb) -> Self {
        Self { db }
    }

    /// Create one new session without making a Script visible.
    #[allow(clippy::too_many_arguments)]
    pub fn create(
        &self,
        id: &str,
        account_id: AccountId,
        script_name: &str,
        entries: &[NewAssetUploadEntry],
        now_ms: i64,
        expires_at_ms: i64,
        max_open_per_worker: u32,
    ) -> Result<AssetUploadSession, PlatformError> {
        crate::workers::validate_worker_name(script_name)?;
        validate_entries(entries)?;
        if expires_at_ms <= now_ms || max_open_per_worker == 0 {
            return Err(invalid());
        }
        self.db.with_immediate(|tx| {
            tx.execute(
                "UPDATE asset_upload_sessions SET status='expired',updated_at_ms=?1
                 WHERE status='open' AND expires_at_ms<=?1",
                [now_ms],
            )
            .map_err(|_| db_error())?;
            let account_exists: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM accounts WHERE id=?1)",
                    [account_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(|_| db_error())?;
            if !account_exists {
                return Err(not_found());
            }
            let open: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM asset_upload_sessions
                     WHERE account_id=?1 AND script_name=?2 AND status='open'",
                    params![account_id.to_string(), script_name],
                    |row| row.get(0),
                )
                .map_err(|_| db_error())?;
            if open >= i64::from(max_open_per_worker) {
                return Err(PlatformError::new(
                    ErrorCode::AssetLimitExceeded,
                    "Static Assets upload session quota was exceeded",
                ));
            }
            tx.execute(
                "INSERT INTO asset_upload_sessions
                 (id,account_id,script_name,status,created_at_ms,expires_at_ms,updated_at_ms)
                 VALUES (?1,?2,?3,'open',?4,?5,?4)",
                params![
                    id,
                    account_id.to_string(),
                    script_name,
                    now_ms,
                    expires_at_ms,
                ],
            )
            .map_err(|_| db_error())?;
            for entry in entries {
                tx.execute(
                    "INSERT INTO asset_upload_entries
                     (session_id,path,wrangler_hash,size,content_type,artifact_sha256,uploaded_at_ms)
                     VALUES (?1,?2,?3,?4,?5,?6,?7)",
                    params![
                        id,
                        entry.path,
                        entry.wrangler_hash,
                        entry.size,
                        Option::<String>::None,
                        Option::<Vec<u8>>::None,
                        Option::<i64>::None,
                    ],
                )
                .map_err(|_| db_error())?;
            }
            refresh_complete(tx, id, now_ms)?;
            read(tx, id, now_ms)
        })
    }

    /// Read one non-expired session within its account and Script scope.
    pub fn get(
        &self,
        id: &str,
        account_id: AccountId,
        script_name: &str,
        now_ms: i64,
    ) -> Result<AssetUploadSession, PlatformError> {
        self.db.with_immediate(|tx| {
            tx.execute(
                "UPDATE asset_upload_sessions SET status='expired',updated_at_ms=?1
                 WHERE id=?2 AND status='open' AND expires_at_ms<=?1",
                params![now_ms, id],
            )
            .map_err(|_| db_error())?;
            let session = read(tx, id, now_ms)?;
            if session.account_id != account_id || session.script_name != script_name {
                return Err(not_found());
            }
            Ok(session)
        })
    }

    /// Persist one verified object for every manifest path carrying the same Wrangler hash.
    #[allow(clippy::too_many_arguments)]
    pub fn mark_uploaded(
        &self,
        id: &str,
        account_id: AccountId,
        script_name: &str,
        wrangler_hash: &str,
        content_type: Option<&str>,
        artifact_sha256: [u8; 32],
        size: u64,
        now_ms: i64,
    ) -> Result<AssetUploadSession, PlatformError> {
        self.db.with_immediate(|tx| {
            let session = read(tx, id, now_ms)?;
            if session.account_id != account_id || session.script_name != script_name {
                return Err(not_found());
            }
            let matching = session
                .entries
                .iter()
                .filter(|entry| entry.wrangler_hash == wrangler_hash)
                .collect::<Vec<_>>();
            if matching.is_empty()
                || matching.iter().any(|entry| {
                    entry.size != size
                        || entry
                            .artifact_sha256
                            .is_some_and(|value| value != artifact_sha256)
                        || entry.artifact_sha256.is_some()
                            && entry.content_type.as_deref() != content_type
                })
            {
                return Err(PlatformError::new(
                    ErrorCode::AssetUploadConflict,
                    "asset retry does not match verified session evidence",
                ));
            }
            if matching.iter().all(|entry| entry.artifact_sha256.is_some()) {
                return Ok(session);
            }
            let changed = tx
                .execute(
                    "UPDATE asset_upload_entries
                     SET content_type=?1,artifact_sha256=?2,uploaded_at_ms=?3
                     WHERE session_id=?4 AND wrangler_hash=?5 AND size=?6
                       AND artifact_sha256 IS NULL",
                    params![
                        content_type,
                        artifact_sha256.as_slice(),
                        now_ms,
                        id,
                        wrangler_hash,
                        size,
                    ],
                )
                .map_err(|_| db_error())?;
            if changed == 0 {
                return Err(PlatformError::new(
                    ErrorCode::AssetUploadConflict,
                    "asset hash or declared size does not match the upload session",
                ));
            }
            refresh_complete(tx, id, now_ms)?;
            read(tx, id, now_ms)
        })
    }

    /// Reserve a completed session for exactly one Version-upload request.
    pub fn reserve(
        &self,
        id: &str,
        account_id: AccountId,
        script_name: &str,
        reservation_id: &str,
        now_ms: i64,
    ) -> Result<AssetUploadSession, PlatformError> {
        self.db.with_immediate(|tx| {
            let session = read(tx, id, now_ms)?;
            if session.account_id != account_id || session.script_name != script_name {
                return Err(not_found());
            }
            match session.reservation_id.as_deref() {
                Some(existing) if existing == reservation_id => return Ok(session),
                Some(_) => return Ok(session),
                None if session.complete => {}
                None => {
                    return Err(PlatformError::new(
                        ErrorCode::AssetUploadIncomplete,
                        "Static Assets upload is incomplete",
                    ));
                }
            }
            tx.execute(
                "UPDATE asset_upload_sessions
                 SET status='reserved',reservation_id=?1,released_reservation_id=NULL,updated_at_ms=?2
                 WHERE id=?3 AND status='complete' AND reservation_id IS NULL",
                params![reservation_id, now_ms, id],
            )
            .map_err(|_| db_error())?;
            read(tx, id, now_ms)
        })
    }

    /// Mark one reserved session consumed after the Version transaction completes.
    pub fn consume(
        &self,
        id: &str,
        reservation_id: &str,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            let session = read(tx, id, now_ms)?;
            if session.reservation_id.as_deref() != Some(reservation_id) {
                return Err(not_found());
            }
            if tx
                .execute(
                    "UPDATE asset_upload_sessions SET status='consumed',updated_at_ms=?1
                     WHERE id=?2 AND reservation_id=?3 AND status='reserved'",
                    params![now_ms, id, reservation_id],
                )
                .map_err(|_| db_error())?
                == 0
                && session.reservation_id.as_deref() != Some(reservation_id)
            {
                return Err(not_found());
            }
            Ok(())
        })
    }

    /// Release a reservation after a failed Version creation so the upload can be retried.
    pub fn release(
        &self,
        id: &str,
        reservation_id: &str,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        self.db.with_immediate(|tx| {
            let changed = tx.execute(
                "UPDATE asset_upload_sessions
                 SET status='complete',reservation_id=NULL,released_reservation_id=?3,updated_at_ms=?1
                 WHERE id=?2 AND reservation_id=?3 AND status='reserved'",
                params![now_ms, id, reservation_id],
            )
            .map_err(|_| db_error())?;
            if changed == 1 {
                return Ok(());
            }
            let replay: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM asset_upload_sessions
                     WHERE id=?1 AND status='complete' AND reservation_id IS NULL
                       AND released_reservation_id=?2)",
                    params![id, reservation_id],
                    |row| row.get(0),
                )
                .map_err(|_| db_error())?;
            replay.then_some(()).ok_or_else(not_found)
        })
    }
}

fn validate_entries(entries: &[NewAssetUploadEntry]) -> Result<(), PlatformError> {
    if entries.is_empty() || entries.len() > 20_000 {
        return Err(invalid());
    }
    let mut prior: Option<&str> = None;
    let mut total = 0_u64;
    let mut hash_sizes = BTreeMap::new();
    for entry in entries {
        if entry.path.is_empty()
            || entry.path.len() > 2_048
            || entry.path.contains('\\')
            || entry.wrangler_hash.len() != 32
            || !entry
                .wrangler_hash
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            || entry.size > 25 * 1024 * 1024
            || prior.is_some_and(|value| value.as_bytes() >= entry.path.as_bytes())
        {
            return Err(invalid());
        }
        total = total.checked_add(entry.size).ok_or_else(invalid)?;
        if total > 512 * 1024 * 1024 {
            return Err(invalid());
        }
        if hash_sizes
            .insert(entry.wrangler_hash.as_str(), entry.size)
            .is_some_and(|size| size != entry.size)
        {
            return Err(invalid());
        }
        prior = Some(&entry.path);
    }
    Ok(())
}

fn refresh_complete(
    tx: &rusqlite::Transaction<'_>,
    id: &str,
    now_ms: i64,
) -> Result<(), PlatformError> {
    tx.execute(
        "UPDATE asset_upload_sessions SET status='complete',updated_at_ms=?1
         WHERE id=?2 AND status='open' AND NOT EXISTS(
           SELECT 1 FROM asset_upload_entries WHERE session_id=?2 AND artifact_sha256 IS NULL
         )",
        params![now_ms, id],
    )
    .map_err(|_| db_error())?;
    Ok(())
}

fn read(
    tx: &rusqlite::Transaction<'_>,
    id: &str,
    now_ms: i64,
) -> Result<AssetUploadSession, PlatformError> {
    let row: Option<(String, String, String, Option<String>, i64)> = tx
        .query_row(
            "SELECT account_id,script_name,status,reservation_id,expires_at_ms
             FROM asset_upload_sessions WHERE id=?1 AND status!='expired'",
            [id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()
        .map_err(|_| db_error())?;
    let (account, script_name, status, reservation_id, expires_at_ms) =
        row.ok_or_else(not_found)?;
    if expires_at_ms <= now_ms {
        return Err(not_found());
    }
    let mut statement = tx
        .prepare(
            "SELECT path,wrangler_hash,size,content_type,artifact_sha256
             FROM asset_upload_entries WHERE session_id=?1 ORDER BY path",
        )
        .map_err(|_| db_error())?;
    let rows = statement
        .query_map([id], |row| {
            let size: i64 = row.get(2)?;
            let digest: Option<Vec<u8>> = row.get(4)?;
            Ok(AssetUploadEntry {
                path: row.get(0)?,
                wrangler_hash: row.get(1)?,
                size: u64::try_from(size).map_err(|_| rusqlite::Error::InvalidQuery)?,
                content_type: row.get(3)?,
                artifact_sha256: digest
                    .map(|value| {
                        <[u8; 32]>::try_from(value).map_err(|_| rusqlite::Error::InvalidQuery)
                    })
                    .transpose()?,
            })
        })
        .map_err(|_| db_error())?;
    let entries = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| db_error())?;
    Ok(AssetUploadSession {
        id: id.to_owned(),
        account_id: AccountId::from_str(&account).map_err(|_| db_error())?,
        script_name,
        complete: matches!(status.as_str(), "complete" | "reserved" | "consumed"),
        reservation_id,
        expires_at_ms,
        entries,
    })
}

fn invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::AssetManifestInvalid,
        "Static Assets manifest is invalid",
    )
}

fn not_found() -> PlatformError {
    PlatformError::new(
        ErrorCode::AssetUploadIncomplete,
        "Static Assets upload session is missing or expired",
    )
}

fn db_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::VersionInvariantViolation,
        "Static Assets upload authority is inconsistent",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PlatformStorage;
    use open_compute_core::{DataConfig, SystemClock};

    fn config(root: &std::path::Path) -> DataConfig {
        DataConfig {
            path: root.to_owned(),
            master_key_file: root.join("keys/master.key"),
            master_key_env: None,
            sqlite_busy_timeout_ms: 5_000,
            free_space_soft_bytes: 1_073_741_824,
            free_space_hard_bytes: 268_435_456,
        }
    }

    #[test]
    fn sessions_precede_scripts_and_reservations_are_restart_safe_and_single_use() {
        let temp = tempfile::tempdir().unwrap();
        let config = config(&temp.path().join("data"));
        let account;
        {
            let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
            account = storage.identity().default_account_id;
            let repository = AssetUploadRepository::new(storage.db());
            let session = repository
                .create(
                    "session",
                    account,
                    "new-script",
                    &[NewAssetUploadEntry {
                        path: "/index.html".to_owned(),
                        wrangler_hash: "0123456789abcdef0123456789abcdef".to_owned(),
                        size: 4,
                    }],
                    10,
                    1_000,
                    2,
                )
                .unwrap();
            assert!(!session.complete);
            assert_eq!(session.script_name, "new-script");
            assert!(
                repository
                    .mark_uploaded(
                        "session",
                        account,
                        "new-script",
                        "0123456789abcdef0123456789abcdef",
                        Some("text/html"),
                        [7; 32],
                        4,
                        20,
                    )
                    .unwrap()
                    .complete
            );
            assert_eq!(
                repository
                    .mark_uploaded(
                        "session",
                        account,
                        "new-script",
                        "0123456789abcdef0123456789abcdef",
                        Some("text/html"),
                        [8; 32],
                        4,
                        21,
                    )
                    .unwrap_err()
                    .code(),
                ErrorCode::AssetUploadConflict
            );
        }
        let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
        let repository = AssetUploadRepository::new(storage.db());
        assert!(
            repository
                .reserve("session", account, "new-script", "request-one", 30)
                .unwrap()
                .complete
        );
        assert!(
            repository
                .reserve("session", account, "new-script", "request-one", 31)
                .is_ok()
        );
        assert_eq!(
            repository
                .reserve("session", account, "new-script", "request-two", 31)
                .unwrap()
                .reservation_id
                .as_deref(),
            Some("request-one")
        );
        repository.release("session", "request-one", 32).unwrap();
        repository.release("session", "request-one", 32).unwrap();
        assert_eq!(
            repository
                .release("session", "wrong-request", 32)
                .unwrap_err()
                .code(),
            ErrorCode::AssetUploadIncomplete
        );
        repository
            .reserve("session", account, "new-script", "request-two", 33)
            .unwrap();
        repository.consume("session", "request-two", 34).unwrap();
        assert!(
            storage
                .db()
                .with_immediate(|tx| {
                    tx.execute(
                        "UPDATE asset_upload_sessions SET status='complete' WHERE id='session'",
                        [],
                    )
                    .map(|_| ())
                    .map_err(|_| db_error())
                })
                .is_err()
        );
        assert!(
            storage
                .db()
                .with_immediate(|tx| {
                    tx.execute(
                        "UPDATE asset_upload_entries SET artifact_sha256=zeroblob(32) WHERE session_id='session'",
                        [],
                    )
                    .map(|_| ())
                    .map_err(|_| db_error())
                })
                .is_err()
        );
        assert_eq!(
            repository
                .reserve("session", account, "new-script", "request-three", 35)
                .unwrap()
                .reservation_id
                .as_deref(),
            Some("request-two")
        );
        assert_eq!(
            repository
                .get("session", account, "other-script", 35)
                .unwrap_err()
                .code(),
            ErrorCode::AssetUploadIncomplete
        );
        assert_eq!(
            repository
                .get("session", account, "new-script", 1_000)
                .unwrap_err()
                .code(),
            ErrorCode::AssetUploadIncomplete
        );
    }

    #[test]
    fn manifest_rows_reject_noncanonical_order_and_backslashes_before_insert() {
        let temp = tempfile::tempdir().unwrap();
        let storage =
            PlatformStorage::bootstrap(&config(&temp.path().join("data")), &SystemClock).unwrap();
        let repository = AssetUploadRepository::new(storage.db());
        let account = storage.identity().default_account_id;
        for entries in [
            vec![NewAssetUploadEntry {
                path: "/bad\\path".to_owned(),
                wrangler_hash: "0123456789abcdef0123456789abcdef".to_owned(),
                size: 1,
            }],
            vec![
                NewAssetUploadEntry {
                    path: "/b".to_owned(),
                    wrangler_hash: "0123456789abcdef0123456789abcdef".to_owned(),
                    size: 1,
                },
                NewAssetUploadEntry {
                    path: "/a".to_owned(),
                    wrangler_hash: "fedcba9876543210fedcba9876543210".to_owned(),
                    size: 1,
                },
            ],
        ] {
            assert_eq!(
                repository
                    .create("invalid", account, "script", &entries, 1, 100, 2)
                    .unwrap_err()
                    .code(),
                ErrorCode::AssetManifestInvalid
            );
        }
    }
}
