//! Bounded disposable cache for normalized AI Search parse results.

use open_compute_core::{ErrorCode, PlatformError};
use rusqlite::{Connection, OpenFlags, OptionalExtension as _, TransactionBehavior, params};
use sha2::{Digest as _, Sha256};
use std::path::Path;
use std::sync::Mutex;

const CACHE_SCHEMA_VERSION: i64 = 1;
const MAX_CACHE_ENTRIES: i64 = 512;
const MAX_CACHE_BYTES: i64 = 128 * 1024 * 1024;
const MAX_CACHE_ENTRY_BYTES: usize = 32 * 1024 * 1024;
const MAX_SOURCE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_FILENAME_BYTES: usize = 1_024;
const MAX_CONTENT_TYPE_BYTES: usize = 128;
const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS parse_cache (
    cache_key BLOB PRIMARY KEY CHECK(length(cache_key) = 32),
    source_sha256 BLOB NOT NULL CHECK(length(source_sha256) = 32),
    source_size INTEGER NOT NULL CHECK(source_size > 0),
    filename TEXT NOT NULL,
    content_type TEXT NOT NULL,
    parser_contract_sha256 BLOB NOT NULL CHECK(length(parser_contract_sha256) = 32),
    payload BLOB NOT NULL,
    payload_sha256 BLOB NOT NULL CHECK(length(payload_sha256) = 32),
    created_at_ms INTEGER NOT NULL,
    last_access_at_ms INTEGER NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS parse_cache_lru
ON parse_cache(last_access_at_ms, created_at_ms, cache_key);
";

/// Complete identity of one reusable derived parse result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AiSearchParseCacheKey {
    digest: [u8; 32],
    source_sha256: [u8; 32],
    source_size: u64,
    filename: String,
    content_type: String,
    parser_contract_sha256: [u8; 32],
}

impl AiSearchParseCacheKey {
    /// Validate all output-affecting inputs and derive the canonical cache key.
    pub fn new(
        source_sha256: [u8; 32],
        source_size: u64,
        filename: &str,
        content_type: &str,
        parser_contract_sha256: [u8; 32],
    ) -> Result<Self, PlatformError> {
        if source_size == 0
            || source_size > MAX_SOURCE_BYTES
            || !valid_text(filename, MAX_FILENAME_BYTES)
            || !valid_text(content_type, MAX_CONTENT_TYPE_BYTES)
        {
            return Err(limit_error());
        }
        let mut hash = Sha256::new();
        hash.update(b"open-compute/ai-search-parse-cache/v1\0");
        hash.update(source_sha256);
        hash.update(source_size.to_be_bytes());
        update_text(&mut hash, filename)?;
        update_text(&mut hash, content_type)?;
        hash.update(parser_contract_sha256);
        Ok(Self {
            digest: hash.finalize().into(),
            source_sha256,
            source_size,
            filename: filename.to_owned(),
            content_type: content_type.to_owned(),
            parser_contract_sha256,
        })
    }

    /// Opaque digest suitable for process-local coordination without exposing source identity.
    #[must_use]
    pub const fn digest(&self) -> [u8; 32] {
        self.digest
    }
}

/// Result of a cache lookup.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AiSearchParseCacheLookup {
    /// A payload passed identity and digest verification.
    Hit(Vec<u8>),
    /// No matching cache entry exists.
    Miss,
    /// A matching row was corrupt and has been discarded.
    Corrupt,
}

/// Result of a cache admission.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AiSearchParseCacheStore {
    /// The payload was admitted, possibly replacing an identical key.
    Stored {
        /// Number of least-recently-used entries removed to restore capacity.
        evicted: u64,
    },
    /// The derived payload exceeded the cache-only entry bound.
    TooLarge,
}

/// Separate SQLite cache whose absence or eviction never affects index authority.
#[derive(Debug)]
pub struct AiSearchParseCache {
    connection: Mutex<Connection>,
}

impl AiSearchParseCache {
    /// Maximum bytes reserved before attempting one cache admission.
    #[must_use]
    pub const fn maximum_entry_bytes() -> u64 {
        MAX_CACHE_ENTRY_BYTES as u64
    }

    /// Open or create one per-instance disposable cache database.
    pub fn open(path: &Path, busy_timeout_ms: u64) -> Result<Self, PlatformError> {
        if busy_timeout_ms == 0 {
            return Err(limit_error());
        }
        let parent = path.parent().ok_or_else(cache_error)?;
        crate::fs::validate_owned_dir(parent)?;
        crate::fs::ensure_file_secure(path)?;
        let file = crate::fs::open_nofollow(path, false, true)?;
        crate::fs::validate_authority_fd(&file)?;
        drop(file);
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|_| cache_error())?;
        connection
            .execute_batch(&format!(
                "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; \
                 PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF; \
                 PRAGMA busy_timeout={busy_timeout_ms};"
            ))
            .map_err(|_| cache_error())?;
        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(|_| cache_error())?;
        if version != 0 && version != CACHE_SCHEMA_VERSION {
            return Err(PlatformError::new(
                ErrorCode::SchemaUnsupported,
                "AI Search parse cache schema is unsupported",
            ));
        }
        connection
            .execute_batch(SCHEMA)
            .map_err(|_| cache_error())?;
        if version == 0 {
            connection
                .execute_batch("PRAGMA user_version=1")
                .map_err(|_| cache_error())?;
        }
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    /// Read and verify one derived payload, deleting a corrupt row before returning.
    pub fn get(
        &self,
        key: &AiSearchParseCacheKey,
        now_ms: i64,
    ) -> Result<AiSearchParseCacheLookup, PlatformError> {
        let connection = self.connection.lock().map_err(|_| cache_error())?;
        let row = connection
            .query_row(
                "SELECT source_sha256, source_size, filename, content_type,
                        parser_contract_sha256, payload, payload_sha256
                   FROM parse_cache WHERE cache_key=?1",
                [key.digest.as_slice()],
                |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Vec<u8>>(4)?,
                        row.get::<_, Vec<u8>>(5)?,
                        row.get::<_, Vec<u8>>(6)?,
                    ))
                },
            )
            .optional()
            .map_err(|_| cache_error())?;
        let Some(row) = row else {
            return Ok(AiSearchParseCacheLookup::Miss);
        };
        let valid = row.0.as_slice() == key.source_sha256
            && u64::try_from(row.1).ok() == Some(key.source_size)
            && row.2 == key.filename
            && row.3 == key.content_type
            && row.4.as_slice() == key.parser_contract_sha256
            && row.5.len() <= MAX_CACHE_ENTRY_BYTES
            && row.6.as_slice() == Sha256::digest(&row.5).as_slice();
        if !valid {
            connection
                .execute(
                    "DELETE FROM parse_cache WHERE cache_key=?1",
                    [key.digest.as_slice()],
                )
                .map_err(|_| cache_error())?;
            return Ok(AiSearchParseCacheLookup::Corrupt);
        }
        connection
            .execute(
                "UPDATE parse_cache SET last_access_at_ms=?2 WHERE cache_key=?1",
                params![key.digest.as_slice(), now_ms],
            )
            .map_err(|_| cache_error())?;
        Ok(AiSearchParseCacheLookup::Hit(row.5))
    }

    /// Admit one verified derived payload and enforce deterministic LRU bounds.
    pub fn put(
        &self,
        key: &AiSearchParseCacheKey,
        payload: &[u8],
        now_ms: i64,
    ) -> Result<AiSearchParseCacheStore, PlatformError> {
        if payload.is_empty() || payload.len() > MAX_CACHE_ENTRY_BYTES {
            return Ok(AiSearchParseCacheStore::TooLarge);
        }
        let payload_sha256: [u8; 32] = Sha256::digest(payload).into();
        let mut connection = self.connection.lock().map_err(|_| cache_error())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| cache_error())?;
        transaction
            .execute(
                "INSERT INTO parse_cache
                 (cache_key, source_sha256, source_size, filename, content_type,
                  parser_contract_sha256, payload, payload_sha256,
                  created_at_ms, last_access_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)
                 ON CONFLICT(cache_key) DO UPDATE SET
                   source_sha256=excluded.source_sha256,
                   source_size=excluded.source_size,
                   filename=excluded.filename,
                   content_type=excluded.content_type,
                   parser_contract_sha256=excluded.parser_contract_sha256,
                   payload=excluded.payload,
                   payload_sha256=excluded.payload_sha256,
                   last_access_at_ms=excluded.last_access_at_ms",
                params![
                    key.digest.as_slice(),
                    key.source_sha256,
                    i64::try_from(key.source_size).map_err(|_| limit_error())?,
                    key.filename,
                    key.content_type,
                    key.parser_contract_sha256,
                    payload,
                    payload_sha256,
                    now_ms,
                ],
            )
            .map_err(|_| cache_error())?;
        let evicted = prune(&transaction)?;
        transaction.commit().map_err(|_| cache_error())?;
        Ok(AiSearchParseCacheStore::Stored { evicted })
    }

    /// Remove one exact derived entry after payload-level validation fails.
    pub fn discard(&self, key: &AiSearchParseCacheKey) -> Result<(), PlatformError> {
        self.connection
            .lock()
            .map_err(|_| cache_error())?
            .execute(
                "DELETE FROM parse_cache WHERE cache_key=?1",
                [key.digest.as_slice()],
            )
            .map_err(|_| cache_error())?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn corrupt_payload(&self, digest: [u8; 32]) {
        self.connection
            .lock()
            .expect("cache lock")
            .execute(
                "UPDATE parse_cache SET payload=X'00' WHERE cache_key=?1",
                [digest.as_slice()],
            )
            .expect("corrupt cache payload");
    }
}

fn prune(transaction: &rusqlite::Transaction<'_>) -> Result<u64, PlatformError> {
    let mut evicted = 0_u64;
    loop {
        let (entries, bytes): (i64, i64) = transaction
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(length(payload)), 0) FROM parse_cache",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|_| cache_error())?;
        if entries <= MAX_CACHE_ENTRIES && bytes <= MAX_CACHE_BYTES {
            return Ok(evicted);
        }
        let removed = transaction
            .execute(
                "DELETE FROM parse_cache WHERE cache_key=(
                   SELECT cache_key FROM parse_cache
                   ORDER BY last_access_at_ms, created_at_ms, cache_key LIMIT 1)",
                [],
            )
            .map_err(|_| cache_error())?;
        if removed != 1 {
            return Err(cache_error());
        }
        evicted = evicted.saturating_add(1);
    }
}

fn update_text(hash: &mut Sha256, value: &str) -> Result<(), PlatformError> {
    let length = u64::try_from(value.len()).map_err(|_| limit_error())?;
    hash.update(length.to_be_bytes());
    hash.update(value.as_bytes());
    Ok(())
}

fn valid_text(value: &str, maximum: usize) -> bool {
    !value.is_empty() && value.len() <= maximum && !value.chars().any(char::is_control)
}

fn cache_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::CacheEntryCorrupt,
        "AI Search parse cache is unavailable or corrupt",
    )
}

fn limit_error() -> PlatformError {
    PlatformError::new(
        ErrorCode::LimitInvalid,
        "AI Search parse cache input exceeds a limit",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache() -> (tempfile::TempDir, AiSearchParseCache) {
        let directory = tempfile::tempdir().expect("tempdir");
        let cache = AiSearchParseCache::open(&directory.path().join("cache.sqlite"), 1_000)
            .expect("open cache");
        (directory, cache)
    }

    fn key(filename: &str, contract: [u8; 32]) -> AiSearchParseCacheKey {
        AiSearchParseCacheKey::new([7; 32], 12, filename, "text/plain", contract).expect("key")
    }

    #[test]
    fn cache_survives_reopen_and_identity_changes_miss() {
        let (directory, cache) = cache();
        let source_key = key("source.txt", [8; 32]);
        assert_eq!(
            cache.get(&source_key, 1).unwrap(),
            AiSearchParseCacheLookup::Miss
        );
        assert_eq!(
            cache
                .put(&source_key, br#"{"markdown":"hello"}"#, 2)
                .unwrap(),
            AiSearchParseCacheStore::Stored { evicted: 0 }
        );
        drop(cache);
        let cache = AiSearchParseCache::open(&directory.path().join("cache.sqlite"), 1_000)
            .expect("reopen cache");
        assert!(matches!(
            cache.get(&source_key, 3).unwrap(),
            AiSearchParseCacheLookup::Hit(_)
        ));
        assert_eq!(
            cache.get(&key("renamed.txt", [8; 32]), 3).unwrap(),
            AiSearchParseCacheLookup::Miss
        );
        assert_eq!(
            cache.get(&key("source.txt", [9; 32]), 3).unwrap(),
            AiSearchParseCacheLookup::Miss
        );
        assert_eq!(
            cache
                .get(
                    &AiSearchParseCacheKey::new([9; 32], 12, "source.txt", "text/plain", [8; 32],)
                        .unwrap(),
                    3,
                )
                .unwrap(),
            AiSearchParseCacheLookup::Miss
        );
        assert_eq!(
            cache
                .get(
                    &AiSearchParseCacheKey::new(
                        [7; 32],
                        12,
                        "source.txt",
                        "text/markdown",
                        [8; 32],
                    )
                    .unwrap(),
                    3,
                )
                .unwrap(),
            AiSearchParseCacheLookup::Miss
        );
    }

    #[test]
    fn corrupt_payload_is_discarded_and_oversized_value_is_not_admitted() {
        let (_directory, cache) = cache();
        let key = key("source.txt", [8; 32]);
        cache.put(&key, b"valid", 1).unwrap();
        cache.corrupt_payload(key.digest());
        assert_eq!(
            cache.get(&key, 2).unwrap(),
            AiSearchParseCacheLookup::Corrupt
        );
        assert_eq!(cache.get(&key, 3).unwrap(), AiSearchParseCacheLookup::Miss);
        assert_eq!(
            cache
                .put(&key, &vec![0; MAX_CACHE_ENTRY_BYTES + 1], 4)
                .unwrap(),
            AiSearchParseCacheStore::TooLarge
        );
    }

    #[test]
    fn least_recently_used_entry_is_evicted_at_capacity() {
        let (_directory, cache) = cache();
        for ordinal in 0..=MAX_CACHE_ENTRIES {
            let filename = format!("source-{ordinal}.txt");
            cache
                .put(&key(&filename, [8; 32]), b"value", ordinal)
                .expect("cache value");
        }
        assert_eq!(
            cache.get(&key("source-0.txt", [8; 32]), 1_000).unwrap(),
            AiSearchParseCacheLookup::Miss
        );
        assert!(matches!(
            cache
                .get(
                    &key(&format!("source-{MAX_CACHE_ENTRIES}.txt"), [8; 32]),
                    1_000,
                )
                .unwrap(),
            AiSearchParseCacheLookup::Hit(_)
        ));
    }
}
