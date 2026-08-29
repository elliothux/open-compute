//! D1 backup and restore control operations.

use super::*;
use sha2::Digest as _;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt as _;

const D1_BACKUP_MANIFEST_SCHEMA: u32 = 1;

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

pub(super) async fn create_backup(
    State(state): State<HttpState>,
    Path((account, resource)): Path<(String, String)>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request) else {
        return unauthorized_or_unavailable(&state, &request, request_id);
    };
    let (account_id, resource_id) = match parse_ids(&account, &resource) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let _admission = match api
        .storage
        .reserve_mutation(api.config.database_quota_bytes)
    {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let key = match idempotency_key(&request) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let backup_metric = D1LifecycleGuard::new(state.metrics().clone(), D1Lifecycle::Backup);
    let user_version = match api.backend.user_version(account_id, resource_id).await {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let mut canonical = b"open-compute/d1-backup/v1\0".to_vec();
    canonical.extend_from_slice(account_id.as_uuid().as_bytes());
    canonical.extend_from_slice(resource_id.as_uuid().as_bytes());
    let fingerprint = api.storage.crypto().fingerprint_request(&canonical);
    let storage = api.storage.clone();
    let candidate = uuid::Uuid::now_v7().hyphenated().to_string();
    let reservation = tokio::task::spawn_blocking(move || {
        let database = D1DatabaseRepository::new(storage.db()).get(account_id, resource_id)?;
        let backup = D1DatabaseRepository::new(storage.db()).create_backup(
            resource_id,
            &candidate,
            database.schema_version,
            user_version,
            &key,
            &fingerprint,
            now_ms(),
        )?;
        Ok::<_, PlatformError>((database, backup))
    })
    .await;
    let (database, backup) = match reservation {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => return error_response(error, request_id),
        Err(_) => return error_response(internal(), request_id),
    };
    if backup.state == D1BackupState::Ready {
        backup_metric.success();
        return json_response(&serde_json::json!({ "backup": backup }), StatusCode::OK);
    }
    if backup.state == D1BackupState::Failed {
        return error_response(replayed_backup_failure(&backup), request_id);
    }
    if backup.state != D1BackupState::Creating {
        return error_response(
            PlatformError::new(
                ErrorCode::IdempotencyConflict,
                "D1 backup cannot resume from its current state",
            ),
            request_id,
        );
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
            fail_backup(&api.storage, &backup.id, error.code()).await;
            crate::sqlite_staging::remove_sqlite_staging(&stage);
            return error_response(error, request_id);
        }
        Err(error) => {
            fail_backup(&api.storage, &backup.id, error.code()).await;
            crate::sqlite_staging::remove_sqlite_staging(&stage);
            return error_response(error, request_id);
        }
    }
    let (digest, size) = match tokio::task::spawn_blocking({
        let stage = stage.clone();
        move || hash_file(&stage)
    })
    .await
    {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => {
            fail_backup(&api.storage, &backup.id, error.code()).await;
            crate::sqlite_staging::remove_sqlite_staging(&stage);
            return error_response(error, request_id);
        }
        Err(_) => {
            fail_backup(&api.storage, &backup.id, ErrorCode::Internal).await;
            crate::sqlite_staging::remove_sqlite_staging(&stage);
            return error_response(internal(), request_id);
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
                        match tokio::task::spawn_blocking(move || {
                            D1DatabaseRepository::new(storage.db()).complete_backup(
                                &backup_id,
                                &object_key,
                                &digest,
                                size,
                                now_ms(),
                            )
                        })
                        .await
                        {
                            Ok(result) => result,
                            Err(_) => Err(internal()),
                        }
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
        Ok(backup) => {
            backup_metric.success();
            json_response(
                &serde_json::json!({ "backup": backup }),
                StatusCode::CREATED,
            )
        }
        Err(error) => {
            fail_backup(&api.storage, &backup.id, error.code()).await;
            error_response(error, request_id)
        }
    }
}

pub(super) async fn list_backups(
    State(state): State<HttpState>,
    Path((account, resource)): Path<(String, String)>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request) else {
        return unauthorized_or_unavailable(&state, &request, request_id);
    };
    let (account_id, resource_id) = match parse_ids(&account, &resource) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let storage = api.storage.clone();
    match tokio::task::spawn_blocking(move || {
        D1DatabaseRepository::new(storage.db()).get(account_id, resource_id)?;
        D1DatabaseRepository::new(storage.db()).list_backups(account_id, resource_id)
    })
    .await
    {
        Ok(Ok(backups)) => {
            json_response(&serde_json::json!({ "backups": backups }), StatusCode::OK)
        }
        Ok(Err(error)) => error_response(error, request_id),
        Err(_) => error_response(internal(), request_id),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RestoreDatabaseBody {
    backup_id: String,
    new_name: String,
}

pub(super) async fn restore_database(
    State(state): State<HttpState>,
    Path(account): Path<String>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request) else {
        return unauthorized_or_unavailable(&state, &request, request_id);
    };
    let account_id = match parse_account(&account) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let key = match idempotency_key(&request) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let body = match read_json::<RestoreDatabaseBody>(request).await {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let storage = api.storage.clone();
    let backup_id = body.backup_id.clone();
    let backup = match tokio::task::spawn_blocking(move || {
        D1DatabaseRepository::new(storage.db()).get_backup(account_id, &backup_id)
    })
    .await
    {
        Ok(Ok(value)) if value.state == D1BackupState::Ready => value,
        Ok(Ok(_)) => {
            return error_response(
                PlatformError::new(ErrorCode::ResourceNotReady, "D1 backup is not ready"),
                request_id,
            );
        }
        Ok(Err(error)) => return error_response(error, request_id),
        Err(_) => return error_response(internal(), request_id),
    };
    let (Some(object_key), Some(digest), Some(size)) =
        (backup.object_key.clone(), backup.sha256, backup.size_bytes)
    else {
        return error_response(internal(), request_id);
    };
    let _admission = match api.storage.reserve_mutation(size.saturating_mul(2).max(1)) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let manifest_key = match api.artifacts.d1_backup_manifest_key(&object_key) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let manifest_bytes = match api.artifacts.get_d1_backup_manifest(&manifest_key).await {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
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
            return error_response(
                PlatformError::new(
                    ErrorCode::ArtifactIntegrityError,
                    "D1 backup manifest failed integrity validation",
                ),
                request_id,
            );
        }
    }
    if let Err(error) = crate::d1_backend::ensure_d1_storage_headroom(&api.storage) {
        return error_response(error, request_id);
    }
    let stage = api
        .storage
        .data_dir()
        .backup_staging_dir()
        .join(format!("{}.d1.restore", uuid::Uuid::now_v7().hyphenated()));
    let stage_for_create = stage.clone();
    let file = tokio::task::spawn_blocking(move || {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(stage_for_create)
    })
    .await;
    let Ok(Ok(file)) = file else {
        return error_response(
            PlatformError::new(
                ErrorCode::D1DatabaseFull,
                "failed to create D1 restore staging file",
            ),
            request_id,
        );
    };
    let mut file = file;
    if let Err(error) = api
        .artifacts
        .download_d1_backup(&object_key, &hex::encode(digest), size, &mut file)
        .await
    {
        crate::sqlite_staging::remove_sqlite_staging(&stage);
        return error_response(error, request_id);
    }
    let synced = tokio::task::spawn_blocking(move || file.sync_all()).await;
    if !matches!(synced, Ok(Ok(()))) {
        crate::sqlite_staging::remove_sqlite_staging(&stage);
        return error_response(internal(), request_id);
    }
    let restore_key = format!(
        "restore-{}",
        hex::encode(sha2::Sha256::digest(key.as_bytes()))
    );
    let operation = RestoreOperation {
        account_id,
        backup_id: backup.id,
        new_name: body.new_name,
        idempotency_key: restore_key,
        request_id,
        quota_bytes: api.config.database_quota_bytes,
        max_resources_per_account: api.max_resources_per_account,
    };
    let storage = api.storage.clone();
    let stage_for_restore = stage.clone();
    let restored = tokio::task::spawn_blocking(move || {
        restore_downloaded_database(&storage, &stage_for_restore, &operation)
    })
    .await;
    crate::sqlite_staging::remove_sqlite_staging(&stage);
    match restored {
        Ok(Ok(CreateResourceOutcome::Applied(result))) => {
            json_response(&result, StatusCode::CREATED)
        }
        Ok(Ok(CreateResourceOutcome::Replay(bytes))) => json_bytes(bytes, StatusCode::OK),
        Ok(Err(error)) => error_response(error, request_id),
        Err(_) => error_response(internal(), request_id),
    }
}

struct RestoreOperation {
    account_id: AccountId,
    backup_id: String,
    new_name: String,
    idempotency_key: String,
    request_id: RequestId,
    quota_bytes: u64,
    max_resources_per_account: u32,
}

fn restore_downloaded_database(
    storage: &PlatformStorage,
    source: &std::path::Path,
    operation: &RestoreOperation,
) -> Result<CreateResourceOutcome, PlatformError> {
    let operation_now = now_ms();
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
        repository.mark_ready(resource.id, now_ms())?;
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

async fn fail_backup(storage: &Arc<PlatformStorage>, backup_id: &str, code: ErrorCode) {
    let storage = storage.clone();
    let backup_id = backup_id.to_owned();
    let _ = tokio::task::spawn_blocking(move || {
        D1DatabaseRepository::new(storage.db()).fail_backup(&backup_id, code, now_ms())
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
        ErrorCode::S3Unavailable,
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
