//! Worker-local cache SQLite engine and bounded handle manager.

use super::authority::{
    canonical_purge, claim_refresh, current_fence, enforce_byte_quota, enforce_variant_limit,
    load_candidates, schema_sha256, validate_response, verify_identity, verify_schema,
};
use super::model::{
    CacheBodyRef, CacheIdentity, CacheLookup, CacheLookupStatus, CachePurge, CachePut,
    CacheSurface, corrupt, limit_error, protocol_error, validate_request_headers, validate_vary,
    vary_fingerprint,
};
use super::paths::CachePaths;
use crate::fs;
use open_compute_core::{AccountId, ErrorCode, PlatformError, ResponseCacheConfig, WorkerId};
use rusqlite::{
    Connection, Error as SqlError, ErrorCode as SqlErrorCode, OpenFlags, TransactionBehavior,
    params,
};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Current per-Worker cache database schema.
pub const CACHE_DATABASE_SCHEMA_VERSION: u32 = 1;
pub(super) const FORMAT: &[u8] = b"open-compute-response-cache";
const CACHE_TOMBSTONE_RETENTION: u64 = 64;

/// Bounded operator and metrics summary for one or more response-cache databases.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CacheStats {
    /// Reachable cache entries.
    pub entries: u64,
    /// Logical immutable body bytes referenced by entries.
    pub body_bytes: u64,
    /// On-disk SQLite metadata bytes.
    pub metadata_bytes: u64,
    /// Live stale-refresh leases.
    pub active_refreshes: u64,
    /// Process-local database handles.
    pub open_databases: u64,
}

impl CacheStats {
    fn merge(&mut self, other: Self) {
        self.entries = self.entries.saturating_add(other.entries);
        self.body_bytes = self.body_bytes.saturating_add(other.body_bytes);
        self.metadata_bytes = self.metadata_bytes.saturating_add(other.metadata_bytes);
        self.active_refreshes = self.active_refreshes.saturating_add(other.active_refreshes);
        self.open_databases = self.open_databases.saturating_add(other.open_databases);
    }
}

/// Direct engine for one stable account/Worker cache identity.
#[derive(Clone, Debug)]
pub struct CacheEngine {
    path: PathBuf,
    account_id: AccountId,
    worker_id: WorkerId,
    config: ResponseCacheConfig,
}

impl CacheEngine {
    /// Open or initialize one Worker cache database below a validated Worker directory.
    pub fn open_or_create(
        path: PathBuf,
        account_id: AccountId,
        worker_id: WorkerId,
        created_at_ms: i64,
        config: ResponseCacheConfig,
    ) -> Result<Self, PlatformError> {
        if created_at_ms < 0 {
            return Err(protocol_error());
        }
        let engine = Self {
            path,
            account_id,
            worker_id,
            config,
        };
        if engine.path.exists() || std::fs::symlink_metadata(&engine.path).is_ok() {
            engine.verify()?;
        } else {
            engine.create(created_at_ms)?;
        }
        Ok(engine)
    }

    /// Verify path, embedded identity, schema shape, and SQLite integrity.
    pub fn verify(&self) -> Result<(), PlatformError> {
        let connection = self.open_connection(false)?;
        verify_identity(&connection, self.account_id, self.worker_id)?;
        verify_schema(&connection)?;
        let result: String = connection
            .query_row("PRAGMA quick_check", [], |row| row.get(0))
            .map_err(map_sql)?;
        if result != "ok" {
            return Err(corrupt());
        }
        Ok(())
    }

    /// Capture the current purge fence before a streamed body upload begins.
    pub fn prepare_put(&self, identity: &CacheIdentity) -> Result<u64, PlatformError> {
        self.validate_identity(identity)?;
        let connection = self.open_connection(false)?;
        current_fence(&connection)
    }

    /// Capture the purge fence and allocate a metadata generation above every current variant.
    pub fn prepare_put_generation(
        &self,
        identity: &CacheIdentity,
    ) -> Result<(u64, u64), PlatformError> {
        self.validate_identity(identity)?;
        let connection = self.open_connection(false)?;
        let fence = current_fence(&connection)?;
        let base_hash = identity.base_hash()?;
        let current: u64 = connection
            .query_row(
                "SELECT COALESCE(MAX(generation), 0) FROM cache_entries WHERE base_hash = ?1",
                [base_hash.as_slice()],
                |row| row.get(0),
            )
            .map_err(map_sql)?;
        Ok((fence, current.checked_add(1).ok_or_else(corrupt)?))
    }

    /// Read a matching variant and claim a bounded stale refresh lease when eligible.
    pub fn lookup(
        &self,
        identity: &CacheIdentity,
        request_headers: &BTreeMap<String, String>,
        now_ms: i64,
    ) -> Result<CacheLookup, PlatformError> {
        self.validate_identity(identity)?;
        validate_request_headers(request_headers, self.config.max_header_bytes as usize)?;
        if now_ms < 0 {
            return Err(protocol_error());
        }
        let mut connection = self.open_connection(true)?;
        let fence = current_fence(&connection)?;
        let base_hash = identity.base_hash()?;
        let identity_bytes = identity.canonical_bytes()?;
        let candidates = load_candidates(&connection, &base_hash)?;
        for candidate in candidates {
            if candidate.identity_json != identity_bytes {
                return Err(corrupt());
            }
            let vary: Vec<String> =
                serde_json::from_slice(&candidate.vary_json).map_err(|_| corrupt())?;
            validate_vary(&vary).map_err(|_| corrupt())?;
            if vary_fingerprint(&vary, request_headers) != candidate.vary_fingerprint {
                continue;
            }
            let response = candidate.response()?;
            validate_response(&response, &self.config, candidate.updated_at_ms)
                .map_err(|_| corrupt())?;
            if now_ms < response.fresh_until_ms {
                return Ok(CacheLookup {
                    status: CacheLookupStatus::Hit,
                    response: Some(response),
                    fence_generation: fence,
                    refresh_token: None,
                });
            }
            if identity.surface == CacheSurface::Automatic
                && now_ms < response.stale_while_revalidate_until_ms
            {
                let token = claim_refresh(
                    &mut connection,
                    candidate.id,
                    response.generation,
                    now_ms,
                    self.config.refresh_lease_ms,
                )?;
                return Ok(CacheLookup {
                    status: if token.is_some() {
                        CacheLookupStatus::Updating
                    } else {
                        CacheLookupStatus::Stale
                    },
                    response: Some(response),
                    fence_generation: fence,
                    refresh_token: token,
                });
            }
            if identity.surface == CacheSurface::Automatic
                && now_ms < response.stale_if_error_until_ms
            {
                return Ok(CacheLookup {
                    status: CacheLookupStatus::StaleIfError,
                    response: Some(response),
                    fence_generation: fence,
                    refresh_token: None,
                });
            }
            connection
                .execute("DELETE FROM cache_entries WHERE id = ?1", [candidate.id])
                .map_err(map_sql)?;
            return Ok(CacheLookup {
                status: CacheLookupStatus::Expired,
                response: None,
                fence_generation: fence,
                refresh_token: None,
            });
        }
        Ok(CacheLookup {
            status: CacheLookupStatus::Miss,
            response: None,
            fence_generation: fence,
            refresh_token: None,
        })
    }

    /// Atomically publish cache metadata only if purge and refresh fences remain current.
    pub fn put(&self, put: &CachePut) -> Result<(), PlatformError> {
        self.validate_identity(&put.identity)?;
        validate_request_headers(&put.request_headers, self.config.max_header_bytes as usize)?;
        validate_response(&put.response, &self.config, put.now_ms)?;
        let identity_json = put.identity.canonical_bytes()?;
        let base_hash = put.identity.base_hash()?;
        let vary_fingerprint = vary_fingerprint(&put.response.vary, &put.request_headers);
        let headers_json =
            serde_json::to_vec(&put.response.headers).map_err(|_| protocol_error())?;
        let vary_json = serde_json::to_vec(&put.response.vary).map_err(|_| protocol_error())?;
        let mut connection = self.open_connection(true)?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sql)?;
        let fence = current_fence(&tx)?;
        if fence != put.expected_fence_generation {
            return Err(stale_write());
        }
        if let Some(token) = &put.refresh_token {
            let valid: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM cache_refresh_leases
                     WHERE entry_id IN (SELECT id FROM cache_entries WHERE base_hash = ?1)
                       AND token = ?2 AND base_generation < ?3 AND deadline_ms > ?4)",
                    params![
                        base_hash.as_slice(),
                        token,
                        i64::try_from(put.response.generation).map_err(|_| protocol_error())?,
                        put.now_ms
                    ],
                    |row| row.get(0),
                )
                .map_err(map_sql)?;
            if !valid {
                return Err(stale_write());
            }
        }
        tx.execute(
            "INSERT INTO cache_entries
             (base_hash, identity_json, canonical_url, vary_fingerprint, vary_json, status,
              headers_json, body_sha256, body_size, fresh_until_ms, swr_until_ms, sie_until_ms,
              generation, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?14)
             ON CONFLICT(base_hash, vary_fingerprint) DO UPDATE SET
               identity_json=excluded.identity_json, canonical_url=excluded.canonical_url,
               vary_json=excluded.vary_json, status=excluded.status, headers_json=excluded.headers_json,
               body_sha256=excluded.body_sha256, body_size=excluded.body_size,
               fresh_until_ms=excluded.fresh_until_ms, swr_until_ms=excluded.swr_until_ms,
               sie_until_ms=excluded.sie_until_ms, generation=excluded.generation,
               updated_at_ms=excluded.updated_at_ms",
            params![
                base_hash.as_slice(), identity_json, put.identity.canonical_url,
                vary_fingerprint.as_slice(), vary_json, i64::from(put.response.status), headers_json,
                put.response.body.sha256, i64::try_from(put.response.body.size).map_err(|_| limit_error())?,
                put.response.fresh_until_ms, put.response.stale_while_revalidate_until_ms,
                put.response.stale_if_error_until_ms,
                i64::try_from(put.response.generation).map_err(|_| protocol_error())?, put.now_ms,
            ],
        ).map_err(map_sql)?;
        let entry_id: i64 = tx
            .query_row(
                "SELECT id FROM cache_entries WHERE base_hash = ?1 AND vary_fingerprint = ?2",
                params![base_hash.as_slice(), vary_fingerprint.as_slice()],
                |row| row.get(0),
            )
            .map_err(map_sql)?;
        tx.execute("DELETE FROM cache_tags WHERE entry_id = ?1", [entry_id])
            .map_err(map_sql)?;
        for tag in &put.response.tags {
            tx.execute(
                "INSERT INTO cache_tags(entry_id, tag) VALUES (?1, ?2)",
                params![entry_id, tag],
            )
            .map_err(map_sql)?;
        }
        tx.execute(
            "DELETE FROM cache_refresh_leases WHERE entry_id = ?1",
            [entry_id],
        )
        .map_err(map_sql)?;
        enforce_variant_limit(&tx, &base_hash, entry_id, self.config.max_variants_per_key)?;
        enforce_byte_quota(&tx, entry_id, self.config.max_bytes_per_worker)?;
        tx.commit().map_err(map_sql)
    }

    /// Delete the currently matching explicit Cache API variant.
    pub fn delete(
        &self,
        identity: &CacheIdentity,
        request_headers: &BTreeMap<String, String>,
    ) -> Result<bool, PlatformError> {
        self.validate_identity(identity)?;
        if identity.surface == CacheSurface::Automatic {
            return Err(protocol_error());
        }
        validate_request_headers(request_headers, self.config.max_header_bytes as usize)?;
        let mut connection = self.open_connection(true)?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sql)?;
        let identity_json = identity.canonical_bytes()?;
        let base_hash = identity.base_hash()?;
        let mut deleted = false;
        for candidate in load_candidates(&tx, &base_hash)? {
            if candidate.identity_json != identity_json {
                return Err(corrupt());
            }
            let vary: Vec<String> =
                serde_json::from_slice(&candidate.vary_json).map_err(|_| corrupt())?;
            if vary_fingerprint(&vary, request_headers) == candidate.vary_fingerprint {
                deleted |= tx
                    .execute("DELETE FROM cache_entries WHERE id = ?1", [candidate.id])
                    .map_err(map_sql)?
                    == 1;
            }
        }
        tx.commit().map_err(map_sql)?;
        Ok(deleted)
    }

    /// Fence all in-flight writes, delete the selected variants, and return the deleted count.
    pub fn purge(&self, purge: &CachePurge, now_ms: i64) -> Result<u64, PlatformError> {
        let purge = canonical_purge(purge, &self.config)?;
        if now_ms < 0 {
            return Err(protocol_error());
        }
        let mut connection = self.open_connection(true)?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sql)?;
        let next_fence = current_fence(&tx)?.checked_add(1).ok_or_else(corrupt)?;
        tx.execute(
            "UPDATE cache_meta SET value = ?1 WHERE key = 'fence_generation'",
            [next_fence.to_string().into_bytes()],
        )
        .map_err(map_sql)?;
        tx.execute(
            "INSERT INTO cache_tombstones(generation, created_at_ms) VALUES (?1, ?2)",
            params![i64::try_from(next_fence).map_err(|_| corrupt())?, now_ms],
        )
        .map_err(map_sql)?;
        let tombstone_floor = next_fence.saturating_sub(CACHE_TOMBSTONE_RETENTION);
        tx.execute(
            "DELETE FROM cache_tombstones WHERE generation <= ?1",
            [i64::try_from(tombstone_floor).map_err(|_| corrupt())?],
        )
        .map_err(map_sql)?;
        let mut statement = tx
            .prepare("SELECT id, canonical_url FROM cache_entries ORDER BY id")
            .map_err(map_sql)?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(map_sql)?;
        let mut selected = Vec::new();
        for row in rows {
            let (id, url) = row.map_err(map_sql)?;
            let path_match = purge.path_prefixes.iter().any(|prefix| {
                if prefix.starts_with('/') {
                    url::Url::parse(&url)
                        .ok()
                        .is_some_and(|value| value.path().starts_with(prefix))
                } else {
                    url.starts_with(prefix)
                }
            });
            let mut tag_match = false;
            for tag in &purge.tags {
                if tx
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM cache_tags WHERE entry_id = ?1 AND tag = ?2)",
                        params![id, tag],
                        |value| value.get::<_, bool>(0),
                    )
                    .map_err(map_sql)?
                {
                    tag_match = true;
                    break;
                }
            }
            if purge.purge_everything || path_match || tag_match {
                selected.push(id);
            }
        }
        drop(statement);
        for id in &selected {
            tx.execute("DELETE FROM cache_entries WHERE id = ?1", [id])
                .map_err(map_sql)?;
        }
        tx.commit().map_err(map_sql)?;
        Ok(selected.len() as u64)
    }

    /// Enumerate every immutable body reference currently reachable from this database.
    pub fn referenced_bodies(&self) -> Result<Vec<CacheBodyRef>, PlatformError> {
        let connection = self.open_connection(false)?;
        let mut statement = connection.prepare(
            "SELECT DISTINCT body_sha256, body_size FROM cache_entries ORDER BY body_sha256, body_size",
        ).map_err(map_sql)?;
        let rows = statement
            .query_map([], |row| {
                Ok(CacheBodyRef {
                    sha256: row.get(0)?,
                    size: u64::try_from(row.get::<_, i64>(1)?)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                })
            })
            .map_err(map_sql)?;
        rows.map(|row| row.map_err(map_sql)).collect()
    }

    /// Read low-cardinality size and lifecycle counters from the verified database.
    pub fn stats(&self, now_ms: i64) -> Result<CacheStats, PlatformError> {
        if now_ms < 0 {
            return Err(protocol_error());
        }
        let connection = self.open_connection(false)?;
        let (entries, body_bytes): (i64, i64) = connection
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(body_size), 0) FROM cache_entries",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(map_sql)?;
        let active_refreshes: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM cache_refresh_leases WHERE deadline_ms > ?1",
                [now_ms],
                |row| row.get(0),
            )
            .map_err(map_sql)?;
        Ok(CacheStats {
            entries: u64::try_from(entries).map_err(|_| corrupt())?,
            body_bytes: u64::try_from(body_bytes).map_err(|_| corrupt())?,
            metadata_bytes: std::fs::metadata(&self.path)
                .map_err(|_| unavailable())?
                .len(),
            active_refreshes: u64::try_from(active_refreshes).map_err(|_| corrupt())?,
            open_databases: 0,
        })
    }

    fn validate_identity(&self, identity: &CacheIdentity) -> Result<(), PlatformError> {
        if identity.account_id != self.account_id || identity.worker_id != self.worker_id {
            return Err(corrupt());
        }
        identity.validate(
            self.config.max_url_bytes as usize,
            self.config.max_cache_name_bytes as usize,
        )
    }

    fn create(&self, created_at_ms: i64) -> Result<(), PlatformError> {
        let parent = self.path.parent().ok_or_else(corrupt)?;
        fs::validate_owned_dir(parent)?;
        fs::ensure_file_secure(&self.path)?;
        let mut connection = Connection::open_with_flags(
            &self.path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(map_sql)?;
        connection.execute_batch(
            "PRAGMA journal_mode = DELETE;
             PRAGMA synchronous = FULL;
             PRAGMA foreign_keys = ON;
             PRAGMA trusted_schema = OFF;
             CREATE TABLE cache_meta (key TEXT PRIMARY KEY, value BLOB NOT NULL) STRICT, WITHOUT ROWID;
             CREATE TABLE cache_entries (
               id INTEGER PRIMARY KEY,
               base_hash BLOB NOT NULL CHECK(length(base_hash) = 32),
               identity_json BLOB NOT NULL CHECK(length(identity_json) BETWEEN 2 AND 65536),
               canonical_url TEXT NOT NULL CHECK(length(canonical_url) BETWEEN 1 AND 32768),
               vary_fingerprint BLOB NOT NULL CHECK(length(vary_fingerprint) = 32),
               vary_json BLOB NOT NULL CHECK(length(vary_json) BETWEEN 2 AND 8192),
               status INTEGER NOT NULL CHECK(status BETWEEN 200 AND 599),
               headers_json BLOB NOT NULL CHECK(length(headers_json) BETWEEN 2 AND 65536),
               body_sha256 TEXT NOT NULL CHECK(length(body_sha256) = 64),
               body_size INTEGER NOT NULL CHECK(body_size >= 0),
               fresh_until_ms INTEGER NOT NULL CHECK(fresh_until_ms >= 0),
               swr_until_ms INTEGER NOT NULL CHECK(swr_until_ms >= fresh_until_ms),
               sie_until_ms INTEGER NOT NULL CHECK(sie_until_ms >= fresh_until_ms),
               generation INTEGER NOT NULL CHECK(generation > 0),
               created_at_ms INTEGER NOT NULL CHECK(created_at_ms >= 0),
               updated_at_ms INTEGER NOT NULL CHECK(updated_at_ms >= created_at_ms),
               UNIQUE(base_hash, vary_fingerprint)
             ) STRICT;
             CREATE INDEX cache_entries_lru ON cache_entries(updated_at_ms, id);
             CREATE INDEX cache_entries_url ON cache_entries(canonical_url, id);
             CREATE TABLE cache_tags (
               entry_id INTEGER NOT NULL REFERENCES cache_entries(id) ON DELETE CASCADE,
               tag TEXT NOT NULL CHECK(length(tag) BETWEEN 1 AND 128),
               PRIMARY KEY(entry_id, tag)
             ) STRICT, WITHOUT ROWID;
             CREATE INDEX cache_tags_reverse ON cache_tags(tag, entry_id);
             CREATE TABLE cache_refresh_leases (
               entry_id INTEGER PRIMARY KEY REFERENCES cache_entries(id) ON DELETE CASCADE,
               token TEXT NOT NULL UNIQUE CHECK(length(token) = 32),
               deadline_ms INTEGER NOT NULL CHECK(deadline_ms > 0),
               base_generation INTEGER NOT NULL CHECK(base_generation > 0)
             ) STRICT;
             CREATE TABLE cache_tombstones (
               generation INTEGER PRIMARY KEY CHECK(generation > 0),
               created_at_ms INTEGER NOT NULL CHECK(created_at_ms >= 0)
             ) STRICT;",
        ).map_err(map_sql)?;
        let schema_sha256 = schema_sha256(&connection)?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sql)?;
        for (key, value) in [
            ("format", FORMAT.to_vec()),
            (
                "schema_version",
                CACHE_DATABASE_SCHEMA_VERSION.to_string().into_bytes(),
            ),
            ("account_id", self.account_id.to_string().into_bytes()),
            ("worker_id", self.worker_id.to_string().into_bytes()),
            ("schema_sha256", schema_sha256.into_bytes()),
            ("created_at_ms", created_at_ms.to_string().into_bytes()),
            ("fence_generation", b"1".to_vec()),
        ] {
            tx.execute(
                "INSERT INTO cache_meta(key, value) VALUES (?1, ?2)",
                params![key, value],
            )
            .map_err(map_sql)?;
        }
        tx.commit().map_err(map_sql)?;
        drop(connection);
        fs::chmod(&self.path, 0o600)?;
        let file = fs::open_nofollow(&self.path, false, true)?;
        fs::validate_authority_fd(&file)?;
        file.sync_all().map_err(|_| unavailable())?;
        fs::fsync_dir(parent)?;
        self.verify()
    }

    fn open_connection(&self, write: bool) -> Result<Connection, PlatformError> {
        fs::validate_owned_file(&self.path, true)?;
        let descriptor = fs::open_nofollow(&self.path, false, write)?;
        fs::validate_authority_fd(&descriptor)?;
        drop(descriptor);
        let connection = Connection::open_with_flags(
            &self.path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(map_sql)?;
        connection
            .busy_timeout(Duration::from_millis(self.config.busy_timeout_ms))
            .map_err(map_sql)?;
        connection
            .execute_batch(
                "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = FULL;
             PRAGMA foreign_keys = ON;
             PRAGMA trusted_schema = OFF;
             PRAGMA wal_autocheckpoint = 1000;
             PRAGMA temp_store = MEMORY;
             PRAGMA cache_size = -4096;",
            )
            .map_err(map_sql)?;
        verify_identity(&connection, self.account_id, self.worker_id)?;
        verify_schema(&connection)?;
        Ok(connection)
    }
}

fn engine_from_database(
    path: &Path,
    config: ResponseCacheConfig,
) -> Result<CacheEngine, PlatformError> {
    fs::validate_owned_file(path, true)?;
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(map_sql)?;
    let account = meta_id::<AccountId>(&connection, "account_id")?;
    let worker = meta_id::<WorkerId>(&connection, "worker_id")?;
    drop(connection);
    let engine = CacheEngine {
        path: path.to_path_buf(),
        account_id: account,
        worker_id: worker,
        config,
    };
    engine.verify()?;
    Ok(engine)
}

fn meta_id<T: std::str::FromStr>(connection: &Connection, key: &str) -> Result<T, PlatformError> {
    let bytes: Vec<u8> = connection
        .query_row(
            "SELECT value FROM cache_meta WHERE key = ?1",
            [key],
            |row| row.get(0),
        )
        .map_err(map_sql)?;
    std::str::from_utf8(&bytes)
        .ok()
        .and_then(|value| value.parse().ok())
        .ok_or_else(corrupt)
}

#[derive(Debug, Default)]
struct ManagerState {
    handles: HashMap<String, Arc<CacheEngine>>,
    lru: VecDeque<String>,
}

/// Bounded process-local cache-engine handle manager; SQLite remains authoritative.
#[derive(Debug)]
pub struct CacheManager {
    paths: CachePaths,
    config: ResponseCacheConfig,
    state: Mutex<ManagerState>,
}

impl CacheManager {
    /// Bind a handle manager to the platform cache directory.
    pub fn open(data_root: &Path, config: ResponseCacheConfig) -> Result<Self, PlatformError> {
        Ok(Self {
            paths: CachePaths::open(data_root)?,
            config,
            state: Mutex::new(ManagerState::default()),
        })
    }

    /// Open or create the stable database for one already-authorized Worker.
    pub fn engine(
        &self,
        account: AccountId,
        worker: WorkerId,
        now_ms: i64,
    ) -> Result<Arc<CacheEngine>, PlatformError> {
        let key = format!("{account}/{worker}");
        let mut state = self.state.lock().map_err(|_| unavailable())?;
        if let Some(engine) = state.handles.get(&key).cloned() {
            state.lru.retain(|value| value != &key);
            state.lru.push_back(key);
            return Ok(engine);
        }
        let directory = self.paths.ensure_worker_dir(account, worker)?;
        let engine = Arc::new(CacheEngine::open_or_create(
            directory.join("cache.sqlite"),
            account,
            worker,
            now_ms,
            self.config.clone(),
        )?);
        while state.handles.len() >= self.config.max_connections as usize {
            let Some(oldest) = state.lru.pop_front() else {
                break;
            };
            state.handles.remove(&oldest);
        }
        state.handles.insert(key.clone(), engine.clone());
        state.lru.push_back(key);
        Ok(engine)
    }

    /// Verify all databases and return their unique immutable object-body references.
    pub fn referenced_bodies(&self) -> Result<Vec<CacheBodyRef>, PlatformError> {
        let mut references = BTreeMap::<(String, u64), CacheBodyRef>::new();
        for path in self.paths.databases()? {
            let engine = engine_from_database(&path, self.config.clone())?;
            for body in engine.referenced_bodies()? {
                references.insert((body.sha256.clone(), body.size), body);
            }
        }
        Ok(references.into_values().collect())
    }

    /// Inspect one Worker without creating a database when it has never cached a response.
    pub fn worker_stats(
        &self,
        account: AccountId,
        worker: WorkerId,
        now_ms: i64,
    ) -> Result<CacheStats, PlatformError> {
        let path = self.paths.database_path(account, worker);
        match std::fs::symlink_metadata(&path) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(CacheStats::default());
            }
            Err(_) => return Err(unavailable()),
        }
        let engine = engine_from_database(&path, self.config.clone())?;
        let mut stats = engine.stats(now_ms)?;
        let key = format!("{account}/{worker}");
        stats.open_databases = u64::from(
            self.state
                .lock()
                .map_err(|_| unavailable())?
                .handles
                .contains_key(&key),
        );
        Ok(stats)
    }

    /// Inspect every verified Worker database for fixed-series process metrics.
    pub fn stats(&self, now_ms: i64) -> Result<CacheStats, PlatformError> {
        let mut stats = CacheStats::default();
        for path in self.paths.databases()? {
            stats.merge(engine_from_database(&path, self.config.clone())?.stats(now_ms)?);
        }
        stats.open_databases = self.state.lock().map_err(|_| unavailable())?.handles.len() as u64;
        Ok(stats)
    }

    /// Fence all prior writes and remove every logical entry for a Worker.
    pub fn purge_worker(
        &self,
        account: AccountId,
        worker: WorkerId,
        now_ms: i64,
    ) -> Result<u64, PlatformError> {
        let path = self.paths.database_path(account, worker);
        match std::fs::symlink_metadata(&path) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(_) => return Err(unavailable()),
        }
        engine_from_database(&path, self.config.clone())?.purge(
            &CachePurge {
                tags: Vec::new(),
                path_prefixes: Vec::new(),
                purge_everything: true,
            },
            now_ms,
        )
    }
}

fn stale_write() -> PlatformError {
    PlatformError::new(
        ErrorCode::CacheResultUnknown,
        "cache write lost its purge or refresh fence",
    )
}

fn unavailable() -> PlatformError {
    PlatformError::new(
        ErrorCode::CacheUnavailable,
        "cache metadata authority is unavailable",
    )
}

#[allow(clippy::needless_pass_by_value)]
pub(super) fn map_sql(error: SqlError) -> PlatformError {
    match error {
        SqlError::SqliteFailure(inner, _) => match inner.code {
            SqlErrorCode::DatabaseCorrupt | SqlErrorCode::NotADatabase => corrupt(),
            SqlErrorCode::DatabaseBusy | SqlErrorCode::DatabaseLocked => unavailable(),
            SqlErrorCode::DiskFull => limit_error(),
            _ => unavailable(),
        },
        _ => unavailable(),
    }
}
