//! Private control-plane [`rusqlite`] connection.

use crate::migrations;
use open_compute_core::clock::Clock;
use open_compute_core::{ErrorCode, PlatformError};
#[cfg(test)]
use rusqlite::OptionalExtension;
use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior};
use std::path::Path;
use std::sync::Mutex;

/// Control database. The rusqlite connection is never exposed.
#[derive(Debug)]
pub struct ControlDb {
    conn: Mutex<Connection>,
}

impl ControlDb {
    /// Open or create `control.sqlite` with P0.1 PRAGMAs.
    pub fn open(path: &Path, busy_timeout_ms: u64) -> Result<Self, PlatformError> {
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NOFOLLOW;
        let open_path = leaf_nofollow_path(path)?;
        let conn = Connection::open_with_flags(&open_path, flags).map_err(|_| {
            PlatformError::new(
                ErrorCode::MigrationFailed,
                "failed to open control database",
            )
        })?;
        conn.busy_timeout(std::time::Duration::from_millis(busy_timeout_ms))
            .map_err(|_| {
                PlatformError::new(
                    ErrorCode::MigrationFailed,
                    "failed to set sqlite busy_timeout",
                )
            })?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|_| {
                PlatformError::new(ErrorCode::MigrationFailed, "failed to enable WAL journal")
            })?;
        conn.pragma_update(None, "synchronous", "FULL")
            .map_err(|_| {
                PlatformError::new(ErrorCode::MigrationFailed, "failed to set synchronous FULL")
            })?;
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(|_| {
                PlatformError::new(ErrorCode::MigrationFailed, "failed to enable foreign_keys")
            })?;
        conn.pragma_update(None, "trusted_schema", "OFF")
            .map_err(|_| {
                PlatformError::new(
                    ErrorCode::MigrationFailed,
                    "failed to disable trusted_schema",
                )
            })?;
        let db = Self {
            conn: Mutex::new(conn),
        };
        db.verify_foreign_keys()?;
        db.quick_check()?;
        Ok(db)
    }

    /// Open an existing control database read-only with `query_only`.
    ///
    /// Does not create files, enable WAL, migrate, or checkpoint.
    pub fn open_readonly(path: &Path, busy_timeout_ms: u64) -> Result<Self, PlatformError> {
        Self::open_readonly_uri(path, busy_timeout_ms, sqlite_readonly_uri)
    }

    /// Open an existing control database read-only while observing committed WAL frames.
    ///
    /// This is reserved for stopped-platform startup and restore fences. SQLite may create
    /// or update WAL coordination sidecars, so zero-side-effect diagnostics must use
    /// [`Self::open_readonly`] instead.
    pub fn open_readonly_wal_aware(
        path: &Path,
        busy_timeout_ms: u64,
    ) -> Result<Self, PlatformError> {
        Self::open_readonly_uri(path, busy_timeout_ms, sqlite_wal_readonly_uri)
    }

    fn open_readonly_uri(
        path: &Path,
        busy_timeout_ms: u64,
        make_uri: fn(&Path) -> String,
    ) -> Result<Self, PlatformError> {
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NOFOLLOW
            | OpenFlags::SQLITE_OPEN_URI;
        let open_path = leaf_nofollow_path(path)?;
        let uri = make_uri(&open_path);
        let conn = Connection::open_with_flags(&uri, flags).map_err(|_| {
            PlatformError::new(
                ErrorCode::MigrationFailed,
                "failed to open control database read-only",
            )
        })?;
        conn.busy_timeout(std::time::Duration::from_millis(busy_timeout_ms))
            .map_err(|_| {
                PlatformError::new(
                    ErrorCode::MigrationFailed,
                    "failed to set sqlite busy_timeout",
                )
            })?;
        conn.pragma_update(None, "query_only", "ON").map_err(|_| {
            PlatformError::new(
                ErrorCode::MigrationFailed,
                "failed to enable sqlite query_only",
            )
        })?;
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(|_| {
                PlatformError::new(ErrorCode::MigrationFailed, "failed to enable foreign_keys")
            })?;
        conn.pragma_update(None, "trusted_schema", "OFF")
            .map_err(|_| {
                PlatformError::new(
                    ErrorCode::MigrationFailed,
                    "failed to disable trusted_schema",
                )
            })?;
        let db = Self {
            conn: Mutex::new(conn),
        };
        db.verify_foreign_keys()?;
        Ok(db)
    }

    /// Run `PRAGMA quick_check` after verifying `foreign_keys=ON`.
    pub fn quick_check(&self) -> Result<(), PlatformError> {
        self.verify_foreign_keys()?;
        let conn = self.lock()?;
        verify_foreign_keys_on(&conn)?;
        let status: String = conn
            .pragma_query_value(None, "quick_check", |row| row.get(0))
            .map_err(|_| {
                PlatformError::new(
                    ErrorCode::MigrationFailed,
                    "control database quick_check failed",
                )
            })?;
        if status != "ok" {
            return Err(PlatformError::new(
                ErrorCode::MigrationFailed,
                "control database quick_check reported corruption",
            ));
        }
        Ok(())
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>, PlatformError> {
        self.conn.lock().map_err(|_| {
            PlatformError::new(ErrorCode::MigrationFailed, "control db mutex poisoned")
        })
    }

    pub(crate) fn verify_foreign_keys(&self) -> Result<(), PlatformError> {
        let conn = self.lock()?;
        verify_foreign_keys_on(&conn)
    }

    /// Toggle foreign-key enforcement for fail-closed integration tests.
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_foreign_keys_for_test(&self, enabled: bool) -> Result<(), PlatformError> {
        let conn = self.lock()?;
        conn.pragma_update(None, "foreign_keys", if enabled { "ON" } else { "OFF" })
            .map_err(|_| {
                PlatformError::new(
                    ErrorCode::MigrationFailed,
                    "failed to configure test foreign-key enforcement",
                )
            })
    }

    /// Apply pending forward-only migrations.
    pub fn migrate(&self, clock: &dyn Clock) -> Result<(), PlatformError> {
        self.verify_foreign_keys()?;
        migrations::apply(self, clock)
    }

    /// Apply migrations with test-only fault injection.
    #[cfg(any(test, feature = "test-support"))]
    pub fn migrate_with_fault(
        &self,
        clock: &dyn Clock,
        fault: Option<migrations::MigrationFault>,
    ) -> Result<(), PlatformError> {
        self.verify_foreign_keys()?;
        migrations::apply_with_fault(self, clock, fault)
    }

    /// Current `PRAGMA user_version`.
    pub fn user_version(&self) -> Result<i64, PlatformError> {
        let conn = self.lock()?;
        verify_foreign_keys_on(&conn)?;
        conn.pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(|_| {
                PlatformError::new(ErrorCode::MigrationFailed, "failed to read user_version")
            })
    }

    pub(crate) fn with_exclusive<T>(
        &self,
        f: impl FnOnce(&Transaction<'_>) -> Result<T, PlatformError>,
    ) -> Result<T, PlatformError> {
        let mut conn = self.lock()?;
        verify_foreign_keys_on(&conn)?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Exclusive)
            .map_err(|_| {
                PlatformError::new(
                    ErrorCode::MigrationFailed,
                    "failed to begin exclusive transaction",
                )
            })?;
        verify_foreign_keys_on(&tx)?;
        let result = f(&tx)?;
        tx.commit().map_err(|_| {
            PlatformError::new(
                ErrorCode::MigrationFailed,
                "failed to commit exclusive transaction",
            )
        })?;
        Ok(result)
    }

    pub(crate) fn with_immediate<T>(
        &self,
        f: impl FnOnce(&Transaction<'_>) -> Result<T, PlatformError>,
    ) -> Result<T, PlatformError> {
        let mut conn = self.lock()?;
        verify_foreign_keys_on(&conn)?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| {
                PlatformError::new(
                    ErrorCode::MigrationFailed,
                    "failed to begin immediate transaction",
                )
            })?;
        verify_foreign_keys_on(&tx)?;
        let result = f(&tx)?;
        tx.commit().map_err(|_| {
            PlatformError::new(
                ErrorCode::MigrationFailed,
                "failed to commit immediate transaction",
            )
        })?;
        Ok(result)
    }

    #[cfg(test)]
    pub(crate) fn query_meta(&self, key: &str) -> Result<Option<String>, PlatformError> {
        use rusqlite::OptionalExtension;

        self.with_exclusive(|tx| {
            let bytes: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT value FROM platform_meta WHERE key = ?1",
                    [key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|_| {
                    PlatformError::new(ErrorCode::MigrationFailed, "failed to read platform_meta")
                })?;
            match bytes {
                None => Ok(None),
                Some(raw) => {
                    let value = String::from_utf8(raw).map_err(|_| {
                        PlatformError::new(
                            ErrorCode::ConfigInvalid,
                            "platform_meta value is not valid UTF-8",
                        )
                    })?;
                    Ok(Some(value))
                }
            }
        })
    }

    #[cfg(test)]
    pub(crate) fn pragma_display(&self, name: &str) -> Result<String, PlatformError> {
        let conn = self.lock()?;
        verify_foreign_keys_on(&conn)?;
        conn.pragma_query_value(None, name, |row| {
            if let Ok(s) = row.get::<_, String>(0) {
                return Ok(s);
            }
            if let Ok(i) = row.get::<_, i64>(0) {
                return Ok(i.to_string());
            }
            Err(rusqlite::Error::InvalidQuery)
        })
        .map_err(|_| PlatformError::new(ErrorCode::MigrationFailed, "failed to read pragma"))
    }

    pub(crate) fn with_read<T>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T, PlatformError>,
    ) -> Result<T, PlatformError> {
        let conn = self.lock()?;
        verify_foreign_keys_on(&conn)?;
        f(&conn)
    }

    pub(crate) fn table_exists(&self, name: &str) -> Result<bool, PlatformError> {
        self.with_read(|conn| {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [name],
                    |row| row.get(0),
                )
                .map_err(|_| {
                    PlatformError::new(
                        ErrorCode::MigrationFailed,
                        "failed to inspect sqlite_master",
                    )
                })?;
            Ok(count > 0)
        })
    }

    #[cfg(test)]
    pub(crate) fn index_sql(&self, name: &str) -> Result<Option<String>, PlatformError> {
        self.with_exclusive(|tx| {
            tx.query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'index' AND name = ?1",
                [name],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| PlatformError::new(ErrorCode::MigrationFailed, "failed to inspect index"))
        })
    }

    #[cfg(test)]
    pub(crate) fn table_sql(&self, name: &str) -> Result<Option<String>, PlatformError> {
        self.with_exclusive(|tx| {
            tx.query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [name],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| PlatformError::new(ErrorCode::MigrationFailed, "failed to inspect table"))
        })
    }

    #[cfg(test)]
    pub(crate) fn dump_bytes(&self) -> Result<Vec<u8>, PlatformError> {
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare("SELECT sql FROM sqlite_master")
            .map_err(|_| PlatformError::new(ErrorCode::MigrationFailed, "dump failed"))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, Option<String>>(0))
            .map_err(|_| PlatformError::new(ErrorCode::MigrationFailed, "dump failed"))?;
        let mut out = Vec::new();
        for row in rows.flatten().flatten() {
            out.extend_from_slice(row.as_bytes());
        }
        Ok(out)
    }
}

pub(crate) fn leaf_nofollow_path(path: &Path) -> Result<std::path::PathBuf, PlatformError> {
    let parent = path.parent().ok_or_else(|| {
        PlatformError::new(
            ErrorCode::PathInvalid,
            "control database path must have a parent",
        )
    })?;
    let name = path.file_name().ok_or_else(|| {
        PlatformError::new(
            ErrorCode::PathInvalid,
            "control database path must have a file name",
        )
    })?;
    let parent_canon = std::fs::canonicalize(parent).map_err(|_| {
        PlatformError::new(
            ErrorCode::PathInvalid,
            "control database parent cannot be canonicalized",
        )
    })?;
    let open_path = parent_canon.join(name);
    if let Ok(meta) = std::fs::symlink_metadata(&open_path)
        && meta.file_type().is_symlink()
    {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "owned path must not be a symlink",
        ));
    }
    Ok(open_path)
}

pub(crate) fn sqlite_readonly_uri(path: &Path) -> String {
    sqlite_uri(path, true)
}

fn sqlite_wal_readonly_uri(path: &Path) -> String {
    sqlite_uri(path, false)
}

fn sqlite_uri(path: &Path, immutable: bool) -> String {
    let raw = path.to_string_lossy();
    let mut encoded = String::from("file:");
    for byte in raw.as_bytes() {
        match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(*byte as char);
            }
            other => {
                encoded.push_str(&format!("%{other:02X}"));
            }
        }
    }
    encoded.push_str(if immutable {
        "?mode=ro&immutable=1"
    } else {
        "?mode=ro"
    });
    encoded
}

pub(crate) fn verify_foreign_keys_on(conn: &Connection) -> Result<(), PlatformError> {
    let value: i64 = conn
        .pragma_query_value(None, "foreign_keys", |row| row.get(0))
        .map_err(|_| {
            PlatformError::new(
                ErrorCode::MigrationFailed,
                "failed to verify foreign_keys pragma",
            )
        })?;
    if value != 1 {
        return Err(PlatformError::new(
            ErrorCode::MigrationFailed,
            "foreign_keys must be ON for every control-db operation",
        ));
    }
    Ok(())
}

pub(crate) fn set_user_version(tx: &Transaction<'_>, version: i64) -> Result<(), PlatformError> {
    tx.pragma_update(None, "user_version", version)
        .map_err(|_| PlatformError::new(ErrorCode::MigrationFailed, "failed to set user_version"))
}

#[cfg(test)]
#[path = "control_db_tests.rs"]
mod coverage_tests;
