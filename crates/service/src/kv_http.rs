//! P0.4 KV namespace control API.

use crate::http::{HttpState, authorize};
use crate::metrics::{KvLifecycle, KvLifecycleGuard};
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use open_compute_artifacts::ArtifactStore;
use open_compute_core::{
    AccountId, BindingKind, ErrorCode, KvConfig, PlatformError, RequestId, ResourceId,
    ResourceState,
};
use open_compute_storage::{
    KvBackupState, KvEngine, KvNamespaceRepository, KvPaths, PlatformStorage,
    ReserveResourceCreate, ResourceCreateReservation, ResourceRepository,
};
use open_compute_workers::{
    CreateResourceOutcome, CreateResourceRequest, CreateResourceResult, KvResourceDriver,
    ResourceController, ResourcePins,
};
use serde::{Deserialize, Serialize};
use sha2::Digest as _;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt as _;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MAX_JSON_BODY: usize = 4096;
const IDEMPOTENCY_HEADER: &str = "idempotency-key";
const KV_BACKUP_MANIFEST_SCHEMA: u32 = 1;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct KvBackupManifest {
    backup_schema: u32,
    backup_id: String,
    source_resource_id: ResourceId,
    kv_schema_version: u32,
    sha256: String,
    size_bytes: u64,
    created_at_ms: i64,
}

/// Shared KV Control API composition state.
#[derive(Clone)]
pub struct KvApiState {
    storage: Arc<PlatformStorage>,
    artifacts: ArtifactStore,
    pins: ResourcePins,
    config: KvConfig,
    delete_drain_timeout: Duration,
}

impl std::fmt::Debug for KvApiState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KvApiState")
            .field("artifacts", &self.artifacts)
            .field("pins", &self.pins)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl KvApiState {
    /// Bind central authority, S3 backup storage, pins, and operator limits.
    #[must_use]
    pub fn new(
        storage: Arc<PlatformStorage>,
        artifacts: ArtifactStore,
        pins: ResourcePins,
        config: KvConfig,
        delete_drain_timeout: Duration,
    ) -> Self {
        Self {
            storage,
            artifacts,
            pins,
            config,
            delete_drain_timeout,
        }
    }
}

/// Router for the product-specific KV management surface.
pub fn control_router() -> Router<HttpState> {
    Router::new()
        .route(
            "/v1/accounts/{account_id}/kv/namespaces",
            post(create_namespace).get(list_namespaces),
        )
        .route(
            "/v1/accounts/{account_id}/kv/namespaces/{resource_id}/backups",
            post(create_backup),
        )
        .route("/v1/accounts/{account_id}/kv/backups", get(list_backups))
        .route(
            "/v1/accounts/{account_id}/kv/namespaces:restore",
            post(restore_namespace),
        )
        .route(
            "/v1/accounts/{account_id}/kv/backups/{backup_id}",
            axum::routing::delete(delete_backup),
        )
        .route(
            "/v1/accounts/{account_id}/kv/namespaces/{resource_id}",
            get(get_namespace)
                .patch(rename_namespace)
                .delete(delete_namespace),
        )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateNamespaceBody {
    name: String,
}

async fn create_namespace(
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
    let body = match read_json::<CreateNamespaceBody>(request).await {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let storage = api.storage.clone();
    let pins = api.pins.clone();
    let quota = api.config.namespace_quota_bytes;
    let result = tokio::task::spawn_blocking(move || {
        let driver = KvResourceDriver::new(&storage, quota);
        ResourceController::new(&storage, pins, driver).create(&CreateResourceRequest {
            account_id,
            kind: BindingKind::KvNamespace,
            name: body.name,
            idempotency_key: key,
            driver_schema_version: open_compute_storage::KV_SCHEMA_VERSION,
            request_id,
            now_ms: now_ms(),
        })
    })
    .await;
    match result {
        Ok(Ok(CreateResourceOutcome::Applied(value))) => json_response(&value, StatusCode::CREATED),
        Ok(Ok(CreateResourceOutcome::Replay(bytes))) => json_bytes(bytes, StatusCode::OK),
        Ok(Err(error)) => error_response(error, request_id),
        Err(_) => error_response(internal(), request_id),
    }
}

async fn list_namespaces(
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
    let storage = api.storage.clone();
    match tokio::task::spawn_blocking(move || {
        KvNamespaceRepository::new(storage.db()).list(account_id)
    })
    .await
    {
        Ok(Ok(namespaces)) => json_response(
            &serde_json::json!({ "namespaces": namespaces }),
            StatusCode::OK,
        ),
        Ok(Err(error)) => error_response(error, request_id),
        Err(_) => error_response(internal(), request_id),
    }
}

async fn get_namespace(
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
        KvNamespaceRepository::new(storage.db()).get(account_id, resource_id)
    })
    .await
    {
        Ok(Ok(namespace)) => json_response(
            &serde_json::json!({ "namespace": namespace }),
            StatusCode::OK,
        ),
        Ok(Err(error)) => error_response(error, request_id),
        Err(_) => error_response(internal(), request_id),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RenameNamespaceBody {
    name: String,
}

async fn rename_namespace(
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
    let body = match read_json::<RenameNamespaceBody>(request).await {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let storage = api.storage.clone();
    let pins = api.pins.clone();
    let quota = api.config.namespace_quota_bytes;
    match tokio::task::spawn_blocking(move || {
        let driver = KvResourceDriver::new(&storage, quota);
        ResourceController::new(&storage, pins, driver).rename(
            account_id,
            resource_id,
            &body.name,
            request_id,
            now_ms(),
        )
    })
    .await
    {
        Ok(Ok(resource)) => json_response(
            &serde_json::json!({ "namespace": resource }),
            StatusCode::OK,
        ),
        Ok(Err(error)) => error_response(error, request_id),
        Err(_) => error_response(internal(), request_id),
    }
}

async fn delete_namespace(
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
    if let Err(error) = idempotency_key(&request) {
        return error_response(error, request_id);
    }
    let storage = api.storage.clone();
    let current = tokio::task::spawn_blocking(move || {
        ResourceRepository::new(storage.db()).get(account_id, resource_id)
    })
    .await;
    match current {
        Ok(Ok(resource)) if resource.state == ResourceState::Tombstoned => {
            return json_response(
                &serde_json::json!({ "resourceId": resource_id, "state": "tombstoned" }),
                StatusCode::OK,
            );
        }
        Ok(Ok(_)) => {}
        Ok(Err(error)) => return error_response(error, request_id),
        Err(_) => return error_response(internal(), request_id),
    }
    let driver = KvResourceDriver::new(&api.storage, api.config.namespace_quota_bytes);
    let controller = ResourceController::new(&api.storage, api.pins.clone(), driver);
    match controller
        .delete(
            account_id,
            resource_id,
            request_id,
            now_ms(),
            api.delete_drain_timeout,
        )
        .await
    {
        Ok(()) => json_response(
            &serde_json::json!({ "resourceId": resource_id, "state": "tombstoned" }),
            StatusCode::ACCEPTED,
        ),
        Err(error) => error_response(error, request_id),
    }
}

async fn create_backup(
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
    let key = match idempotency_key(&request) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let backup_metric = KvLifecycleGuard::new(state.metrics().clone(), KvLifecycle::Backup);
    let mut canonical = b"open-compute/kv-backup/v1\0".to_vec();
    canonical.extend_from_slice(account_id.as_uuid().as_bytes());
    canonical.extend_from_slice(resource_id.as_uuid().as_bytes());
    let fingerprint = api.storage.crypto().fingerprint_request(&canonical);
    let storage = api.storage.clone();
    let reservation_storage = storage.clone();
    let candidate = uuid::Uuid::now_v7().hyphenated().to_string();
    let reservation = tokio::task::spawn_blocking(move || {
        let namespace =
            KvNamespaceRepository::new(reservation_storage.db()).get(account_id, resource_id)?;
        let backup = KvNamespaceRepository::new(reservation_storage.db()).create_backup(
            resource_id,
            &candidate,
            namespace.schema_version,
            &key,
            &fingerprint,
            now_ms(),
        )?;
        Ok::<_, PlatformError>((namespace, backup))
    })
    .await;
    let (namespace, backup) = match reservation {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => return error_response(error, request_id),
        Err(_) => return error_response(internal(), request_id),
    };
    if backup.state == KvBackupState::Ready {
        backup_metric.success();
        return json_response(&serde_json::json!({ "backup": backup }), StatusCode::OK);
    }
    if backup.state == KvBackupState::Failed {
        return error_response(replayed_backup_failure(&backup), request_id);
    }
    if backup.state != KvBackupState::Creating {
        return error_response(
            PlatformError::new(
                ErrorCode::IdempotencyConflict,
                "KV backup operation cannot resume from its current state",
            ),
            request_id,
        );
    }
    let pin = match api.pins.try_pin(resource_id) {
        Ok(pin) => pin,
        Err(error) => return error_response(error, request_id),
    };
    let backup_id = backup.id.clone();
    let stage = storage
        .data_dir()
        .backup_staging_dir()
        .join(format!("{backup_id}.sqlite"));
    let stage_for_backup = stage.clone();
    let prepared = tokio::task::spawn_blocking(move || {
        if stage_for_backup.exists() {
            std::fs::remove_file(&stage_for_backup).map_err(|_| internal())?;
        }
        let paths = KvPaths::open(storage.data_dir().root())?;
        let database =
            paths.resolve_storage_key(&namespace.storage_key, account_id, resource_id)?;
        KvEngine::from_record(database, &namespace)?.online_backup(&stage_for_backup)?;
        hash_file(&stage_for_backup)
    })
    .await;
    let (digest, size) = match prepared {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => {
            drop(pin);
            let _ = std::fs::remove_file(&stage);
            fail_backup(&api.storage, &backup.id, error.code()).await;
            return error_response(error, request_id);
        }
        Err(_) => {
            drop(pin);
            let _ = std::fs::remove_file(&stage);
            fail_backup(&api.storage, &backup.id, ErrorCode::Internal).await;
            return error_response(internal(), request_id);
        }
    };
    let base = format!("backups/kv/{account_id}/{resource_id}/{backup_id}");
    let relative = format!("{base}/data.sqlite");
    let upload = api
        .artifacts
        .put_kv_backup_file(&relative, &stage, &hex::encode(digest), size)
        .await;
    let response = match upload {
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
            let encoded = serde_json::to_vec(&manifest).map_err(|_| internal());
            match encoded {
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
                        let completed = tokio::task::spawn_blocking(move || {
                            KvNamespaceRepository::new(storage.db()).complete_backup(
                                &backup_id,
                                &object_key,
                                &digest,
                                size,
                                now_ms(),
                            )
                        })
                        .await;
                        match completed {
                            Ok(result) => result,
                            Err(_) => Err(internal()),
                        }
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
    let _ = std::fs::remove_file(stage);
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

async fn list_backups(
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
    let storage = api.storage.clone();
    match tokio::task::spawn_blocking(move || {
        KvNamespaceRepository::new(storage.db()).list_backups(account_id)
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
struct RestoreNamespaceBody {
    backup_id: String,
    new_name: String,
}

async fn restore_namespace(
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
    let body = match read_json::<RestoreNamespaceBody>(request).await {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let restore_metric = KvLifecycleGuard::new(state.metrics().clone(), KvLifecycle::Restore);
    let storage = api.storage.clone();
    let backup_id = body.backup_id.clone();
    let backup = match tokio::task::spawn_blocking(move || {
        KvNamespaceRepository::new(storage.db()).get_backup(account_id, &backup_id)
    })
    .await
    {
        Ok(Ok(value)) if value.state == KvBackupState::Ready => value,
        Ok(Ok(_)) => {
            return error_response(
                PlatformError::new(
                    ErrorCode::ResourceNotReady,
                    "KV backup is not ready for restore",
                ),
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
    let manifest_key = match api.artifacts.kv_backup_manifest_key(&object_key) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let manifest_bytes = match api.artifacts.get_kv_backup_manifest(&manifest_key).await {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let manifest = match serde_json::from_slice::<KvBackupManifest>(&manifest_bytes) {
        Ok(value)
            if serde_json::to_vec(&value).ok().as_deref() == Some(manifest_bytes.as_ref())
                && value.backup_schema == KV_BACKUP_MANIFEST_SCHEMA
                && value.backup_id == backup.id
                && value.source_resource_id == backup.source_resource_id
                && value.kv_schema_version == backup.kv_schema_version
                && value.sha256 == hex::encode(digest)
                && value.size_bytes == size
                && value.created_at_ms == backup.created_at_ms =>
        {
            value
        }
        _ => {
            state.metrics().inc_kv_corruption(1);
            return error_response(
                PlatformError::new(
                    ErrorCode::ArtifactIntegrityError,
                    "KV backup manifest failed integrity validation",
                ),
                request_id,
            );
        }
    };
    let _ = manifest;
    let storage = api.storage.clone();
    let source_resource = backup.source_resource_id;
    let source = match tokio::task::spawn_blocking(move || {
        ResourceRepository::new(storage.db()).get(account_id, source_resource)
    })
    .await
    {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => return error_response(error, request_id),
        Err(_) => return error_response(internal(), request_id),
    };
    let stage = api
        .storage
        .data_dir()
        .backup_staging_dir()
        .join(format!("{}.restore", uuid::Uuid::now_v7().hyphenated()));
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
                ErrorCode::KvStorageFull,
                "failed to create KV restore staging file",
            ),
            request_id,
        );
    };
    let mut file = file;
    if let Err(error) = api
        .artifacts
        .download_kv_backup(&object_key, &hex::encode(digest), size, &mut file)
        .await
    {
        let _ = std::fs::remove_file(&stage);
        return error_response(error, request_id);
    }
    let synced = tokio::task::spawn_blocking(move || file.sync_all()).await;
    if !matches!(synced, Ok(Ok(()))) {
        let _ = std::fs::remove_file(&stage);
        return error_response(internal(), request_id);
    }
    let restore_key = format!(
        "restore-{}",
        hex::encode(sha2::Sha256::digest(key.as_bytes()))
    );
    let storage = api.storage.clone();
    let quota = api.config.namespace_quota_bytes;
    let operation = RestoreOperation {
        account_id,
        source_account: source.account_id,
        source_resource: source.id,
        backup_id: backup.id,
        new_name: body.new_name,
        idempotency_key: restore_key,
        request_id,
        quota_bytes: quota,
    };
    let stage_for_restore = stage.clone();
    let restored = tokio::task::spawn_blocking(move || {
        restore_downloaded_namespace(&storage, &stage_for_restore, &operation)
    })
    .await;
    let _ = std::fs::remove_file(stage);
    match restored {
        Ok(Ok(CreateResourceOutcome::Applied(result))) => {
            restore_metric.success();
            json_response(&result, StatusCode::CREATED)
        }
        Ok(Ok(CreateResourceOutcome::Replay(bytes))) => {
            restore_metric.success();
            json_bytes(bytes, StatusCode::OK)
        }
        Ok(Err(error)) => error_response(error, request_id),
        Err(_) => error_response(internal(), request_id),
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
    quota_bytes: u64,
}

fn restore_downloaded_namespace(
    storage: &PlatformStorage,
    source: &std::path::Path,
    operation: &RestoreOperation,
) -> Result<CreateResourceOutcome, PlatformError> {
    let operation_now = now_ms();
    let mut canonical = Vec::new();
    canonical.extend_from_slice(b"open-compute/kv-restore/v1\0");
    canonical.extend_from_slice(operation.account_id.as_uuid().as_bytes());
    canonical.extend_from_slice(operation.backup_id.as_bytes());
    canonical.push(0);
    canonical.extend_from_slice(operation.new_name.as_bytes());
    canonical.extend_from_slice(&operation.quota_bytes.to_be_bytes());
    let fingerprint = storage.crypto().fingerprint_request(&canonical);
    let repository = ResourceRepository::new(storage.db());
    let reservation = repository.reserve_create(&ReserveResourceCreate {
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
    })?;
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
        repository.mark_ready(resource.id, now_ms())?;
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
        now_ms(),
        operation.quota_bytes,
    );
    if let Err(error) = result {
        let _ = paths.remove_namespace_staging(&staging);
        return Err(error);
    }
    Ok(staging)
}

async fn delete_backup(
    State(state): State<HttpState>,
    Path((account, backup_id)): Path<(String, String)>,
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
    if let Err(error) = idempotency_key(&request) {
        return error_response(error, request_id);
    }
    let storage = api.storage.clone();
    let selected_backup_id = backup_id.clone();
    let backup = match tokio::task::spawn_blocking(move || {
        KvNamespaceRepository::new(storage.db()).get_backup(account_id, &selected_backup_id)
    })
    .await
    {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => return error_response(error, request_id),
        Err(_) => return error_response(internal(), request_id),
    };
    if backup.state == KvBackupState::Tombstoned {
        return json_response(&serde_json::json!({ "backup": backup }), StatusCode::OK);
    }
    if backup.state == KvBackupState::Ready
        && let Some(key) = backup.object_key.as_deref()
    {
        let manifest = match api.artifacts.kv_backup_manifest_key(key) {
            Ok(value) => value,
            Err(error) => return error_response(error, request_id),
        };
        if let Err(error) = api.artifacts.delete_kv_backup(&manifest).await {
            return error_response(error, request_id);
        }
        if let Err(error) = api.artifacts.delete_kv_backup(key).await {
            return error_response(error, request_id);
        }
    }
    let storage = api.storage.clone();
    match tokio::task::spawn_blocking(move || {
        KvNamespaceRepository::new(storage.db()).tombstone_backup(account_id, &backup_id, now_ms())
    })
    .await
    {
        Ok(Ok(backup)) => json_response(
            &serde_json::json!({ "backup": backup }),
            StatusCode::ACCEPTED,
        ),
        Ok(Err(error)) => error_response(error, request_id),
        Err(_) => error_response(internal(), request_id),
    }
}

fn authorized_api<'a>(state: &'a HttpState, request: &Request) -> Option<&'a Arc<KvApiState>> {
    if authorize(state, request) {
        state.kv_api()
    } else {
        None
    }
}

fn unauthorized_or_unavailable(
    state: &HttpState,
    request: &Request,
    request_id: RequestId,
) -> Response {
    if !authorize(state, request) {
        error_response(
            PlatformError::new(
                ErrorCode::AdminAuthRequired,
                "admin authentication is required",
            ),
            request_id,
        )
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}

async fn read_json<T: for<'de> Deserialize<'de>>(request: Request) -> Result<T, PlatformError> {
    let bytes = to_bytes(request.into_body(), MAX_JSON_BODY)
        .await
        .map_err(|_| {
            PlatformError::new(
                ErrorCode::LimitInvalid,
                "control request body exceeds limit",
            )
        })?;
    serde_json::from_slice(&bytes).map_err(|_| {
        PlatformError::new(ErrorCode::ConfigInvalid, "control request JSON is invalid")
    })
}

fn idempotency_key(request: &Request) -> Result<String, PlatformError> {
    let key = request
        .headers()
        .get(IDEMPOTENCY_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if key.is_empty()
        || key.len() > 128
        || key
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
    {
        return Err(PlatformError::new(
            ErrorCode::ConfigInvalid,
            "a bounded Idempotency-Key is required",
        ));
    }
    Ok(key.to_owned())
}

fn parse_account(value: &str) -> Result<AccountId, PlatformError> {
    AccountId::from_str(value)
        .map_err(|_| PlatformError::new(ErrorCode::ConfigInvalid, "account ID is invalid"))
}

fn parse_ids(account: &str, resource: &str) -> Result<(AccountId, ResourceId), PlatformError> {
    Ok((
        parse_account(account)?,
        ResourceId::from_str(resource)
            .map_err(|_| PlatformError::new(ErrorCode::ConfigInvalid, "resource ID is invalid"))?,
    ))
}

fn request_id(request: &Request) -> RequestId {
    request
        .extensions()
        .get::<RequestId>()
        .copied()
        .unwrap_or_else(RequestId::generate)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
        })
}

fn json_response(value: &impl Serialize, status: StatusCode) -> Response {
    serde_json::to_vec(value).map_or_else(
        |_| error_response(internal(), RequestId::generate()),
        |bytes| json_bytes(bytes, status),
    )
}

fn json_bytes(bytes: Vec<u8>, status: StatusCode) -> Response {
    (
        status,
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        Body::from(bytes),
    )
        .into_response()
}

#[allow(clippy::needless_pass_by_value)]
fn error_response(error: PlatformError, request_id: RequestId) -> Response {
    let code = error.code();
    let status = match code {
        ErrorCode::ResourceNotFound => StatusCode::NOT_FOUND,
        ErrorCode::ResourceNameConflict | ErrorCode::IdempotencyConflict => StatusCode::CONFLICT,
        ErrorCode::ResourceReferenced | ErrorCode::ResourceNotReady => StatusCode::CONFLICT,
        ErrorCode::AdminAuthRequired => StatusCode::UNAUTHORIZED,
        ErrorCode::ConfigInvalid | ErrorCode::LimitInvalid => StatusCode::BAD_REQUEST,
        ErrorCode::KvStorageFull
        | ErrorCode::KvUnavailable
        | ErrorCode::KvBusy
        | ErrorCode::S3Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (
        status,
        axum::Json(serde_json::json!({
            "ok": false,
            "error": {
                "code": code.as_str(),
                "message": "control request failed",
                "requestId": request_id,
            }
        })),
    )
        .into_response()
}

fn internal() -> PlatformError {
    PlatformError::new(ErrorCode::Internal, "KV control operation failed")
}

async fn fail_backup(storage: &Arc<PlatformStorage>, backup_id: &str, code: ErrorCode) {
    let storage = storage.clone();
    let backup_id = backup_id.to_owned();
    let _ = tokio::task::spawn_blocking(move || {
        KvNamespaceRepository::new(storage.db()).fail_backup(&backup_id, code, now_ms())
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
        ErrorCode::S3Unavailable,
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

fn hash_file(path: &std::path::Path) -> Result<([u8; 32], u64), PlatformError> {
    use sha2::Digest as _;
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
#[path = "kv_http_tests.rs"]
mod tests;
