//! Durable KV backup and restore workflows shared by the active management API.

use super::*;
use crate::metrics::MetricsRegistry;

/// Create or replay one immutable KV namespace backup.
pub(crate) async fn create_backup(
    api: &KvApiState,
    account_id: AccountId,
    resource_id: ResourceId,
    key: String,
    now_ms: i64,
) -> Result<open_compute_storage::KvBackupRecord, PlatformError> {
    let _admission = api
        .storage
        .reserve_mutation(api.config.namespace_quota_bytes)?;
    let mut canonical = b"open-compute/kv-backup/v1\0".to_vec();
    canonical.extend_from_slice(account_id.as_uuid().as_bytes());
    canonical.extend_from_slice(resource_id.as_uuid().as_bytes());
    let fingerprint = api.storage.crypto().fingerprint_request(&canonical);
    let storage = api.storage.clone();
    let reservation_storage = storage.clone();
    let candidate = uuid::Uuid::now_v7().hyphenated().to_string();
    let (namespace, backup) = tokio::task::spawn_blocking(move || {
        let namespace =
            KvNamespaceRepository::new(reservation_storage.db()).get(account_id, resource_id)?;
        let backup = KvNamespaceRepository::new(reservation_storage.db()).create_backup(
            resource_id,
            &candidate,
            namespace.schema_version,
            &key,
            &fingerprint,
            now_ms,
        )?;
        Ok::<_, PlatformError>((namespace, backup))
    })
    .await
    .map_err(|_| internal())??;
    if backup.state == KvBackupState::Ready {
        return Ok(backup);
    }
    if backup.state == KvBackupState::Failed {
        return Err(replayed_backup_failure(&backup));
    }
    if backup.state != KvBackupState::Creating {
        return Err(PlatformError::new(
            ErrorCode::IdempotencyConflict,
            "KV backup operation cannot resume from its current state",
        ));
    }

    let pin = api.pins.try_pin(resource_id)?;
    let backup_id = backup.id.clone();
    let stage = storage
        .data_dir()
        .backup_staging_dir()
        .join(format!("{backup_id}.sqlite"));
    let stage_for_backup = stage.clone();
    let prepared = tokio::task::spawn_blocking(move || {
        crate::sqlite_staging::remove_sqlite_staging(&stage_for_backup);
        let paths = KvPaths::open(storage.data_dir().root())?;
        let database =
            paths.resolve_storage_key(&namespace.storage_key, account_id, resource_id)?;
        KvEngine::from_record(database, &namespace)?.online_backup(&stage_for_backup)?;
        hash_file(&stage_for_backup)
    })
    .await
    .map_err(|_| internal())
    .and_then(|result| result);
    let (digest, size) = match prepared {
        Ok(value) => value,
        Err(error) => {
            drop(pin);
            crate::sqlite_staging::remove_sqlite_staging(&stage);
            fail_backup(&api.storage, &backup.id, error.code(), now_ms).await;
            return Err(error);
        }
    };

    let base = format!("backups/kv/{account_id}/{resource_id}/{backup_id}");
    let relative = format!("{base}/data.sqlite");
    let response = match api
        .artifacts
        .put_kv_backup_file(&relative, &stage, &hex::encode(digest), size)
        .await
    {
        Ok(object_key) => {
            let manifest = KvBackupManifest {
                backup_schema: KV_BACKUP_MANIFEST_SCHEMA,
                backup_id: backup.id.clone(),
                source_resource_id: resource_id,
                kv_schema_version: backup.kv_schema_version,
                sha256: hex::encode(digest),
                size_bytes: size,
                created_at_ms: backup.created_at_ms,
            };
            match serde_json::to_vec(&manifest).map_err(|_| internal()) {
                Ok(encoded) => match api
                    .artifacts
                    .put_kv_backup_manifest(
                        &format!("{base}/manifest.json"),
                        bytes::Bytes::from(encoded),
                    )
                    .await
                {
                    Ok(_) => {
                        let storage = api.storage.clone();
                        let backup_id = backup.id.clone();
                        tokio::task::spawn_blocking(move || {
                            KvNamespaceRepository::new(storage.db()).complete_backup(
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
                        let _ = api.artifacts.delete_kv_backup(&object_key).await;
                        Err(error)
                    }
                },
                Err(error) => Err(error),
            }
        }
        Err(error) => Err(error),
    };
    drop(pin);
    crate::sqlite_staging::remove_sqlite_staging(&stage);
    match response {
        Ok(backup) => Ok(backup),
        Err(error) => {
            fail_backup(&api.storage, &backup.id, error.code(), now_ms).await;
            Err(error)
        }
    }
}

/// Restore a ready KV backup as a new namespace and return its immutable ID.
#[expect(
    clippy::too_many_arguments,
    reason = "the restore command keeps its authority, identity, and audit inputs explicit"
)]
pub(crate) async fn restore_backup(
    api: &KvApiState,
    metrics: &MetricsRegistry,
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
        KvNamespaceRepository::new(storage.db()).get_backup(account_id, &selected_backup_id)
    })
    .await
    .map_err(|_| internal())??;
    if backup.state != KvBackupState::Ready {
        return Err(PlatformError::new(
            ErrorCode::ResourceNotReady,
            "KV backup is not ready for restore",
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
    let manifest_key = api.artifacts.kv_backup_manifest_key(&object_key)?;
    let manifest_bytes = api.artifacts.get_kv_backup_manifest(&manifest_key).await?;
    match serde_json::from_slice::<KvBackupManifest>(&manifest_bytes) {
        Ok(value)
            if serde_json::to_vec(&value).ok().as_deref() == Some(manifest_bytes.as_ref())
                && value.backup_schema == KV_BACKUP_MANIFEST_SCHEMA
                && value.backup_id == backup.id
                && value.source_resource_id == backup.source_resource_id
                && value.kv_schema_version == backup.kv_schema_version
                && value.sha256 == hex::encode(digest)
                && value.size_bytes == size
                && value.created_at_ms == backup.created_at_ms => {}
        _ => {
            metrics.inc_kv_corruption(1);
            return Err(PlatformError::new(
                ErrorCode::ArtifactIntegrityError,
                "KV backup manifest failed integrity validation",
            ));
        }
    }
    let storage = api.storage.clone();
    let source_resource = backup.source_resource_id;
    let source = tokio::task::spawn_blocking(move || {
        ResourceRepository::new(storage.db()).get(account_id, source_resource)
    })
    .await
    .map_err(|_| internal())??;
    let stage = api
        .storage
        .data_dir()
        .backup_staging_dir()
        .join(format!("{}.restore", uuid::Uuid::now_v7().hyphenated()));
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
            ErrorCode::KvStorageFull,
            "failed to create KV restore staging file",
        )
    })?;
    if let Err(error) = api
        .artifacts
        .download_kv_backup(&object_key, &hex::encode(digest), size, &mut file)
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
    let storage = api.storage.clone();
    let operation = RestoreOperation {
        account_id,
        source_account: source.account_id,
        source_resource: source.id,
        backup_id: backup.id,
        new_name,
        idempotency_key: restore_key,
        request_id,
        now_ms,
        quota_bytes: api.config.namespace_quota_bytes,
        max_resources_per_account: api.max_resources_per_account,
    };
    let stage_for_restore = stage.clone();
    let restored = tokio::task::spawn_blocking(move || {
        restore_downloaded_namespace(&storage, &stage_for_restore, &operation)
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
    source_account: AccountId,
    source_resource: ResourceId,
    backup_id: String,
    new_name: String,
    idempotency_key: String,
    request_id: RequestId,
    now_ms: i64,
    quota_bytes: u64,
    max_resources_per_account: u32,
}

fn restore_downloaded_namespace(
    storage: &PlatformStorage,
    source: &std::path::Path,
    operation: &RestoreOperation,
) -> Result<CreateResourceOutcome, PlatformError> {
    let operation_now = operation.now_ms;
    let mut canonical = b"open-compute/kv-restore/v1\0".to_vec();
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
            kind: BindingKind::KvNamespace,
            name: &operation.new_name,
            idempotency_key: &operation.idempotency_key,
            fingerprint_key_id: storage.crypto().fingerprint_key_id(),
            request_fingerprint: &fingerprint,
            resource_id: ResourceId::generate(),
            driver_schema_version: open_compute_storage::KV_SCHEMA_VERSION,
            request_id: operation.request_id,
            now_ms: operation_now,
            expires_at_ms: operation_now.saturating_add(24 * 60 * 60 * 1000),
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
                "KV restore idempotency is in a failed state",
            ));
        }
        ResourceCreateReservation::Reserved(resource)
        | ResourceCreateReservation::Continue(resource) => resource,
    };
    let catalog = KvNamespaceRepository::new(storage.db());
    let storage_key = KvPaths::storage_key(resource.account_id, resource.id);
    let record = if resource.state == ResourceState::Creating {
        catalog.ensure_restoring_namespace(
            &resource,
            &storage_key,
            open_compute_storage::KV_SCHEMA_VERSION,
            operation.quota_bytes,
            &operation.backup_id,
        )?
    } else {
        catalog.get(resource.account_id, resource.id)?
    };
    if record.restore_backup_id.as_deref() != Some(operation.backup_id.as_str()) {
        return Err(PlatformError::new(
            ErrorCode::ResourceInvariantViolation,
            "KV restore intent does not match durable authority",
        ));
    }
    let paths = KvPaths::open(storage.data_dir().root())?;
    let live = paths.resolve_storage_key(&storage_key, resource.account_id, resource.id)?;
    if live.exists() {
        let engine = KvEngine::from_record(live, &record)?;
        if engine.restore_backup_id()?.as_deref() != Some(operation.backup_id.as_str()) {
            return Err(PlatformError::new(
                ErrorCode::ResourceInvariantViolation,
                "KV restore database does not match durable intent",
            ));
        }
    } else {
        let candidates = paths.namespace_staging_candidates(resource.id)?;
        if candidates.len() > 1 {
            return Err(PlatformError::new(
                ErrorCode::ResourceInvariantViolation,
                "KV restore has multiple physical candidates",
            ));
        }
        let staging = if let Some(staging) = candidates.first() {
            let valid = KvEngine::from_record(staging.join("data.sqlite"), &record)
                .and_then(|engine| engine.restore_backup_id())
                .is_ok_and(|marker| marker.as_deref() == Some(operation.backup_id.as_str()));
            if valid {
                staging.clone()
            } else {
                paths.remove_namespace_staging(staging)?;
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
            "KV restore cannot resume from this resource state",
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
    paths: &KvPaths,
) -> Result<std::path::PathBuf, PlatformError> {
    let staging = paths.create_namespace_staging(resource.id)?;
    let result = KvEngine::restore(
        source,
        &staging.join("data.sqlite"),
        operation.source_account,
        operation.source_resource,
        resource.account_id,
        resource.id,
        &operation.backup_id,
        operation.now_ms,
        operation.quota_bytes,
    );
    if let Err(error) = result {
        let _ = paths.remove_namespace_staging(&staging);
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
        KvNamespaceRepository::new(storage.db()).fail_backup(&backup_id, code, now_ms)
    })
    .await;
}

fn replayed_backup_failure(backup: &open_compute_storage::KvBackupRecord) -> PlatformError {
    let code = backup
        .error_code
        .as_deref()
        .map_or(ErrorCode::Internal, kv_error_code);
    PlatformError::new(code, "KV backup operation previously failed")
}

fn kv_error_code(value: &str) -> ErrorCode {
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
        ErrorCode::KvStorageFull,
        ErrorCode::KvUnavailable,
        ErrorCode::KvCorrupt,
        ErrorCode::KvBusy,
        ErrorCode::Internal,
    ]
    .into_iter()
    .find(|code| code.as_str() == value)
    .unwrap_or(ErrorCode::Internal)
}

pub(super) fn hash_file(path: &std::path::Path) -> Result<([u8; 32], u64), PlatformError> {
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

#[cfg(test)]
#[path = "kv_backup_tests.rs"]
mod tests;
