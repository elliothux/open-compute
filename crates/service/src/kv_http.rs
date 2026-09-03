//! P0.4 KV namespace control API.

use crate::binding_backend::KvBindingExecutor;
use crate::http::{HttpState, ProductErrorCode, authorize};
use crate::kv_backend::{KvCommand, KvCommandResult, SqliteKvBindingExecutor};
use crate::metrics::{KvLifecycle, KvLifecycleGuard};
use crate::operator_binding::operator_binding;
use crate::snapshot_pins::SnapshotPins;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::{Path, Query, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use open_compute_artifacts::ArtifactStore;
use open_compute_core::{
    AccountId, BindingKind, ErrorCode, KvConfig, PlatformError, RequestId, ResourceId,
    ResourceState,
};
use open_compute_storage::{
    CatalogDirection, CatalogSort, DEFAULT_CATALOG_LIST_LIMIT, KV_MAX_LIST_LIMIT,
    KV_MAX_VALUE_BYTES, KvBackupState, KvEngine, KvNamespaceRepository, KvPaths, PlatformStorage,
    ReserveResourceCreate, ResourceCreateReservation, ResourceRepository, decode_catalog_cursor,
    normalize_catalog_limit,
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
/// Maximum JSON body size for operator KV value PUT requests.
pub(crate) const KV_OPERATOR_PUT_MAX_BODY: usize = KV_MAX_VALUE_BYTES + 128 * 1024;
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
    executor: Arc<SqliteKvBindingExecutor>,
    config: KvConfig,
    max_resources_per_account: u32,
    delete_drain_timeout: Duration,
    snapshot_pins: Arc<SnapshotPins>,
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
        executor: Arc<SqliteKvBindingExecutor>,
        config: KvConfig,
        max_resources_per_account: u32,
        delete_drain_timeout: Duration,
    ) -> Self {
        Self {
            storage,
            artifacts,
            pins,
            executor,
            config,
            max_resources_per_account,
            delete_drain_timeout,
            snapshot_pins: Arc::new(SnapshotPins::empty()),
        }
    }

    /// Use the authenticated immutable-object pins frozen at daemon startup.
    #[must_use]
    pub(crate) fn with_snapshot_pins(mut self, pins: Arc<SnapshotPins>) -> Self {
        self.snapshot_pins = pins;
        self
    }

    pub(crate) fn storage(&self) -> &Arc<PlatformStorage> {
        &self.storage
    }

    pub(crate) fn pins(&self) -> &ResourcePins {
        &self.pins
    }

    pub(crate) fn executor(&self) -> &Arc<SqliteKvBindingExecutor> {
        &self.executor
    }

    pub(crate) const fn config(&self) -> &KvConfig {
        &self.config
    }

    pub(crate) const fn delete_drain_timeout(&self) -> Duration {
        self.delete_drain_timeout
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
        .route(
            "/v1/accounts/{account_id}/kv/namespaces/{resource_id}/keys",
            get(list_keys),
        )
        .route(
            "/v1/accounts/{account_id}/kv/namespaces/{resource_id}/values/{key}",
            get(get_value).put(put_value).delete(delete_value),
        )
}

/// Returns true when `path` targets an operator KV value mutation with a concrete key segment.
pub(crate) fn operator_kv_value_put_path(path: &str) -> bool {
    const PREFIX: &str = "/operator/api/v1/accounts/";
    if !path.starts_with(PREFIX) {
        return false;
    }
    let Some(values_index) = path.find("/values/") else {
        return false;
    };
    path.contains("/kv/namespaces/") && values_index + "/values/".len() < path.len()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListNamespacesQuery {
    search: Option<String>,
    status: Option<ResourceState>,
    sort: Option<CatalogSort>,
    direction: Option<CatalogDirection>,
    cursor: Option<String>,
    limit: Option<u16>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListKeysQuery {
    prefix: Option<String>,
    cursor: Option<String>,
    limit: Option<u16>,
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
    query: Result<Query<ListNamespacesQuery>, axum::extract::rejection::QueryRejection>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request) else {
        return unauthorized_or_unavailable(&state, &request, request_id);
    };
    let Ok(Query(query)) = query else {
        return error_response(
            PlatformError::new(
                ErrorCode::ConfigInvalid,
                "KV namespace list query is invalid",
            ),
            request_id,
        );
    };
    let account_id = match parse_account(&account) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let after = match query.cursor.as_deref() {
        None => None,
        Some(cursor) => match decode_catalog_cursor(cursor) {
            Ok(value) => Some(value),
            Err(error) => return error_response(error, request_id),
        },
    };
    let limit = normalize_catalog_limit(query.limit.unwrap_or(DEFAULT_CATALOG_LIST_LIMIT));
    let sort = query.sort.unwrap_or(CatalogSort::Name);
    let direction = query.direction.unwrap_or(CatalogDirection::Asc);
    let search = query
        .search
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let storage = api.storage.clone();
    match tokio::task::spawn_blocking(move || {
        KvNamespaceRepository::new(storage.db()).list_page(
            account_id,
            search.as_deref(),
            query.status,
            sort,
            direction,
            after,
            limit,
        )
    })
    .await
    {
        Ok(Ok(page)) => json_response(
            &serde_json::json!({
                "namespaces": page.items,
                "cursor": page.next_cursor,
                "listComplete": page.next_cursor.is_none(),
            }),
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
    let admission = match api.storage.reserve_mutation(64 * 1024) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let storage = api.storage.clone();
    let pins = api.pins.clone();
    let quota = api.config.namespace_quota_bytes;
    match tokio::task::spawn_blocking(move || {
        let _admission = admission;
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
    let _admission = match api
        .storage
        .reserve_mutation(api.config.namespace_quota_bytes)
    {
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
        crate::sqlite_staging::remove_sqlite_staging(&stage_for_backup);
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
            crate::sqlite_staging::remove_sqlite_staging(&stage);
            fail_backup(&api.storage, &backup.id, error.code()).await;
            return error_response(error, request_id);
        }
        Err(_) => {
            drop(pin);
            crate::sqlite_staging::remove_sqlite_staging(&stage);
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
    let _admission = match api.storage.reserve_mutation(size.saturating_mul(2).max(1)) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
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
        max_resources_per_account: api.max_resources_per_account,
    };
    let stage_for_restore = stage.clone();
    let restored = tokio::task::spawn_blocking(move || {
        restore_downloaded_namespace(&storage, &stage_for_restore, &operation)
    })
    .await;
    crate::sqlite_staging::remove_sqlite_staging(&stage);
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
    max_resources_per_account: u32,
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
        if let Err(error) = api.snapshot_pins.ensure_unpinned(key) {
            return error_response(error, request_id);
        }
        let manifest = match api.artifacts.kv_backup_manifest_key(key) {
            Ok(value) => value,
            Err(error) => return error_response(error, request_id),
        };
        if let Err(error) = api.snapshot_pins.ensure_unpinned(&manifest) {
            return error_response(error, request_id);
        }
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

async fn list_keys(
    State(state): State<HttpState>,
    Path((account, resource)): Path<(String, String)>,
    query: Result<Query<ListKeysQuery>, axum::extract::rejection::QueryRejection>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request) else {
        return unauthorized_or_unavailable(&state, &request, request_id);
    };
    let Ok(Query(query)) = query else {
        return error_response(
            PlatformError::new(ErrorCode::ConfigInvalid, "KV key list query is invalid"),
            request_id,
        );
    };
    let (account_id, resource_id) = match parse_ids(&account, &resource) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let prefix = query.prefix.unwrap_or_default();
    let limit = query.limit.unwrap_or(100).clamp(1, KV_MAX_LIST_LIMIT);
    let storage = api.storage.clone();
    let executor = api.executor.clone();
    let cursor = query.cursor;
    match tokio::task::spawn_blocking(move || {
        let binding =
            operator_binding(&storage, account_id, resource_id, BindingKind::KvNamespace)?;
        let result = executor.execute(
            &binding,
            KvCommand::List {
                prefix,
                limit,
                cursor,
            },
        )?;
        Ok::<_, PlatformError>(result)
    })
    .await
    {
        Ok(Ok(KvCommandResult::List {
            rows,
            complete,
            cursor,
        })) => {
            let keys: Vec<serde_json::Value> = rows
                .into_iter()
                .filter_map(|row| {
                    let name = String::from_utf8(row.key).ok()?;
                    let expiration = row.expires_at_ms.map(|ms| ms / 1000);
                    let metadata: Option<serde_json::Value> = row
                        .metadata_json
                        .as_deref()
                        .and_then(|bytes| serde_json::from_slice(bytes).ok());
                    Some(serde_json::json!({
                        "name": name,
                        "expiration": expiration,
                        "metadata": metadata,
                    }))
                })
                .collect();
            json_response(
                &serde_json::json!({
                    "keys": keys,
                    "cursor": cursor,
                    "listComplete": complete,
                }),
                StatusCode::OK,
            )
        }
        Ok(Ok(_)) => error_response(internal(), request_id),
        Ok(Err(error)) => error_response(error, request_id),
        Err(_) => error_response(internal(), request_id),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PutValueBody {
    value: String,
    metadata: Option<serde_json::Value>,
    expiration: Option<u64>,
    expiration_ttl: Option<u64>,
}

async fn put_value(
    State(state): State<HttpState>,
    Path((account, resource, key)): Path<(String, String, String)>,
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
    if key.is_empty() {
        return error_response(
            PlatformError::new(ErrorCode::ConfigInvalid, "KV key is invalid"),
            request_id,
        );
    }
    if let Err(error) = idempotency_key(&request) {
        return error_response(error, request_id);
    }
    let body = match read_json_with_limit::<PutValueBody>(request, KV_OPERATOR_PUT_MAX_BODY).await {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let value = body.value.into_bytes();
    if value.len() > KV_MAX_VALUE_BYTES {
        return error_response(
            PlatformError::new(ErrorCode::LimitInvalid, "KV value exceeds limit"),
            request_id,
        );
    }
    let admission = match api.storage.reserve_mutation(value.len() as u64 + 64 * 1024) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let metadata_present = body.metadata.is_some();
    let storage = api.storage.clone();
    let executor = api.executor.clone();
    let response_key = key.clone();
    let command_key = key;
    match tokio::task::spawn_blocking(move || {
        let _admission = admission;
        let binding =
            operator_binding(&storage, account_id, resource_id, BindingKind::KvNamespace)?;
        let result = executor.execute(
            &binding,
            KvCommand::Put {
                key: command_key,
                value,
                expiration: body.expiration,
                expiration_ttl: body.expiration_ttl,
                metadata: body.metadata,
                metadata_present,
            },
        )?;
        Ok::<_, PlatformError>(result)
    })
    .await
    {
        Ok(Ok(KvCommandResult::Mutation)) => {
            json_response(&serde_json::json!({ "key": response_key }), StatusCode::OK)
        }
        Ok(Ok(_)) => error_response(internal(), request_id),
        Ok(Err(error)) => error_response(error, request_id),
        Err(_) => error_response(internal(), request_id),
    }
}

async fn delete_value(
    State(state): State<HttpState>,
    Path((account, resource, key)): Path<(String, String, String)>,
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
    if key.is_empty() {
        return error_response(
            PlatformError::new(ErrorCode::ConfigInvalid, "KV key is invalid"),
            request_id,
        );
    }
    if let Err(error) = idempotency_key(&request) {
        return error_response(error, request_id);
    }
    let admission = match api.storage.reserve_mutation(64 * 1024) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let storage = api.storage.clone();
    let executor = api.executor.clone();
    let response_key = key.clone();
    let command_key = key;
    match tokio::task::spawn_blocking(move || {
        let _admission = admission;
        let binding =
            operator_binding(&storage, account_id, resource_id, BindingKind::KvNamespace)?;
        let result = executor.execute(&binding, KvCommand::Delete { key: command_key })?;
        Ok::<_, PlatformError>(result)
    })
    .await
    {
        Ok(Ok(KvCommandResult::Mutation)) => {
            json_response(&serde_json::json!({ "key": response_key }), StatusCode::OK)
        }
        Ok(Ok(_)) => error_response(internal(), request_id),
        Ok(Err(error)) => error_response(error, request_id),
        Err(_) => error_response(internal(), request_id),
    }
}

async fn get_value(
    State(state): State<HttpState>,
    Path((account, resource, key)): Path<(String, String, String)>,
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
    if key.is_empty() {
        return error_response(
            PlatformError::new(ErrorCode::ConfigInvalid, "KV key is invalid"),
            request_id,
        );
    }
    let storage = api.storage.clone();
    let executor = api.executor.clone();
    match tokio::task::spawn_blocking(move || {
        let binding =
            operator_binding(&storage, account_id, resource_id, BindingKind::KvNamespace)?;
        let result = executor.execute(
            &binding,
            KvCommand::Get {
                keys: vec![key],
                cache_ttl: None,
            },
        )?;
        Ok::<_, PlatformError>(result)
    })
    .await
    {
        Ok(Ok(KvCommandResult::Entries(mut entries))) => {
            let entry = entries.pop().flatten();
            let (value, metadata) = entry.map_or((None, None), |entry| {
                let metadata: Option<serde_json::Value> = entry
                    .metadata_json
                    .as_deref()
                    .and_then(|bytes| serde_json::from_slice(bytes).ok());
                let value = match String::from_utf8(entry.value.clone()) {
                    Ok(text) => Some(text),
                    Err(raw) => Some(STANDARD.encode(raw.into_bytes())),
                };
                (value, metadata)
            });
            json_response(
                &serde_json::json!({ "value": value, "metadata": metadata }),
                StatusCode::OK,
            )
        }
        Ok(Ok(_)) => error_response(internal(), request_id),
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
    read_json_with_limit(request, MAX_JSON_BODY).await
}

async fn read_json_with_limit<T: for<'de> Deserialize<'de>>(
    request: Request,
    limit: usize,
) -> Result<T, PlatformError> {
    let bytes = to_bytes(request.into_body(), limit).await.map_err(|_| {
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
        ErrorCode::QuotaExceeded | ErrorCode::AdmissionBusy => StatusCode::TOO_MANY_REQUESTS,
        ErrorCode::StoragePressure | ErrorCode::DiskHardLimit | ErrorCode::KvStorageFull => {
            StatusCode::INSUFFICIENT_STORAGE
        }
        ErrorCode::PlatformUnavailable => StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::KvUnavailable | ErrorCode::KvBusy | ErrorCode::S3Unavailable => {
            StatusCode::SERVICE_UNAVAILABLE
        }
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let mut response = (
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
        .into_response();
    response.extensions_mut().insert(ProductErrorCode(code));
    response
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
