//! Typed P0.2 control-plane repository.

use crate::catalog_page::{
    CatalogCursor, CatalogCursorValue, CatalogDirection, CatalogListPage, CatalogSort,
    encode_catalog_cursor, invalid_catalog_cursor, normalize_catalog_limit, search_as_worker_id,
};
use crate::control_db::ControlDb;
use crate::crypto::SecretEnvelope;
use open_compute_core::{
    DeploymentId, ErrorCode, InstanceId, PlatformError, RequestId, VersionId, WorkerId,
};
use rusqlite::types::Value;
use rusqlite::{OptionalExtension, Transaction, params, params_from_iter};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use uuid::Uuid;

mod catalog;
mod deployments;
mod idempotency;
mod lifecycle;
mod model;
mod public_origins;
mod python_prepared;
mod retention;
mod version_create;

pub use model::*;
pub use python_prepared::PythonPreparedArtifactRecord;

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

/// Derive the immutable version idempotency referrer identity.
#[must_use]
pub fn idempotency_ref_id(instance_id: InstanceId, scope: &str, key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"open-compute/version-referrer/v1\0");
    hasher.update(instance_id.to_string().as_bytes());
    hasher.update([0]);
    hasher.update(scope.as_bytes());
    hasher.update([0]);
    hasher.update(key.as_bytes());
    hex::encode(hasher.finalize())
}

/// Build the canonical local Worker hostname persisted by route authority.
pub fn local_worker_hostname(
    instance_id: InstanceId,
    worker_name: &str,
) -> Result<String, PlatformError> {
    validate_worker_name(worker_name)?;
    let hostname = format!("{worker_name}.{instance_id}.localhost");
    if hostname.len() > 253 {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "local Worker hostname is too long",
        ));
    }
    Ok(hostname)
}

pub(crate) fn require_instance(
    conn: &rusqlite::Connection,
    instance_id: InstanceId,
) -> Result<(), PlatformError> {
    let found: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM instance_identity WHERE instance_id = ?1)",
            [instance_id],
            |row| row.get(0),
        )
        .map_err(|_| db_error())?;
    if found {
        Ok(())
    } else {
        Err(PlatformError::new(
            ErrorCode::InstanceNotFound,
            "instance was not found",
        ))
    }
}

fn require_live_worker(
    tx: &Transaction<'_>,
    instance_id: InstanceId,
    worker_id: WorkerId,
) -> Result<WorkerRecord, PlatformError> {
    read_worker_tx(tx, instance_id, worker_id).and_then(|worker| {
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
    instance_id: InstanceId,
    worker_id: WorkerId,
) -> Result<WorkerRecord, PlatformError> {
    tx.query_row(
        "SELECT id, (SELECT instance_id FROM instance_identity), name,
                (SELECT version_id FROM worker_deployments WHERE id=workers.active_deployment_id),
                do_storage_id, route_generation, created_at_ms, updated_at_ms, deleted_at_ms,
                ownership, active_deployment_id
         FROM workers WHERE id = ?1 AND (SELECT instance_id FROM instance_identity) = ?2",
        params![worker_id, instance_id],
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
        .query_map([version_id], |row| {
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
        .query_map([version_id], |row| {
            let name: String = row.get(0)?;
            Ok((
                name.clone(),
                StoredVersionSecret {
                    name,
                    revision_id: row.get(1)?,
                    envelope: SecretEnvelope {
                        version: SecretEnvelope::CURRENT_VERSION,
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
    let generation: i64 = row.get(5)?;
    let ownership: String = row.get(9)?;
    Ok(WorkerRecord {
        id: row.get(0)?,
        instance_id: row.get(1)?,
        name: row.get(2)?,
        active_deployment_id: row.get(10)?,
        active_version_id: row.get(3)?,
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

fn map_version(row: &rusqlite::Row<'_>) -> rusqlite::Result<VersionRecord> {
    let version: i64 = row.get(2)?;
    let content_kind: String = row.get(3)?;
    let state: String = row.get(4)?;
    let artifact: Option<Vec<u8>> = row.get(5)?;
    let artifact_size: Option<i64> = row.get(6)?;
    let artifact_schema: Option<i64> = row.get(7)?;
    let descriptor: Vec<u8> = row.get(9)?;
    let loader_schema: i64 = row.get(10)?;
    Ok(VersionRecord {
        id: row.get(0)?,
        worker_id: row.get(1)?,
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
        resource_limits: serde_json::from_slice(&row.get::<_, Vec<u8>>(18)?)
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
        .query_map([version_id], |row| {
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
    let generation: i64 = row.get(6)?;
    Ok(RouteRecord {
        id: row.get(0)?,
        instance_id: row.get(1)?,
        worker_id: row.get(2)?,
        hostname_ascii: row.get(3)?,
        exposure: WorkerOriginExposure::parse(&row.get::<_, String>(8)?)?,
        path_prefix: row.get(4)?,
        entrypoint: row.get(5)?,
        generation: u64::try_from(generation).map_err(|_| rusqlite::Error::InvalidQuery)?,
        created_at_ms: row.get(7)?,
    })
}

fn map_deployment(row: &rusqlite::Row<'_>) -> rusqlite::Result<DeploymentRecord> {
    let source: String = row.get(3)?;
    let annotations: Vec<u8> = row.get(4)?;
    Ok(DeploymentRecord {
        id: row.get(0)?,
        worker_id: row.get(1)?,
        version_id: row.get(2)?,
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

fn audit(
    tx: &Transaction<'_>,
    action: &str,
    target_type: &str,
    target_id: &str,
    request_id: RequestId,
    details: &[u8],
    now_ms: i64,
) -> Result<(), PlatformError> {
    tx.execute(
        "INSERT INTO control_audit_events
         (action, target_type, target_id, request_id, details_json, created_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![action, target_type, target_id, request_id, details, now_ms],
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
    #[cfg(any(test, feature = "test-support"))]
    eprintln!(
        "temporary version DB diagnostic: {}",
        std::backtrace::Backtrace::force_capture()
    );
    PlatformError::new(ErrorCode::Internal, "control database operation failed")
}

#[cfg(test)]
mod resource_limits_model_tests {
    use super::EffectiveResourceLimits;
    use open_compute_core::ErrorCode;

    #[test]
    fn ownership_source_and_content_kind_tokens_round_trip() {
        use super::{DeploymentSource, VersionContentKind, WorkerOwnership};
        assert_eq!(WorkerOwnership::Tenant.as_str(), "tenant");
        assert_eq!(
            WorkerOwnership::parse("tenant").unwrap(),
            WorkerOwnership::Tenant
        );
        assert_eq!(
            WorkerOwnership::parse("other").unwrap_err().code(),
            ErrorCode::VersionInvariantViolation
        );
        for (value, token) in [
            (DeploymentSource::ScriptUpload, "script_upload"),
            (DeploymentSource::VersionsApi, "versions_api"),
            (DeploymentSource::Rollback, "rollback"),
            (DeploymentSource::System, "system"),
        ] {
            assert_eq!(value.as_str(), token);
            assert_eq!(DeploymentSource::parse(token).unwrap(), value);
        }
        assert_eq!(
            DeploymentSource::parse("nope").unwrap_err().code(),
            ErrorCode::VersionInvariantViolation
        );
        for (value, token) in [
            (VersionContentKind::Worker, "worker"),
            (VersionContentKind::AssetsOnly, "assets_only"),
        ] {
            assert_eq!(value.as_str(), token);
            assert_eq!(VersionContentKind::parse(token).unwrap(), value);
        }
        assert_eq!(
            VersionContentKind::parse("unknown").unwrap_err().code(),
            ErrorCode::VersionInvariantViolation
        );
    }

    #[test]
    fn stored_json_uses_typed_decoding() {
        let defaults = EffectiveResourceLimits::standard_defaults();
        assert_eq!(
            serde_json::from_slice::<EffectiveResourceLimits>(&defaults.to_stored_json()).unwrap(),
            defaults
        );
        // Stored values are trusted; only upload and Settings boundaries check bounds.
        let stored: EffectiveResourceLimits =
            serde_json::from_str(r#"{"subRequests":0,"cpuMs":300001}"#).unwrap();
        assert_eq!((stored.cpu_ms, stored.sub_requests), (300_001, 0));
        assert!(serde_json::from_str::<EffectiveResourceLimits>(r#"{"cpuMs":1}"#).is_err());
    }
}
