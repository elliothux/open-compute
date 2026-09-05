//! Stable platform identity and default account.

use crate::control_db::ControlDb;
use open_compute_core::clock::Clock;
use open_compute_core::{AccountId, ErrorCode, ObjectStorageKind, PlatformError, PlatformId};
use rusqlite::OptionalExtension;
use std::str::FromStr;
use std::time::UNIX_EPOCH;

const KEY_PLATFORM_ID: &str = "platform_id";
const KEY_CREATED_AT: &str = "created_at_ms";
const KEY_LAST_STARTED: &str = "last_started_version";
const KEY_MASTER_KEY_ID: &str = "master_key_id";
const KEY_ARTIFACT_SCHEMA: &str = "artifact_schema_version";
const KEY_OBJECT_BACKEND_KIND: &str = "object_backend_kind";
const KEY_OBJECT_AUTHORITY: &str = "object_authority_sha256";
const UNBOUND_OBJECT_AUTHORITY: &str = "unbound";
const DEFAULT_ACCOUNT_NAME: &str = "default";
/// Current artifact schema version persisted at bootstrap.
pub const ARTIFACT_SCHEMA_VERSION: &str = "1";
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Stable identifiers initialized exactly once.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StableIdentity {
    /// Platform instance ID.
    pub platform_id: PlatformId,
    /// Default live account.
    pub default_account_id: AccountId,
    /// Creation time in unix milliseconds.
    pub created_at_ms: i64,
    /// Non-secret master key fingerprint.
    pub master_key_id: String,
    /// Artifact schema version string.
    pub artifact_schema_version: String,
    /// Selected object backend after the first successful authority bind.
    pub object_backend_kind: Option<ObjectStorageKind>,
    /// Selected object authority fingerprint after the first successful bind.
    pub object_authority_sha256: Option<[u8; 32]>,
}

/// Initialize identity inside one exclusive transaction.
pub fn bootstrap(
    db: &ControlDb,
    clock: &dyn Clock,
    master_key_id: &str,
) -> Result<StableIdentity, PlatformError> {
    let now = millis(clock);
    db.with_exclusive(|tx| {
        let existing_platform = read_meta(tx, KEY_PLATFORM_ID)?;

        if let Some(existing) = existing_platform {
            let platform_id = PlatformId::from_str(&existing).map_err(|_| {
                PlatformError::new(ErrorCode::ConfigInvalid, "stored platform_id is invalid")
            })?;
            let created = require_meta(tx, KEY_CREATED_AT)?;
            let created_at_ms = created.parse::<i64>().map_err(|_| {
                PlatformError::new(ErrorCode::ConfigInvalid, "stored created_at_ms is invalid")
            })?;
            let stored_key = require_meta(tx, KEY_MASTER_KEY_ID)?;
            if stored_key != master_key_id {
                return Err(PlatformError::new(
                    ErrorCode::MasterKeyMismatch,
                    "master key fingerprint does not match stored identity",
                ));
            }
            let artifact = require_meta(tx, KEY_ARTIFACT_SCHEMA)?;
            if artifact != ARTIFACT_SCHEMA_VERSION {
                return Err(PlatformError::new(
                    ErrorCode::MigrationFailed,
                    "stored artifact schema version is not supported",
                ));
            }
            let default_account_id = require_default_account(tx)?;
            let (object_backend_kind, object_authority_sha256) =
                read_object_authority_tx(tx)?;
            upsert_meta(tx, KEY_LAST_STARTED, APP_VERSION, now)?;
            return Ok(StableIdentity {
                platform_id,
                default_account_id,
                created_at_ms,
                master_key_id: master_key_id.to_string(),
                artifact_schema_version: artifact,
                object_backend_kind,
                object_authority_sha256,
            });
        }

        let platform_id = PlatformId::generate();
        let default_account_id = AccountId::generate();
        upsert_meta(tx, KEY_PLATFORM_ID, &platform_id.to_string(), now)?;
        upsert_meta(tx, KEY_CREATED_AT, &now.to_string(), now)?;
        upsert_meta(tx, KEY_MASTER_KEY_ID, master_key_id, now)?;
        upsert_meta(tx, KEY_ARTIFACT_SCHEMA, ARTIFACT_SCHEMA_VERSION, now)?;
        upsert_meta(tx, KEY_OBJECT_BACKEND_KIND, UNBOUND_OBJECT_AUTHORITY, now)?;
        upsert_meta(tx, KEY_OBJECT_AUTHORITY, UNBOUND_OBJECT_AUTHORITY, now)?;
        upsert_meta(tx, KEY_LAST_STARTED, APP_VERSION, now)?;
        tx.execute(
            "INSERT INTO accounts (id, name, created_at_ms, deleted_at_ms) VALUES (?1, ?2, ?3, NULL)",
            rusqlite::params![default_account_id.to_string(), DEFAULT_ACCOUNT_NAME, now],
        )
        .map_err(|_| {
            PlatformError::new(ErrorCode::MigrationFailed, "failed to insert default account")
        })?;

        Ok(StableIdentity {
            platform_id,
            default_account_id,
            created_at_ms: now,
            master_key_id: master_key_id.to_string(),
            artifact_schema_version: ARTIFACT_SCHEMA_VERSION.to_string(),
            object_backend_kind: None,
            object_authority_sha256: None,
        })
    })
}

/// Bind a freshly initialized platform to one object authority, or validate the
/// immutable binding on every later start.
pub fn bind_object_authority(
    db: &ControlDb,
    kind: ObjectStorageKind,
    authority_sha256: &[u8; 32],
    now_ms: i64,
) -> Result<(), PlatformError> {
    db.with_exclusive(|tx| {
        let stored_kind = require_meta(tx, KEY_OBJECT_BACKEND_KIND)?;
        let stored_authority = require_meta(tx, KEY_OBJECT_AUTHORITY)?;
        if stored_kind == UNBOUND_OBJECT_AUTHORITY && stored_authority == UNBOUND_OBJECT_AUTHORITY {
            upsert_meta(
                tx,
                KEY_OBJECT_BACKEND_KIND,
                object_backend_kind_str(kind),
                now_ms,
            )?;
            upsert_meta(
                tx,
                KEY_OBJECT_AUTHORITY,
                &hex::encode(authority_sha256),
                now_ms,
            )?;
            return Ok(());
        }
        let expected_kind = object_backend_kind_str(kind);
        let expected_authority = hex::encode(authority_sha256);
        if stored_kind != expected_kind || stored_authority != expected_authority {
            return Err(PlatformError::new(
                ErrorCode::ObjectStorageAuthorityMismatch,
                "object storage authority does not match stored platform identity",
            ));
        }
        Ok(())
    })
}

fn object_backend_kind_str(kind: ObjectStorageKind) -> &'static str {
    match kind {
        ObjectStorageKind::Local => "local",
        ObjectStorageKind::S3 => "s3",
    }
}

fn parse_object_authority(
    stored_kind: &str,
    stored_authority: &str,
) -> Result<(Option<ObjectStorageKind>, Option<[u8; 32]>), PlatformError> {
    if stored_kind == UNBOUND_OBJECT_AUTHORITY && stored_authority == UNBOUND_OBJECT_AUTHORITY {
        return Ok((None, None));
    }
    let kind = match stored_kind {
        "local" => ObjectStorageKind::Local,
        "s3" => ObjectStorageKind::S3,
        _ => return Err(stored_object_authority_invalid()),
    };
    let decoded = hex::decode(stored_authority).map_err(|_| stored_object_authority_invalid())?;
    let authority = decoded
        .try_into()
        .map_err(|_| stored_object_authority_invalid())?;
    Ok((Some(kind), Some(authority)))
}

fn read_object_authority_tx(
    tx: &rusqlite::Transaction<'_>,
) -> Result<(Option<ObjectStorageKind>, Option<[u8; 32]>), PlatformError> {
    let kind = require_meta(tx, KEY_OBJECT_BACKEND_KIND)?;
    let authority = require_meta(tx, KEY_OBJECT_AUTHORITY)?;
    parse_object_authority(&kind, &authority)
}

fn stored_object_authority_invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::ObjectStorageIntegrityError,
        "stored object authority binding is invalid",
    )
}

fn read_meta(tx: &rusqlite::Transaction<'_>, key: &str) -> Result<Option<String>, PlatformError> {
    let raw: Option<Vec<u8>> = tx
        .query_row(
            "SELECT value FROM platform_meta WHERE key = ?1",
            [key],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| {
            PlatformError::new(ErrorCode::MigrationFailed, "failed to read platform_meta")
        })?;
    match raw {
        None => Ok(None),
        Some(bytes) => {
            let value = String::from_utf8(bytes).map_err(|_| {
                PlatformError::new(
                    ErrorCode::ConfigInvalid,
                    "platform_meta value is not valid UTF-8",
                )
            })?;
            Ok(Some(value))
        }
    }
}

fn require_meta(tx: &rusqlite::Transaction<'_>, key: &str) -> Result<String, PlatformError> {
    read_meta(tx, key)?.ok_or_else(|| {
        PlatformError::new(
            ErrorCode::MigrationFailed,
            "stored platform identity is incomplete",
        )
    })
}

fn require_default_account(tx: &rusqlite::Transaction<'_>) -> Result<AccountId, PlatformError> {
    let id: Option<String> = tx
        .query_row(
            "SELECT id FROM accounts WHERE name = ?1 AND deleted_at_ms IS NULL",
            [DEFAULT_ACCOUNT_NAME],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| {
            PlatformError::new(ErrorCode::MigrationFailed, "failed to read default account")
        })?;
    let id = id.ok_or_else(|| {
        PlatformError::new(
            ErrorCode::MigrationFailed,
            "stored default account is missing",
        )
    })?;
    AccountId::from_str(&id).map_err(|_| {
        PlatformError::new(
            ErrorCode::ConfigInvalid,
            "stored default account id is invalid",
        )
    })
}

fn upsert_meta(
    tx: &rusqlite::Transaction<'_>,
    key: &str,
    value: &str,
    now: i64,
) -> Result<(), PlatformError> {
    tx.execute(
        "INSERT INTO platform_meta (key, value, updated_at_ms) VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at_ms = excluded.updated_at_ms",
        rusqlite::params![key, value.as_bytes(), now],
    )
    .map_err(|_| PlatformError::new(ErrorCode::MigrationFailed, "failed to write platform_meta"))?;
    Ok(())
}

/// Read stored identity without updating `last_started_version`.
pub fn inspect_stored(db: &ControlDb) -> Result<StableIdentity, PlatformError> {
    db.with_read(|conn| {
        let platform = read_meta_conn(conn, KEY_PLATFORM_ID)?.ok_or_else(|| {
            PlatformError::new(
                ErrorCode::MigrationFailed,
                "stored platform identity is missing",
            )
        })?;
        let platform_id = PlatformId::from_str(&platform).map_err(|_| {
            PlatformError::new(ErrorCode::ConfigInvalid, "stored platform_id is invalid")
        })?;
        let created = read_meta_conn(conn, KEY_CREATED_AT)?.ok_or_else(|| {
            PlatformError::new(
                ErrorCode::MigrationFailed,
                "stored platform identity is incomplete",
            )
        })?;
        let created_at_ms = created.parse::<i64>().map_err(|_| {
            PlatformError::new(ErrorCode::ConfigInvalid, "stored created_at_ms is invalid")
        })?;
        let master_key_id = read_meta_conn(conn, KEY_MASTER_KEY_ID)?.ok_or_else(|| {
            PlatformError::new(
                ErrorCode::MigrationFailed,
                "stored platform identity is incomplete",
            )
        })?;
        let artifact = read_meta_conn(conn, KEY_ARTIFACT_SCHEMA)?.ok_or_else(|| {
            PlatformError::new(
                ErrorCode::MigrationFailed,
                "stored platform identity is incomplete",
            )
        })?;
        if artifact != ARTIFACT_SCHEMA_VERSION {
            return Err(PlatformError::new(
                ErrorCode::MigrationFailed,
                "stored artifact schema version is not supported",
            ));
        }
        let default_account_id = require_default_account_conn(conn)?;
        let object_kind = read_meta_conn(conn, KEY_OBJECT_BACKEND_KIND)?.ok_or_else(|| {
            PlatformError::new(
                ErrorCode::MigrationFailed,
                "stored object authority binding is missing",
            )
        })?;
        let object_authority = read_meta_conn(conn, KEY_OBJECT_AUTHORITY)?.ok_or_else(|| {
            PlatformError::new(
                ErrorCode::MigrationFailed,
                "stored object authority binding is missing",
            )
        })?;
        let (object_backend_kind, object_authority_sha256) =
            parse_object_authority(&object_kind, &object_authority)?;
        Ok(StableIdentity {
            platform_id,
            default_account_id,
            created_at_ms,
            master_key_id,
            artifact_schema_version: artifact,
            object_backend_kind,
            object_authority_sha256,
        })
    })
}

fn read_meta_conn(conn: &rusqlite::Connection, key: &str) -> Result<Option<String>, PlatformError> {
    let raw: Option<Vec<u8>> = conn
        .query_row(
            "SELECT value FROM platform_meta WHERE key = ?1",
            [key],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| {
            PlatformError::new(ErrorCode::MigrationFailed, "failed to read platform_meta")
        })?;
    match raw {
        None => Ok(None),
        Some(bytes) => {
            let value = String::from_utf8(bytes).map_err(|_| {
                PlatformError::new(
                    ErrorCode::ConfigInvalid,
                    "platform_meta value is not valid UTF-8",
                )
            })?;
            Ok(Some(value))
        }
    }
}

fn require_default_account_conn(conn: &rusqlite::Connection) -> Result<AccountId, PlatformError> {
    let id: Option<String> = conn
        .query_row(
            "SELECT id FROM accounts WHERE name = ?1 AND deleted_at_ms IS NULL",
            [DEFAULT_ACCOUNT_NAME],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| {
            PlatformError::new(ErrorCode::MigrationFailed, "failed to read default account")
        })?;
    let id = id.ok_or_else(|| {
        PlatformError::new(
            ErrorCode::MigrationFailed,
            "stored default account is missing",
        )
    })?;
    AccountId::from_str(&id).map_err(|_| {
        PlatformError::new(
            ErrorCode::ConfigInvalid,
            "stored default account id is invalid",
        )
    })
}

fn millis(clock: &dyn Clock) -> i64 {
    clock
        .now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}
