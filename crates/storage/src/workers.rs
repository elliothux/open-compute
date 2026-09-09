//! Typed P0.2 control-plane repository.

use crate::{
    CatalogCursor, CatalogCursorValue, CatalogDirection, CatalogListPage, CatalogSort, ControlDb,
    SecretEnvelope, encode_catalog_cursor, invalid_catalog_cursor, normalize_catalog_limit,
    search_as_worker_id,
};
use open_compute_core::{
    AccountId, DeploymentId, ErrorCode, PlatformError, RequestId, VersionId, WorkerId,
};
use rusqlite::types::Value;
use rusqlite::{OptionalExtension, Transaction, params, params_from_iter};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::str::FromStr;
use uuid::Uuid;

mod catalog;
mod deployments;
mod idempotency;
mod lifecycle;
mod model;
mod retention;
mod version_create;

pub use model::*;

pub(crate) fn validate_worker_name(name: &str) -> Result<(), PlatformError> {
    let bytes = name.as_bytes();
    if bytes.is_empty()
        || bytes.len() > 63
        || !bytes[0].is_ascii_alphanumeric()
        || !bytes[bytes.len() - 1].is_ascii_alphanumeric()
        || bytes
            .iter()
            .any(|b| !b.is_ascii_lowercase() && !b.is_ascii_digit() && *b != b'-')
    {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "Worker name must be a lowercase ASCII slug",
        ));
    }
    Ok(())
}

pub(crate) fn validate_referrer(kind: &str, ref_id: &str) -> Result<(), PlatformError> {
    let valid = |value: &str, max: usize| {
        !value.is_empty()
            && value.len() <= max
            && value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b':' | b'/')
            })
    };
    if !valid(kind, 64) || !valid(ref_id, 256) {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "version referrer token is invalid",
        ));
    }
    Ok(())
}

pub(crate) fn idempotency_ref_id(account_id: AccountId, scope: &str, key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"open-compute/version-referrer/v1\0");
    hasher.update(account_id.to_string().as_bytes());
    hasher.update([0]);
    hasher.update(scope.as_bytes());
    hasher.update([0]);
    hasher.update(key.as_bytes());
    hex::encode(hasher.finalize())
}

pub(crate) fn validate_exact_route(
    hostname: &str,
    path: &str,
    entrypoint: Option<&str>,
) -> Result<(), PlatformError> {
    if hostname.is_empty()
        || hostname.len() > 253
        || hostname.bytes().any(|byte| {
            !byte.is_ascii_lowercase()
                && !byte.is_ascii_digit()
                && !matches!(byte, b'.' | b'-' | b':' | b'[' | b']')
        })
        || path.is_empty()
        || path.len() > 2048
        || !path.starts_with('/')
        || path.contains(['?', '#', '\0'])
        || entrypoint.is_some_and(|value| {
            value.is_empty()
                || value.len() > 128
                || value
                    .bytes()
                    .any(|byte| !byte.is_ascii_alphanumeric() && !matches!(byte, b'_' | b'$'))
        })
    {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "exact route input is invalid",
        ));
    }
    Ok(())
}

fn require_account(tx: &Transaction<'_>, account_id: AccountId) -> Result<(), PlatformError> {
    let found: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM accounts WHERE id = ?1 AND deleted_at_ms IS NULL)",
            [account_id.to_string()],
            |row| row.get(0),
        )
        .map_err(|_| db_error())?;
    if found {
        Ok(())
    } else {
        Err(PlatformError::new(
            ErrorCode::AccountNotFound,
            "account was not found",
        ))
    }
}

fn require_live_worker(
    tx: &Transaction<'_>,
    account_id: AccountId,
    worker_id: WorkerId,
) -> Result<WorkerRecord, PlatformError> {
    read_worker_tx(tx, account_id, worker_id).and_then(|worker| {
        if worker.deleted_at_ms.is_some() {
            Err(PlatformError::new(
                ErrorCode::WorkerDeleted,
                "Worker is tombstoned",
            ))
        } else {
            Ok(worker)
        }
    })
}

fn read_worker_tx(
    tx: &Transaction<'_>,
    account_id: AccountId,
    worker_id: WorkerId,
) -> Result<WorkerRecord, PlatformError> {
    tx.query_row(
        "SELECT id, account_id, name,
                (SELECT version_id FROM worker_deployments WHERE id=workers.active_deployment_id),
                do_storage_id, route_generation, created_at_ms, updated_at_ms, deleted_at_ms,
                ownership, active_deployment_id
         FROM workers WHERE id = ?1 AND account_id = ?2",
        params![worker_id.to_string(), account_id.to_string()],
        map_worker,
    )
    .optional()
    .map_err(|_| db_error())?
    .ok_or_else(worker_not_found)
}

fn read_vars(
    conn: &rusqlite::Connection,
    version_id: VersionId,
) -> Result<BTreeMap<String, Vec<u8>>, PlatformError> {
    let mut stmt = conn
        .prepare("SELECT name, value_json FROM version_vars WHERE version_id = ?1 ORDER BY name")
        .map_err(|_| db_error())?;
    let rows = stmt
        .query_map([version_id.to_string()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .map_err(|_| db_error())?;
    let mut out = BTreeMap::new();
    for row in rows {
        let (name, value) = row.map_err(|_| db_error())?;
        out.insert(name, value);
    }
    Ok(out)
}

fn read_secrets(
    conn: &rusqlite::Connection,
    version_id: VersionId,
) -> Result<BTreeMap<String, StoredVersionSecret>, PlatformError> {
    let mut stmt = conn
        .prepare(
            "SELECT name, revision_id, key_id, algorithm, nonce, ciphertext
         FROM version_secrets WHERE version_id = ?1 ORDER BY name",
        )
        .map_err(|_| db_error())?;
    let rows = stmt
        .query_map([version_id.to_string()], |row| {
            let name: String = row.get(0)?;
            Ok((
                name.clone(),
                StoredVersionSecret {
                    name,
                    revision_id: row.get(1)?,
                    envelope: SecretEnvelope {
                        version: 1,
                        key_id: row.get(2)?,
                        algorithm: row.get(3)?,
                        nonce: row.get(4)?,
                        ciphertext: row.get(5)?,
                    },
                },
            ))
        })
        .map_err(|_| db_error())?;
    let mut out = BTreeMap::new();
    for row in rows {
        let (name, value) = row.map_err(|_| db_error())?;
        out.insert(name, value);
    }
    Ok(out)
}

fn map_worker(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkerRecord> {
    let id: String = row.get(0)?;
    let account: String = row.get(1)?;
    let active: Option<String> = row.get(3)?;
    let generation: i64 = row.get(5)?;
    let ownership: String = row.get(9)?;
    Ok(WorkerRecord {
        id: WorkerId::from_str(&id).map_err(|_| rusqlite::Error::InvalidQuery)?,
        account_id: AccountId::from_str(&account).map_err(|_| rusqlite::Error::InvalidQuery)?,
        name: row.get(2)?,
        active_deployment_id: row
            .get::<_, Option<String>>(10)?
            .map(|value| DeploymentId::from_str(&value).map_err(|_| rusqlite::Error::InvalidQuery))
            .transpose()?,
        active_version_id: active
            .map(|value| VersionId::from_str(&value).map_err(|_| rusqlite::Error::InvalidQuery))
            .transpose()?,
        do_storage_id: row.get(4)?,
        route_generation: u64::try_from(generation).map_err(|_| rusqlite::Error::InvalidQuery)?,
        created_at_ms: row.get(6)?,
        updated_at_ms: row.get(7)?,
        deleted_at_ms: row.get(8)?,
        ownership: WorkerOwnership::parse(&ownership).map_err(|_| rusqlite::Error::InvalidQuery)?,
    })
}

fn map_observability_settings(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<WorkerObservabilitySettings> {
    let generation: i64 = row.get(0)?;
    let head_sampling_rate: Option<f64> = row.get(2)?;
    let logs_head_sampling_rate: Option<f64> = row.get(4)?;
    if !valid_sampling_rate(head_sampling_rate) || !valid_sampling_rate(logs_head_sampling_rate) {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(WorkerObservabilitySettings {
        generation: u64::try_from(generation).map_err(|_| rusqlite::Error::InvalidQuery)?,
        enabled: row.get(1)?,
        head_sampling_rate,
        logs_enabled: row.get(3)?,
        logs_head_sampling_rate,
        invocation_logs: row.get(5)?,
        persist: row.get(6)?,
        updated_at_ms: row.get(7)?,
    })
}

fn valid_sampling_rate(value: Option<f64>) -> bool {
    value.is_none_or(|rate| rate.is_finite() && (0.0..=1.0).contains(&rate))
}

fn validate_sampling_rate(value: Option<f64>) -> Result<(), PlatformError> {
    if valid_sampling_rate(value) {
        Ok(())
    } else {
        Err(PlatformError::new(
            ErrorCode::LimitInvalid,
            "observability head sampling rate must be between zero and one",
        ))
    }
}

fn map_system_owned_version(row: &rusqlite::Row<'_>) -> rusqlite::Result<SystemOwnedVersionRecord> {
    let kind: String = row.get(0)?;
    let account: String = row.get(1)?;
    let worker: String = row.get(2)?;
    let active: Option<String> = row.get(3)?;
    let assets: Vec<u8> = row.get(4)?;
    Ok(SystemOwnedVersionRecord {
        kind: SystemOwnedVersionKind::parse(&kind).map_err(|_| rusqlite::Error::InvalidQuery)?,
        account_id: AccountId::from_str(&account).map_err(|_| rusqlite::Error::InvalidQuery)?,
        worker_id: WorkerId::from_str(&worker).map_err(|_| rusqlite::Error::InvalidQuery)?,
        active_version_id: active
            .map(|value| VersionId::from_str(&value).map_err(|_| rusqlite::Error::InvalidQuery))
            .transpose()?,
        assets_sha256: array32(&assets)?,
        updated_at_ms: row.get(5)?,
    })
}

fn is_system_reserved_worker_name(name: &str) -> bool {
    name == SYSTEM_DASHBOARD_WORKER_NAME
}

fn require_tenant_worker(worker: &WorkerRecord) -> Result<(), PlatformError> {
    if worker.ownership != WorkerOwnership::Tenant {
        return Err(worker_not_found());
    }
    Ok(())
}

fn map_version(row: &rusqlite::Row<'_>) -> rusqlite::Result<VersionRecord> {
    let id: String = row.get(0)?;
    let worker: String = row.get(1)?;
    let version: i64 = row.get(2)?;
    let content_kind: String = row.get(3)?;
    let state: String = row.get(4)?;
    let artifact: Option<Vec<u8>> = row.get(5)?;
    let artifact_size: Option<i64> = row.get(6)?;
    let artifact_schema: Option<i64> = row.get(7)?;
    let descriptor: Vec<u8> = row.get(9)?;
    let loader_schema: i64 = row.get(10)?;
    Ok(VersionRecord {
        id: VersionId::from_str(&id).map_err(|_| rusqlite::Error::InvalidQuery)?,
        worker_id: WorkerId::from_str(&worker).map_err(|_| rusqlite::Error::InvalidQuery)?,
        version_number: u64::try_from(version).map_err(|_| rusqlite::Error::InvalidQuery)?,
        content_kind: VersionContentKind::parse(&content_kind)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        state: VersionState::parse(&state).map_err(|_| rusqlite::Error::InvalidQuery)?,
        artifact_sha256: artifact.as_deref().map(array32).transpose()?,
        artifact_size: artifact_size
            .map(u64::try_from)
            .transpose()
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        artifact_schema_version: artifact_schema
            .map(u32::try_from)
            .transpose()
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        main_module: row.get(8)?,
        worker_code_sha256: array32(&descriptor)?,
        loader_schema_version: u32::try_from(loader_schema)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        compatibility_date: row.get(16)?,
        compatibility_flags: serde_json::from_slice(&row.get::<_, Vec<u8>>(17)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        created_at_ms: row.get(11)?,
        ready_at_ms: row.get(12)?,
        rejected_at_ms: row.get(13)?,
        rejection_code: row.get(14)?,
        deleted_at_ms: row.get(15)?,
    })
}

fn read_version_annotations(
    conn: &rusqlite::Connection,
    version_id: VersionId,
) -> Result<BTreeMap<String, String>, PlatformError> {
    let mut statement = conn
        .prepare(
            "SELECT name, value FROM version_annotations
             WHERE version_id = ?1 ORDER BY name",
        )
        .map_err(|_| db_error())?;
    let rows = statement
        .query_map([version_id.to_string()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|_| db_error())?;
    let mut annotations = BTreeMap::new();
    for row in rows {
        let (name, value) = row.map_err(|_| db_error())?;
        if annotations.insert(name, value).is_some() {
            return Err(invariant());
        }
    }
    Ok(annotations)
}

fn map_route(row: &rusqlite::Row<'_>) -> rusqlite::Result<RouteRecord> {
    let account: String = row.get(1)?;
    let worker: String = row.get(2)?;
    let kind: String = row.get(3)?;
    let generation: i64 = row.get(7)?;
    Ok(RouteRecord {
        id: row.get(0)?,
        account_id: AccountId::from_str(&account).map_err(|_| rusqlite::Error::InvalidQuery)?,
        worker_id: WorkerId::from_str(&worker).map_err(|_| rusqlite::Error::InvalidQuery)?,
        kind: RouteKind::parse(&kind).map_err(|_| rusqlite::Error::InvalidQuery)?,
        hostname_ascii: row.get(4)?,
        path_prefix: row.get(5)?,
        entrypoint: row.get(6)?,
        generation: u64::try_from(generation).map_err(|_| rusqlite::Error::InvalidQuery)?,
    })
}

fn map_deployment(row: &rusqlite::Row<'_>) -> rusqlite::Result<DeploymentRecord> {
    let id: String = row.get(0)?;
    let worker: String = row.get(1)?;
    let version: String = row.get(2)?;
    let source: String = row.get(3)?;
    let annotations: Vec<u8> = row.get(4)?;
    Ok(DeploymentRecord {
        id: DeploymentId::from_str(&id).map_err(|_| rusqlite::Error::InvalidQuery)?,
        worker_id: WorkerId::from_str(&worker).map_err(|_| rusqlite::Error::InvalidQuery)?,
        version_id: VersionId::from_str(&version).map_err(|_| rusqlite::Error::InvalidQuery)?,
        source: DeploymentSource::parse(&source).map_err(|_| rusqlite::Error::InvalidQuery)?,
        annotations: serde_json::from_slice(&annotations)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        created_at_ms: row.get(5)?,
        deleted_at_ms: row.get(6)?,
    })
}

fn collect_rows<T>(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>>,
) -> Result<Vec<T>, PlatformError> {
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|_| db_error())?);
    }
    Ok(out)
}

#[allow(
    clippy::too_many_arguments,
    reason = "SQLite boundary inputs mirror authoritative persisted fields"
)]
fn audit(
    tx: &Transaction<'_>,
    account_id: AccountId,
    action: &str,
    target_type: &str,
    target_id: &str,
    request_id: RequestId,
    details: &[u8],
    now_ms: i64,
) -> Result<(), PlatformError> {
    tx.execute(
        "INSERT INTO control_audit_events
         (account_id, action, target_type, target_id, request_id, details_json, created_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            account_id.to_string(),
            action,
            target_type,
            target_id,
            request_id.to_string(),
            details,
            now_ms
        ],
    )
    .map_err(|_| db_error())?;
    Ok(())
}

pub(crate) fn array32(bytes: &[u8]) -> rusqlite::Result<[u8; 32]> {
    bytes.try_into().map_err(|_| rusqlite::Error::InvalidQuery)
}

pub(crate) fn worker_not_found() -> PlatformError {
    PlatformError::new(ErrorCode::WorkerNotFound, "Worker was not found")
}

pub(crate) fn version_not_found() -> PlatformError {
    PlatformError::new(ErrorCode::VersionNotFound, "version was not found")
}

pub(crate) fn route_not_found() -> PlatformError {
    PlatformError::new(
        ErrorCode::RouteNotFound,
        "no active route matched the request",
    )
}

pub(crate) fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::VersionInvariantViolation,
        "persisted version invariant failed",
    )
}

pub(crate) fn db_error() -> PlatformError {
    PlatformError::new(ErrorCode::Internal, "control database operation failed")
}
