//! SQLite row decoding, policy validation, and bounded authority helpers.

use super::engine::{CACHE_DATABASE_SCHEMA_VERSION, FORMAT, map_sql};
use super::model::{
    CacheBodyRef, CacheHeader, CachePurge, CacheStoredResponse, corrupt, limit_error,
    protocol_error, validate_headers, validate_tags, validate_vary,
};
use open_compute_core::{AccountId, PlatformError, ResponseCacheConfig, WorkerId};
use rand::RngCore as _;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use sha2::{Digest as _, Sha256};

#[derive(Debug)]
pub(super) struct Candidate {
    pub(super) id: i64,
    pub(super) identity_json: Vec<u8>,
    pub(super) vary_fingerprint: [u8; 32],
    pub(super) vary_json: Vec<u8>,
    status: u16,
    headers_json: Vec<u8>,
    body_sha256: String,
    body_size: u64,
    fresh_until_ms: i64,
    swr_until_ms: i64,
    sie_until_ms: i64,
    pub(super) generation: u64,
    pub(super) updated_at_ms: i64,
}

impl Candidate {
    pub(super) fn response(&self) -> Result<CacheStoredResponse, PlatformError> {
        let headers: Vec<CacheHeader> =
            serde_json::from_slice(&self.headers_json).map_err(|_| corrupt())?;
        let vary: Vec<String> = serde_json::from_slice(&self.vary_json).map_err(|_| corrupt())?;
        Ok(CacheStoredResponse {
            status: self.status,
            headers,
            body: CacheBodyRef {
                sha256: self.body_sha256.clone(),
                size: self.body_size,
            },
            vary,
            tags: Vec::new(),
            fresh_until_ms: self.fresh_until_ms,
            stale_while_revalidate_until_ms: self.swr_until_ms,
            stale_if_error_until_ms: self.sie_until_ms,
            generation: self.generation,
        })
    }
}

pub(super) fn load_candidates(
    connection: &Connection,
    base_hash: &[u8; 32],
) -> Result<Vec<Candidate>, PlatformError> {
    let mut statement = connection
        .prepare(
            "SELECT id, identity_json, vary_fingerprint, vary_json, status, headers_json,
                body_sha256, body_size, fresh_until_ms, swr_until_ms, sie_until_ms, generation,
                updated_at_ms
         FROM cache_entries WHERE base_hash = ?1 ORDER BY updated_at_ms DESC, id DESC LIMIT 257",
        )
        .map_err(map_sql)?;
    let rows = statement
        .query_map([base_hash.as_slice()], |row| {
            let digest: Vec<u8> = row.get(2)?;
            let vary_fingerprint: [u8; 32] = digest
                .try_into()
                .map_err(|_| rusqlite::Error::InvalidQuery)?;
            Ok(Candidate {
                id: row.get(0)?,
                identity_json: row.get(1)?,
                vary_fingerprint,
                vary_json: row.get(3)?,
                status: u16::try_from(row.get::<_, i64>(4)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                headers_json: row.get(5)?,
                body_sha256: row.get(6)?,
                body_size: u64::try_from(row.get::<_, i64>(7)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                fresh_until_ms: row.get(8)?,
                swr_until_ms: row.get(9)?,
                sie_until_ms: row.get(10)?,
                generation: u64::try_from(row.get::<_, i64>(11)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                updated_at_ms: row.get(12)?,
            })
        })
        .map_err(map_sql)?;
    let candidates = rows
        .map(|row| row.map_err(map_sql))
        .collect::<Result<Vec<_>, _>>()?;
    if candidates.len() > 256 {
        return Err(corrupt());
    }
    Ok(candidates)
}

pub(super) fn claim_refresh(
    connection: &mut Connection,
    entry_id: i64,
    generation: u64,
    now_ms: i64,
    lease_ms: u64,
) -> Result<Option<String>, PlatformError> {
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(map_sql)?;
    tx.execute(
        "DELETE FROM cache_refresh_leases WHERE deadline_ms <= ?1",
        [now_ms],
    )
    .map_err(map_sql)?;
    let exists: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM cache_refresh_leases WHERE entry_id = ?1)",
            [entry_id],
            |row| row.get(0),
        )
        .map_err(map_sql)?;
    if exists {
        tx.commit().map_err(map_sql)?;
        return Ok(None);
    }
    let mut random = [0_u8; 16];
    rand::rng().fill_bytes(&mut random);
    let token = hex::encode(random);
    let deadline = now_ms.saturating_add(i64::try_from(lease_ms).unwrap_or(i64::MAX));
    tx.execute(
        "INSERT INTO cache_refresh_leases(entry_id, token, deadline_ms, base_generation)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            entry_id,
            token,
            deadline,
            i64::try_from(generation).map_err(|_| corrupt())?
        ],
    )
    .map_err(map_sql)?;
    tx.commit().map_err(map_sql)?;
    Ok(Some(token))
}

pub(super) fn validate_response(
    response: &CacheStoredResponse,
    config: &ResponseCacheConfig,
    now_ms: i64,
) -> Result<(), PlatformError> {
    if !(200..=599).contains(&response.status)
        || now_ms < 0
        || response.fresh_until_ms < now_ms
        || response.stale_while_revalidate_until_ms < response.fresh_until_ms
        || response.stale_if_error_until_ms < response.fresh_until_ms
        || response.generation == 0
        || response.body.size > config.max_object_bytes
        || response.body.sha256.len() != 64
        || response
            .body
            .sha256
            .bytes()
            .any(|byte| !byte.is_ascii_hexdigit() || byte.is_ascii_uppercase())
    {
        return Err(protocol_error());
    }
    let maximum_deadline = now_ms.saturating_add(
        i64::try_from(config.max_ttl_seconds.saturating_mul(1_000)).unwrap_or(i64::MAX),
    );
    if response.fresh_until_ms > maximum_deadline
        || response.stale_while_revalidate_until_ms > maximum_deadline
        || response.stale_if_error_until_ms > maximum_deadline
    {
        return Err(limit_error());
    }
    validate_headers(&response.headers, config.max_header_bytes as usize)?;
    validate_vary(&response.vary)?;
    validate_tags(&response.tags, config.max_tags_per_entry as usize)
}

pub(super) fn canonical_purge(
    purge: &CachePurge,
    config: &ResponseCacheConfig,
) -> Result<CachePurge, PlatformError> {
    if purge.tags.len() > config.max_tags_per_entry as usize || purge.path_prefixes.len() > 64 {
        return Err(protocol_error());
    }
    let mut tags = purge
        .tags
        .iter()
        .map(|tag| tag.trim().to_ascii_lowercase())
        .collect::<Vec<_>>();
    tags.sort();
    tags.dedup();
    validate_tags(&tags, config.max_tags_per_entry as usize)?;
    let path_prefixes = purge
        .path_prefixes
        .iter()
        .map(|prefix| {
            if prefix.is_empty()
                || prefix.len() > config.max_url_bytes as usize
                || prefix.bytes().any(|byte| byte.is_ascii_control())
            {
                return Err(protocol_error());
            }
            if prefix.starts_with('/') {
                return Ok(prefix.clone());
            }
            let url = url::Url::parse(prefix).map_err(|_| protocol_error())?;
            if !matches!(url.scheme(), "http" | "https") || url.fragment().is_some() {
                return Err(protocol_error());
            }
            super::model::canonical_url(url).map_err(|_| protocol_error())
        })
        .collect::<Result<Vec<_>, _>>()?;
    if !purge.purge_everything && path_prefixes.is_empty() && tags.is_empty() {
        return Err(protocol_error());
    }
    Ok(CachePurge {
        tags,
        path_prefixes,
        purge_everything: purge.purge_everything,
    })
}

pub(super) fn enforce_variant_limit(
    tx: &rusqlite::Transaction<'_>,
    base_hash: &[u8; 32],
    keep_id: i64,
    maximum: u16,
) -> Result<(), PlatformError> {
    let count: u64 = tx
        .query_row(
            "SELECT COUNT(*) FROM cache_entries WHERE base_hash = ?1",
            [base_hash.as_slice()],
            |row| row.get(0),
        )
        .map_err(map_sql)?;
    let remove = count.saturating_sub(u64::from(maximum));
    if remove > 0 {
        tx.execute(
            "DELETE FROM cache_entries WHERE id IN (
               SELECT id FROM cache_entries WHERE base_hash = ?1 AND id != ?2
               ORDER BY updated_at_ms ASC, id ASC LIMIT ?3
             )",
            params![
                base_hash.as_slice(),
                keep_id,
                i64::try_from(remove).map_err(|_| limit_error())?
            ],
        )
        .map_err(map_sql)?;
    }
    Ok(())
}

pub(super) fn enforce_byte_quota(
    tx: &rusqlite::Transaction<'_>,
    keep_id: i64,
    maximum: u64,
) -> Result<(), PlatformError> {
    let mut total: u64 = tx
        .query_row(
            "SELECT COALESCE(SUM(body_size), 0) FROM cache_entries",
            [],
            |row| row.get(0),
        )
        .map_err(map_sql)?;
    if total <= maximum {
        return Ok(());
    }
    let mut statement = tx
        .prepare(
            "SELECT id, body_size FROM cache_entries WHERE id != ?1
             ORDER BY updated_at_ms ASC, id ASC",
        )
        .map_err(map_sql)?;
    let rows = statement
        .query_map([keep_id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, u64>(1)?))
        })
        .map_err(map_sql)?;
    let mut evict = Vec::new();
    for row in rows {
        let (id, size) = row.map_err(map_sql)?;
        evict.push(id);
        total = total.saturating_sub(size);
        if total <= maximum {
            break;
        }
    }
    drop(statement);
    if total > maximum {
        return Err(limit_error());
    }
    for id in evict {
        tx.execute("DELETE FROM cache_entries WHERE id = ?1", [id])
            .map_err(map_sql)?;
    }
    Ok(())
}

pub(super) fn current_fence(connection: &Connection) -> Result<u64, PlatformError> {
    let bytes: Vec<u8> = connection
        .query_row(
            "SELECT value FROM cache_meta WHERE key = 'fence_generation'",
            [],
            |row| row.get(0),
        )
        .map_err(map_sql)?;
    std::str::from_utf8(&bytes)
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value| *value > 0)
        .ok_or_else(corrupt)
}

pub(super) fn verify_identity(
    connection: &Connection,
    account: AccountId,
    worker: WorkerId,
) -> Result<(), PlatformError> {
    for (key, expected) in [
        ("format", FORMAT.to_vec()),
        (
            "schema_version",
            CACHE_DATABASE_SCHEMA_VERSION.to_string().into_bytes(),
        ),
        ("account_id", account.to_string().into_bytes()),
        ("worker_id", worker.to_string().into_bytes()),
    ] {
        let value: Option<Vec<u8>> = connection
            .query_row(
                "SELECT value FROM cache_meta WHERE key = ?1",
                [key],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_sql)?;
        let value = value.ok_or_else(corrupt)?;
        if value != expected {
            return Err(corrupt());
        }
    }
    current_fence(connection)?;
    Ok(())
}

pub(super) fn verify_schema(connection: &Connection) -> Result<(), PlatformError> {
    for table in [
        "cache_entries",
        "cache_tags",
        "cache_refresh_leases",
        "cache_tombstones",
    ] {
        let sql: Option<String> = connection
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [table],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_sql)?;
        if sql.is_none_or(|value| !value.to_ascii_uppercase().contains("STRICT")) {
            return Err(corrupt());
        }
    }
    let stored: Option<Vec<u8>> = connection
        .query_row(
            "SELECT value FROM cache_meta WHERE key = 'schema_sha256'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(map_sql)?;
    let stored = stored.ok_or_else(corrupt)?;
    if stored != schema_sha256(connection)?.as_bytes() {
        return Err(corrupt());
    }
    Ok(())
}

pub(super) fn schema_sha256(connection: &Connection) -> Result<String, PlatformError> {
    let mut statement = connection
        .prepare(
            "SELECT type, name, tbl_name, COALESCE(sql, '') FROM sqlite_schema
             WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name",
        )
        .map_err(map_sql)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(map_sql)?;
    let mut hasher = Sha256::new();
    hasher.update(b"open-compute/response-cache-schema/v1\0");
    for row in rows {
        let (kind, name, table, sql) = row.map_err(map_sql)?;
        for value in [kind, name, table, sql] {
            hasher.update(
                u64::try_from(value.len())
                    .map_err(|_| corrupt())?
                    .to_be_bytes(),
            );
            hasher.update(value.as_bytes());
        }
    }
    Ok(hex::encode(hasher.finalize()))
}
