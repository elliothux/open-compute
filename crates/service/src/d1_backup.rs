//! Durable D1 backup and restore workflows shared by the active management API.

use crate::D1ApiState;
use open_compute_core::{
    AccountId, BindingKind, ErrorCode, PlatformError, RequestId, ResourceId, ResourceState,
};
use open_compute_storage::{
    D1_DATABASE_SCHEMA_VERSION, D1BackupState, D1DatabaseRepository, D1Engine, D1Paths,
    PlatformStorage, ReserveResourceCreate, ResourceCreateReservation, ResourceRepository,
};
use open_compute_workers::{CreateResourceOutcome, CreateResourceResult};
use serde::{Deserialize, Serialize};
use sha2::Digest as _;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt as _;
use std::sync::Arc;

const D1_BACKUP_MANIFEST_SCHEMA: u32 = 1;
const IDEMPOTENCY_TTL_MS: i64 = 24 * 60 * 60 * 1000;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct D1BackupManifest {
    backup_schema: u32,
    backup_id: String,
    source_resource_id: ResourceId,
    d1_schema_version: u32,
    sqlite_user_version: u32,
    sha256: String,
    size_bytes: u64,
    created_at_ms: i64,
}

/// Create or replay one immutable D1 database backup.
pub(crate) async fn create_backup(
    api: &D1ApiState,
    account_id: AccountId,
    resource_id: ResourceId,
    key: String,
    now_ms: i64,
) -> Result<open_compute_storage::D1BackupRecord, PlatformError> {
    let _admission = api
        .storage
        .reserve_mutation(api.config.database_quota_bytes)?;
    let user_version = api.backend.user_version(account_id, resource_id).await?;
    let mut canonical = b"open-compute/d1-backup/v1\0".to_vec();
    canonical.extend_from_slice(account_id.as_uuid().as_bytes());
    canonical.extend_from_slice(resource_id.as_uuid().as_bytes());
    let fingerprint = api.storage.crypto().fingerprint_request(&canonical);
    let storage = api.storage.clone();
    let candidate = uuid::Uuid::now_v7().hyphenated().to_string();
    let (database, backup) = tokio::task::spawn_blocking(move || {
        let database = D1DatabaseRepository::new(storage.db()).get(account_id, resource_id)?;
        let backup = D1DatabaseRepository::new(storage.db()).create_backup(
            resource_id,
            &candidate,
            database.schema_version,
            user_version,
            &key,
            &fingerprint,
            now_ms,
        )?;
        Ok::<_, PlatformError>((database, backup))
    })
    .await
    .map_err(|_| internal())??;
    if backup.state == D1BackupState::Ready {
        return Ok(backup);
    }
    if backup.state == D1BackupState::Failed {
        return Err(replayed_backup_failure(&backup));
    }
    if backup.state != D1BackupState::Creating {
        return Err(PlatformError::new(
            ErrorCode::IdempotencyConflict,
            "D1 backup cannot resume from its current state",
        ));
    }
    let backup_id = backup.id.clone();
    let stage = api
        .storage
        .data_dir()
        .backup_staging_dir()
        .join(format!("{backup_id}.d1.sqlite"));
    crate::sqlite_staging::remove_sqlite_staging(&stage);
    match api
        .backend
        .online_backup(account_id, resource_id, stage.clone())
        .await
    {
        Ok(value) if value == backup.sqlite_user_version => {}
        Ok(_) => {
            let error = PlatformError::new(
                ErrorCode::D1MigrationDrift,
                "D1 schema changed while backup was reserved",
            );
            fail_backup(&api.storage, &backup.id, error.code(), now_ms).await;
            crate::sqlite_staging::remove_sqlite_staging(&stage);
            return Err(error);
        }
        Err(error) => {
            fail_backup(&api.storage, &backup.id, error.code(), now_ms).await;
            crate::sqlite_staging::remove_sqlite_staging(&stage);
            return Err(error);
        }
    }
    let prepared = tokio::task::spawn_blocking({
        let stage = stage.clone();
        move || hash_file(&stage)
    })
    .await
    .map_err(|_| internal())
    .and_then(|result| result);
    let (digest, size) = match prepared {
        Ok(value) => value,
        Err(error) => {
            fail_backup(&api.storage, &backup.id, error.code(), now_ms).await;
            crate::sqlite_staging::remove_sqlite_staging(&stage);
            return Err(error);
        }
    };
    let base = format!("backups/d1/{resource_id}/{backup_id}");
    let relative = format!("{base}/data.sqlite");
    let response = match api
        .artifacts
        .put_d1_backup_file(&relative, &stage, &hex::encode(digest), size)
        .await
    {
        Ok(object_key) => {
            let manifest = D1BackupManifest {
                backup_schema: D1_BACKUP_MANIFEST_SCHEMA,
                backup_id: backup.id.clone(),
                source_resource_id: resource_id,
                d1_schema_version: database.schema_version,
                sqlite_user_version: backup.sqlite_user_version,
                sha256: hex::encode(digest),
                size_bytes: size,
                created_at_ms: backup.created_at_ms,
            };
            match serde_json::to_vec(&manifest).map_err(|_| internal()) {
                Ok(encoded) => match api
                    .artifacts
                    .put_d1_backup_manifest(
                        &format!("{base}/manifest.json"),
                        bytes::Bytes::from(encoded),
                    )
                    .await
                {
                    Ok(_) => {
                        let storage = api.storage.clone();
                        let backup_id = backup.id.clone();
                        tokio::task::spawn_blocking(move || {
                            D1DatabaseRepository::new(storage.db()).complete_backup(
                                &backup_id,
                                &object_key,
                                &digest,
                                size,
                                now_ms,
                            )
                        })
                        .await
                        .map_err(|_| internal())
                        .and_then(|result| result)
                    }
                    Err(error) => {
                        let _ = api.artifacts.delete_d1_backup(&object_key).await;
                        Err(error)
                    }
                },
                Err(error) => Err(error),
            }
        }
        Err(error) => Err(error),
    };
    crate::sqlite_staging::remove_sqlite_staging(&stage);
    match response {
        Ok(backup) => Ok(backup),
        Err(error) => {
            fail_backup(&api.storage, &backup.id, error.code(), now_ms).await;
            Err(error)
        }
    }
}

/// Restore a ready D1 backup as a new database and return its immutable ID.
pub(crate) async fn restore_backup(
    api: &D1ApiState,
    account_id: AccountId,
    backup_id: String,
    new_name: String,
    key: String,
    request_id: RequestId,
    now_ms: i64,
) -> Result<ResourceId, PlatformError> {
    let storage = api.storage.clone();
    let selected_backup_id = backup_id.clone();
    let backup = tokio::task::spawn_blocking(move || {
        D1DatabaseRepository::new(storage.db()).get_backup(account_id, &selected_backup_id)
    })
    .await
    .map_err(|_| internal())??;
    if backup.state != D1BackupState::Ready {
        return Err(PlatformError::new(
            ErrorCode::ResourceNotReady,
            "D1 backup is not ready",
        ));
    }
    let (Some(object_key), Some(digest), Some(size)) =
        (backup.object_key.clone(), backup.sha256, backup.size_bytes)
    else {
        return Err(internal());
    };
    let _admission = api
        .storage
        .reserve_mutation(size.saturating_mul(2).max(1))?;
    let manifest_key = api.artifacts.d1_backup_manifest_key(&object_key)?;
    let manifest_bytes = api.artifacts.get_d1_backup_manifest(&manifest_key).await?;
    match serde_json::from_slice::<D1BackupManifest>(&manifest_bytes) {
        Ok(value)
            if serde_json::to_vec(&value).ok().as_deref() == Some(manifest_bytes.as_ref())
                && value.backup_schema == D1_BACKUP_MANIFEST_SCHEMA
                && value.backup_id == backup.id
                && value.source_resource_id == backup.source_resource_id
                && value.d1_schema_version == backup.d1_schema_version
                && value.sqlite_user_version == backup.sqlite_user_version
                && value.sha256 == hex::encode(digest)
                && value.size_bytes == size
                && value.created_at_ms == backup.created_at_ms => {}
        _ => {
            return Err(PlatformError::new(
                ErrorCode::ArtifactIntegrityError,
                "D1 backup manifest failed integrity validation",
            ));
        }
    }
    crate::d1_backend::ensure_d1_storage_headroom(&api.storage)?;
    let stage = api
        .storage
        .data_dir()
        .backup_staging_dir()
        .join(format!("{}.d1.restore", uuid::Uuid::now_v7().hyphenated()));
    let stage_for_create = stage.clone();
    let mut file = tokio::task::spawn_blocking(move || {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(stage_for_create)
    })
    .await
    .map_err(|_| internal())?
    .map_err(|_| {
        PlatformError::new(
            ErrorCode::D1DatabaseFull,
            "failed to create D1 restore staging file",
        )
    })?;
    if let Err(error) = api
        .artifacts
        .download_d1_backup(&object_key, &hex::encode(digest), size, &mut file)
        .await
    {
        crate::sqlite_staging::remove_sqlite_staging(&stage);
        return Err(error);
    }
    let synced = tokio::task::spawn_blocking(move || file.sync_all())
        .await
        .map_err(|_| internal())?;
    if synced.is_err() {
        crate::sqlite_staging::remove_sqlite_staging(&stage);
        return Err(internal());
    }
    let restore_key = format!(
        "restore-{}",
        hex::encode(sha2::Sha256::digest(key.as_bytes()))
    );
    let operation = RestoreOperation {
        account_id,
        backup_id: backup.id,
        new_name,
        idempotency_key: restore_key,
        request_id,
        now_ms,
        quota_bytes: api.config.database_quota_bytes,
        max_resources_per_account: api.max_resources_per_account,
    };
    let storage = api.storage.clone();
    let stage_for_restore = stage.clone();
    let restored = tokio::task::spawn_blocking(move || {
        restore_downloaded_database(&storage, &stage_for_restore, &operation)
    })
    .await
    .map_err(|_| internal())?;
    crate::sqlite_staging::remove_sqlite_staging(&stage);
    match restored? {
        CreateResourceOutcome::Applied(result) => Ok(result.resource_id),
        CreateResourceOutcome::Replay(bytes) => {
            serde_json::from_slice::<CreateResourceResult>(&bytes)
                .map(|result| result.resource_id)
                .map_err(|_| internal())
        }
    }
}

struct RestoreOperation {
    account_id: AccountId,
    backup_id: String,
    new_name: String,
    idempotency_key: String,
    request_id: RequestId,
    now_ms: i64,
    quota_bytes: u64,
    max_resources_per_account: u32,
}

fn restore_downloaded_database(
    storage: &PlatformStorage,
    source: &std::path::Path,
    operation: &RestoreOperation,
) -> Result<CreateResourceOutcome, PlatformError> {
    let operation_now = operation.now_ms;
    let mut canonical = b"open-compute/d1-restore/v1\0".to_vec();
    canonical.extend_from_slice(operation.account_id.as_uuid().as_bytes());
    canonical.extend_from_slice(operation.backup_id.as_bytes());
    canonical.push(0);
    canonical.extend_from_slice(operation.new_name.as_bytes());
    canonical.extend_from_slice(&operation.quota_bytes.to_be_bytes());
    let fingerprint = storage.crypto().fingerprint_request(&canonical);
    let repository = ResourceRepository::new(storage.db());
    let reservation = repository.reserve_create(
        &ReserveResourceCreate {
            account_id: operation.account_id,
            kind: BindingKind::D1Database,
            name: &operation.new_name,
            idempotency_key: &operation.idempotency_key,
            fingerprint_key_id: storage.crypto().fingerprint_key_id(),
            request_fingerprint: &fingerprint,
            resource_id: ResourceId::generate(),
            driver_schema_version: D1_DATABASE_SCHEMA_VERSION,
            request_id: operation.request_id,
            now_ms: operation_now,
            expires_at_ms: operation_now.saturating_add(IDEMPOTENCY_TTL_MS),
        },
        operation.max_resources_per_account,
    )?;
    let resource = match reservation {
        ResourceCreateReservation::Complete(response) => {
            return Ok(CreateResourceOutcome::Replay(response));
        }
        ResourceCreateReservation::Failed(_) => {
            return Err(PlatformError::new(
                ErrorCode::ResourceInvariantViolation,
                "D1 restore idempotency is in a failed state",
            ));
        }
        ResourceCreateReservation::Reserved(resource)
        | ResourceCreateReservation::Continue(resource) => resource,
    };
    let catalog = D1DatabaseRepository::new(storage.db());
    let storage_key = D1Paths::storage_key(resource.account_id, resource.id);
    let record = if resource.state == ResourceState::Creating {
        catalog.ensure_restoring_database(
            &resource,
            &storage_key,
            D1_DATABASE_SCHEMA_VERSION,
            operation.quota_bytes,
            &operation.backup_id,
        )?
    } else {
        catalog.get(resource.account_id, resource.id)?
    };
    if record.restore_backup_id.as_deref() != Some(operation.backup_id.as_str()) {
        return Err(PlatformError::new(
            ErrorCode::ResourceInvariantViolation,
            "D1 restore intent does not match durable authority",
        ));
    }
    let paths = D1Paths::open(storage.data_dir().root())?;
    let live = paths.resolve_storage_key(&storage_key, resource.account_id, resource.id)?;
    if live.exists() {
        D1Engine::from_record(live, &record)?.quick_check()?;
    } else {
        let candidates = paths.staging_candidates(resource.id)?;
        if candidates.len() > 1 {
            return Err(PlatformError::new(
                ErrorCode::ResourceInvariantViolation,
                "D1 restore has multiple physical candidates",
            ));
        }
        let staging = if let Some(staging) = candidates.first() {
            if D1Engine::from_record(staging.join("data.sqlite"), &record)
                .and_then(|engine| engine.quick_check())
                .is_ok()
            {
                staging.clone()
            } else {
                paths.remove_operation_dir(staging)?;
                create_restored_staging(source, operation, &resource, &paths)?
            }
        } else {
            create_restored_staging(source, operation, &resource, &paths)?
        };
        paths.publish_staging(&staging, resource.account_id, resource.id)?;
    }
    if resource.state == ResourceState::Creating {
        repository.mark_ready(resource.id, operation.now_ms)?;
    } else if resource.state != ResourceState::Ready {
        return Err(PlatformError::new(
            ErrorCode::ResourceNotReady,
            "D1 restore cannot resume from this resource state",
        ));
    }
    let result = CreateResourceResult {
        resource_id: resource.id,
        state: ResourceState::Ready,
    };
    let response = serde_json::to_vec(&result).map_err(|_| internal())?;
    repository.complete_create(
        resource.account_id,
        &operation.idempotency_key,
        &fingerprint,
        resource.id,
        &response,
    )?;
    Ok(CreateResourceOutcome::Applied(result))
}

fn create_restored_staging(
    source: &std::path::Path,
    operation: &RestoreOperation,
    resource: &open_compute_storage::ResourceRecord,
    paths: &D1Paths,
) -> Result<std::path::PathBuf, PlatformError> {
    let staging = paths.create_database_staging(resource.id)?;
    let result = D1Engine::restore_as_new(
        source,
        &staging.join("data.sqlite"),
        resource.account_id,
        resource.id,
        resource.created_at_ms,
        operation.quota_bytes,
    );
    if let Err(error) = result {
        let _ = paths.remove_operation_dir(&staging);
        return Err(error);
    }
    Ok(staging)
}

async fn fail_backup(
    storage: &Arc<PlatformStorage>,
    backup_id: &str,
    code: ErrorCode,
    now_ms: i64,
) {
    let storage = storage.clone();
    let backup_id = backup_id.to_owned();
    let _ = tokio::task::spawn_blocking(move || {
        D1DatabaseRepository::new(storage.db()).fail_backup(&backup_id, code, now_ms)
    })
    .await;
}

fn replayed_backup_failure(backup: &open_compute_storage::D1BackupRecord) -> PlatformError {
    let code = backup
        .error_code
        .as_deref()
        .map_or(ErrorCode::Internal, d1_error_code);
    PlatformError::new(code, "D1 backup operation previously failed")
}

fn d1_error_code(value: &str) -> ErrorCode {
    [
        ErrorCode::ResourceNotFound,
        ErrorCode::ResourceNotReady,
        ErrorCode::ResourceInvariantViolation,
        ErrorCode::IdempotencyConflict,
        ErrorCode::ArtifactUnavailable,
        ErrorCode::ArtifactIntegrityError,
        ErrorCode::ObjectStorageUnavailable,
        ErrorCode::DiskHardLimit,
        ErrorCode::LimitInvalid,
        ErrorCode::D1DatabaseFull,
        ErrorCode::D1DatabaseCorrupt,
        ErrorCode::D1IdentityMismatch,
        ErrorCode::D1MigrationDrift,
        ErrorCode::Internal,
    ]
    .into_iter()
    .find(|code| code.as_str() == value)
    .unwrap_or(ErrorCode::Internal)
}

fn hash_file(path: &std::path::Path) -> Result<([u8; 32], u64), PlatformError> {
    let mut file = std::fs::File::open(path).map_err(|_| internal())?;
    let mut hasher = sha2::Sha256::new();
    let mut total = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|_| internal())?;
        if count == 0 {
            break;
        }
        total = total.checked_add(count as u64).ok_or_else(internal)?;
        hasher.update(&buffer[..count]);
    }
    Ok((hasher.finalize().into(), total))
}

fn internal() -> PlatformError {
    PlatformError::new(ErrorCode::Internal, "D1 backup operation failed")
}

#[cfg(test)]
#[path = "d1_backup_tests.rs"]
mod tests;
