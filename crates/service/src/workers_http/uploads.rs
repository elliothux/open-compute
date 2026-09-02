//! Resumable static-asset version upload HTTP surface.

use super::*;
use bytes::Bytes;
use futures::stream;
use open_compute_artifacts::{ARTIFACT_KEY_VERSION, ArtifactRef};
use open_compute_core::VersionUploadId;
use open_compute_storage::{
    BeginVersionUploadFinalize, NewVersionUpload, NewVersionUploadObject, VersionContentKind,
    VersionObjectKind, VersionUploadFinalizeDisposition, VersionUploadRecord,
    VersionUploadRepository,
};
use open_compute_workers::{
    AssetManifestV1, AssetRoutingConfigV1, MAX_ASSET_MANIFEST_BYTES, VersionAssets, VersionContent,
};
use serde::Serialize;
use std::collections::btree_map::Entry;
use std::io::Write as _;

const MAX_UPLOAD_CREATE_BODY: usize = MAX_ASSET_MANIFEST_BYTES + 1024 * 1024;
const UPLOAD_TTL_MS: i64 = 24 * 60 * 60 * 1000;
const MAX_OPEN_UPLOADS_PER_WORKER: u32 = 2;
const MAX_OPEN_UPLOADS_PER_ACCOUNT: u32 = 4;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BundleInventory {
    sha256: String,
    size: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateUploadBody {
    content_kind: VersionContentKind,
    #[serde(default)]
    bundle: Option<BundleInventory>,
    manifest: AssetManifestV1,
    routing: AssetRoutingConfigV1,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FinalizeUploadBody {
    #[serde(default)]
    main_module: Option<String>,
    #[serde(default)]
    vars: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    secrets: BTreeMap<String, SecretString>,
    #[serde(default)]
    bindings: BTreeMap<String, VersionBindingInput>,
    #[serde(default)]
    services: BTreeMap<String, VersionServiceInput>,
    #[serde(flatten)]
    runtime_features: VersionRuntimeFeatures,
    #[serde(default)]
    queue_consumers: Vec<QueueConsumerInput>,
    #[serde(default)]
    crons: Vec<String>,
    #[serde(default)]
    promote: bool,
}

pub(super) async fn create_version_upload(
    State(state): State<HttpState>,
    Path((account, worker)): Path<(String, String)>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request, request_id) else {
        return unauthorized_or_unavailable(&state, &request, request_id);
    };
    let (account_id, worker_id) = match parse_ids(&account, &worker) {
        Ok(ids) => ids,
        Err(error) => return error_response(error, request_id),
    };
    let key = match idempotency_key(&request) {
        Ok(key) => key,
        Err(error) => return error_response(error, request_id),
    };
    let body = match read_json::<CreateUploadBody>(request, MAX_UPLOAD_CREATE_BODY).await {
        Ok(body) => body,
        Err(error) => return error_response(error, request_id),
    };
    let result = create_session(api, account_id, worker_id, &key, &body, request_id).await;
    result_response_with_status(
        result.map(|session| upload_json(&session)),
        StatusCode::CREATED,
        request_id,
    )
}

async fn create_session(
    api: &WorkerApiState,
    account_id: AccountId,
    worker_id: WorkerId,
    key: &str,
    body: &CreateUploadBody,
    _request_id: RequestId,
) -> Result<VersionUploadRecord, PlatformError> {
    body.manifest.validate()?;
    body.routing.validate()?;
    let manifest_json = body.manifest.canonical_bytes()?;
    let routing_json = body.routing.canonical_bytes()?;
    let manifest_sha256 = body.manifest.sha256()?;
    let bundle = body
        .bundle
        .as_ref()
        .map(|value| parse_object_identity(&value.sha256, value.size))
        .transpose()?;
    if matches!(body.content_kind, VersionContentKind::Worker) != bundle.is_some() {
        return Err(upload_conflict());
    }
    let objects = upload_inventory(&body.manifest, manifest_sha256, manifest_json.len(), bundle)?;
    let canonical = serde_json::to_vec(body).map_err(|_| internal())?;
    let fingerprint = api.storage.crypto().fingerprint_request(&canonical);
    let now = now_ms();
    let repo = VersionUploadRepository::new(api.storage.db());
    let mut session = repo.create_or_get(
        &NewVersionUpload {
            id: VersionUploadId::generate(),
            account_id,
            worker_id,
            idempotency_key: key,
            input_fingerprint: fingerprint,
            content_kind: body.content_kind,
            bundle,
            manifest_sha256,
            manifest_json: &manifest_json,
            routing_config_json: &routing_json,
            objects: &objects,
            now_ms: now,
            expires_at_ms: now.saturating_add(UPLOAD_TTL_MS),
        },
        MAX_OPEN_UPLOADS_PER_WORKER,
        MAX_OPEN_UPLOADS_PER_ACCOUNT,
    )?;
    if session
        .objects
        .iter()
        .any(|object| object.sha256 == manifest_sha256 && !object.verified)
    {
        let _artifact_lifecycle = api.artifacts.reserve_version_artifact().await;
        let artifact = api
            .artifacts
            .put_verified(
                stream::once(async {
                    Ok::<Bytes, std::io::Error>(Bytes::from(manifest_json.clone()))
                }),
                &hex::encode(manifest_sha256),
                u64::try_from(manifest_json.len()).map_err(|_| internal())?,
            )
            .await
            .map_err(|error| map_asset_error(&error))?;
        session = repo.mark_object_verified(
            account_id,
            worker_id,
            session.id,
            artifact.sha256_bytes(),
            artifact.size(),
            now_ms(),
        )?;
    }
    Ok(session)
}

pub(super) async fn get_version_upload(
    State(state): State<HttpState>,
    Path((account, worker, upload)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request, request_id) else {
        return unauthorized_or_unavailable(&state, &request, request_id);
    };
    let result = parse_upload_ids(&account, &worker, &upload).and_then(
        |(account_id, worker_id, upload_id)| {
            VersionUploadRepository::new(api.storage.db()).get(
                account_id,
                worker_id,
                upload_id,
                now_ms(),
            )
        },
    );
    result_response(result.map(|session| upload_json(&session)), request_id)
}

pub(super) async fn put_version_upload_object(
    State(state): State<HttpState>,
    Path((account, worker, upload, sha256)): Path<(String, String, String, String)>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request, request_id) else {
        return unauthorized_or_unavailable(&state, &request, request_id);
    };
    let (account_id, worker_id, upload_id) = match parse_upload_ids(&account, &worker, &upload) {
        Ok(ids) => ids,
        Err(error) => return error_response(error, request_id),
    };
    let digest = match parse_sha256(&sha256) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    let repo = VersionUploadRepository::new(api.storage.db());
    let object = match repo.object_for_upload(account_id, worker_id, upload_id, &digest, now_ms()) {
        Ok(value) => value,
        Err(error) => return error_response(error, request_id),
    };
    if object.verified {
        return result_response(
            repo.get(account_id, worker_id, upload_id, now_ms())
                .map(|session| upload_json(&session)),
            request_id,
        );
    }
    if request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|size| size != object.size)
    {
        return error_response(upload_conflict(), request_id);
    }
    let result = async {
        let _artifact_lifecycle = api.artifacts.reserve_version_artifact().await;
        let staged = stage_object(
            request.into_body(),
            api.storage.data_dir().version_staging_dir(),
            &digest,
            object.size,
        )
        .await?;
        let artifact = api
            .artifacts
            .put_verified_file(&staged.path, &sha256, object.size)
            .await
            .map_err(|error| map_asset_error(&error))?;
        repo.mark_object_verified(
            account_id,
            worker_id,
            upload_id,
            artifact.sha256_bytes(),
            artifact.size(),
            now_ms(),
        )
    }
    .await;
    result_response(result.map(|session| upload_json(&session)), request_id)
}

pub(super) async fn finalize_version_upload(
    State(state): State<HttpState>,
    Path((account, worker, upload)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request, request_id) else {
        return unauthorized_or_unavailable(&state, &request, request_id);
    };
    let (account_id, worker_id, upload_id) = match parse_upload_ids(&account, &worker, &upload) {
        Ok(ids) => ids,
        Err(error) => return error_response(error, request_id),
    };
    let metadata =
        match read_json::<FinalizeUploadBody>(request, MAX_VERSION_METADATA_HEADER_BYTES).await {
            Ok(value) => value,
            Err(error) => return error_response(error, request_id),
        };
    match finalize_session(api, account_id, worker_id, upload_id, metadata, request_id).await {
        Ok(bytes) => json_bytes(bytes, StatusCode::CREATED),
        Err(error) => error_response(error, request_id),
    }
}

async fn finalize_session(
    api: &WorkerApiState,
    account_id: AccountId,
    worker_id: WorkerId,
    upload_id: VersionUploadId,
    metadata: FinalizeUploadBody,
    request_id: RequestId,
) -> Result<Vec<u8>, PlatformError> {
    let lock_index = usize::from(upload_id.as_uuid().as_bytes()[0] & 0x0f);
    let _finalize_guard = api.finalize_locks[lock_index].lock().await;
    let repo = VersionUploadRepository::new(api.storage.db());
    let initial = repo.get(account_id, worker_id, upload_id, now_ms())?;
    let version_id = initial.version_id.unwrap_or_else(VersionId::generate);
    let finalize_input = serde_json::to_vec(&metadata).map_err(|_| internal())?;
    let finalize_fingerprint = api.storage.crypto().fingerprint_request(&finalize_input);
    let finalize = repo.begin_finalize(BeginVersionUploadFinalize {
        account_id,
        worker_id,
        upload_id,
        version_id,
        finalize_fingerprint,
        owner_startup_id: api.storage.data_dir().startup_id(),
        now_ms: now_ms(),
    })?;
    if finalize.disposition == VersionUploadFinalizeDisposition::Committed {
        if let Some(code) = finalize
            .upload
            .finalize_error_code
            .as_deref()
            .and_then(ErrorCode::from_stable_str)
        {
            return Err(PlatformError::new(
                code,
                "version upload finalization previously failed",
            ));
        }
        return finalize.upload.finalize_response_json.ok_or_else(internal);
    }
    let response =
        complete_reserved_finalize(api, version_id, finalize.upload, metadata, request_id).await;
    let response = match response {
        Ok(response) => response,
        Err(error) => {
            repo.mark_finalize_failed(
                account_id,
                worker_id,
                upload_id,
                version_id,
                error.code(),
                now_ms(),
            )?;
            return Err(error);
        }
    };
    repo.mark_committed(
        account_id,
        worker_id,
        upload_id,
        version_id,
        &response,
        now_ms(),
    )?;
    Ok(response)
}

async fn complete_reserved_finalize(
    api: &WorkerApiState,
    version_id: VersionId,
    session: VersionUploadRecord,
    metadata: FinalizeUploadBody,
    request_id: RequestId,
) -> Result<Vec<u8>, PlatformError> {
    let manifest = serde_json::from_slice::<AssetManifestV1>(&session.manifest_json)
        .map_err(|_| internal())?;
    let routing = serde_json::from_slice::<AssetRoutingConfigV1>(&session.routing_config_json)
        .map_err(|_| internal())?;
    if manifest.canonical_bytes()? != session.manifest_json
        || manifest.sha256()? != session.manifest_sha256
        || routing.canonical_bytes()? != session.routing_config_json
    {
        return Err(internal());
    }
    let assets = VersionAssets { manifest, routing };
    let mut staged_bundle = None;
    let content = match session.content_kind {
        VersionContentKind::Worker => {
            let digest = session.bundle_sha256.ok_or_else(internal)?;
            let size = session.bundle_size.ok_or_else(internal)?;
            let staged = download_bundle(api, &digest, size).await?;
            if metadata.main_module.as_deref()
                != Some(staged.bundle.manifest().main_module.as_str())
            {
                return Err(PlatformError::new(
                    ErrorCode::BundleInvalid,
                    "metadata mainModule does not match the canonical bundle",
                ));
            }
            let bundle = staged.bundle.clone();
            staged_bundle = Some(staged);
            VersionContent::Worker {
                bundle: VersionBundle::Staged(bundle),
                assets: Some(assets),
            }
        }
        VersionContentKind::AssetsOnly => {
            if metadata.main_module.is_some() {
                return Err(upload_conflict());
            }
            VersionContent::AssetsOnly { assets }
        }
    };
    let validator: Arc<dyn RuntimeValidator> = Arc::new(api.transport.clone());
    let mut controller = VersionController::new(
        &api.storage,
        api.artifacts.clone(),
        validator,
        api.bundle_limits,
    )
    .with_queue_consumer_limit(api.max_queue_consumer_concurrency);
    if let Some(promoter) = &api.product_promoter {
        controller = controller.with_product_promoter(promoter.clone());
    }
    let outcome = controller
        .finalize_upload(
            CreateVersionRequest {
                account_id: session.account_id,
                worker_id: session.worker_id,
                idempotency_key: format!("version-upload/{}", session.id),
                content,
                vars: metadata.vars,
                secrets: metadata.secrets,
                bindings: metadata.bindings,
                services: metadata.services,
                runtime_features: metadata.runtime_features,
                queue_consumers: metadata.queue_consumers,
                crons: metadata.crons,
                deployment_source: metadata
                    .promote
                    .then_some(open_compute_storage::DeploymentSource::VersionsApi),
                request_id,
                now_ms: now_ms(),
            },
            version_id,
        )
        .await;
    drop(staged_bundle);
    match outcome? {
        CreateVersionOutcome::Applied(result) => serde_json::to_vec(&serde_json::json!({
            "version": result.version.to_api_json(),
            "promoted": result.deployment.is_some(),
        }))
        .map_err(|_| internal()),
        CreateVersionOutcome::Replay(bytes) => Ok(bytes),
    }
}

pub(super) async fn abort_version_upload(
    State(state): State<HttpState>,
    Path((account, worker, upload)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let request_id = request_id(&request);
    let Some(api) = authorized_api(&state, &request, request_id) else {
        return unauthorized_or_unavailable(&state, &request, request_id);
    };
    let result = parse_upload_ids(&account, &worker, &upload).and_then(
        |(account_id, worker_id, upload_id)| {
            VersionUploadRepository::new(api.storage.db()).abort(
                account_id,
                worker_id,
                upload_id,
                now_ms(),
            )
        },
    );
    result_response(result.map(|session| upload_json(&session)), request_id)
}

struct StagedObject {
    path: PathBuf,
    _cleanup: StagingCleanup,
}

async fn stage_object(
    mut body: Body,
    directory: PathBuf,
    expected_sha256: &[u8; 32],
    expected_size: u64,
) -> Result<StagedObject, PlatformError> {
    let path = directory.join(format!("{}.asset-upload", Uuid::now_v7()));
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(|_| {
            PlatformError::new(
                ErrorCode::DiskHardLimit,
                "failed to create asset staging file",
            )
        })?;
    let cleanup = StagingCleanup { path: path.clone() };
    let mut file = tokio::fs::File::from_std(file);
    let mut written = 0_u64;
    let mut hasher = Sha256::new();
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|_| upload_incomplete())?;
        let Ok(data) = frame.into_data() else {
            continue;
        };
        written = written
            .checked_add(u64::try_from(data.len()).map_err(|_| upload_incomplete())?)
            .ok_or_else(upload_incomplete)?;
        if written > expected_size {
            return Err(upload_conflict());
        }
        hasher.update(&data);
        file.write_all(&data).await.map_err(|_| {
            PlatformError::new(
                ErrorCode::DiskHardLimit,
                "failed to write asset staging file",
            )
        })?;
    }
    if written != expected_size || <[u8; 32]>::from(hasher.finalize()) != *expected_sha256 {
        return Err(upload_conflict());
    }
    file.sync_all().await.map_err(|_| {
        PlatformError::new(
            ErrorCode::DiskHardLimit,
            "failed to persist asset staging file",
        )
    })?;
    Ok(StagedObject {
        path,
        _cleanup: cleanup,
    })
}

async fn download_bundle(
    api: &WorkerApiState,
    digest: &[u8; 32],
    size: u64,
) -> Result<StagedUpload, PlatformError> {
    let artifact = ArtifactRef::new(ARTIFACT_KEY_VERSION, &hex::encode(digest), size)?;
    let path = api
        .storage
        .data_dir()
        .version_staging_dir()
        .join(format!("{}.upload-finalize", Uuid::now_v7()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(|_| {
            PlatformError::new(
                ErrorCode::DiskHardLimit,
                "failed to create bundle staging file",
            )
        })?;
    let cleanup = StagingCleanup { path: path.clone() };
    api.artifacts
        .download_verified(&artifact, &mut file)
        .await
        .map_err(|error| map_asset_error(&error))?;
    file.flush().and_then(|()| file.sync_all()).map_err(|_| {
        PlatformError::new(
            ErrorCode::DiskHardLimit,
            "failed to persist bundle staging file",
        )
    })?;
    drop(file);
    let bundle = StagedBundle::open(path, api.bundle_limits)?;
    Ok(StagedUpload {
        bundle,
        _cleanup: cleanup,
    })
}

fn upload_inventory(
    manifest: &AssetManifestV1,
    manifest_sha256: [u8; 32],
    manifest_size: usize,
    bundle: Option<([u8; 32], u64)>,
) -> Result<Vec<NewVersionUploadObject>, PlatformError> {
    let mut objects = BTreeMap::<[u8; 32], (VersionObjectKind, u64)>::new();
    insert_inventory(
        &mut objects,
        manifest_sha256,
        VersionObjectKind::AssetManifest,
        u64::try_from(manifest_size).map_err(|_| upload_conflict())?,
    )?;
    if let Some((digest, size)) = bundle {
        insert_inventory(&mut objects, digest, VersionObjectKind::Bundle, size)?;
    }
    for entry in &manifest.entries {
        insert_inventory(
            &mut objects,
            parse_sha256(&entry.sha256)?,
            VersionObjectKind::AssetBlob,
            entry.size,
        )?;
    }
    Ok(objects
        .into_iter()
        .map(|(sha256, (kind, size))| NewVersionUploadObject { sha256, kind, size })
        .collect())
}

fn insert_inventory(
    objects: &mut BTreeMap<[u8; 32], (VersionObjectKind, u64)>,
    digest: [u8; 32],
    kind: VersionObjectKind,
    size: u64,
) -> Result<(), PlatformError> {
    match objects.entry(digest) {
        Entry::Vacant(entry) => {
            entry.insert((kind, size));
        }
        Entry::Occupied(entry) if entry.get().1 == size => {}
        Entry::Occupied(_) => return Err(upload_conflict()),
    }
    Ok(())
}

fn parse_object_identity(value: &str, size: u64) -> Result<([u8; 32], u64), PlatformError> {
    Ok((parse_sha256(value)?, size))
}

fn parse_sha256(value: &str) -> Result<[u8; 32], PlatformError> {
    if value.len() != 64
        || value
            .bytes()
            .any(|byte| !byte.is_ascii_hexdigit() || byte.is_ascii_uppercase())
    {
        return Err(upload_conflict());
    }
    let bytes = hex::decode(value).map_err(|_| upload_conflict())?;
    bytes.try_into().map_err(|_| upload_conflict())
}

fn parse_upload_ids(
    account: &str,
    worker: &str,
    upload: &str,
) -> Result<(AccountId, WorkerId, VersionUploadId), PlatformError> {
    let (account_id, worker_id) = parse_ids(account, worker)?;
    let upload_id = VersionUploadId::from_str(upload).map_err(|_| upload_conflict())?;
    Ok((account_id, worker_id, upload_id))
}

fn upload_json(session: &VersionUploadRecord) -> serde_json::Value {
    serde_json::json!({
        "id": session.id,
        "accountId": session.account_id,
        "workerId": session.worker_id,
        "contentKind": session.content_kind,
        "status": session.status.as_str(),
        "versionId": session.version_id,
        "errorCode": session.finalize_error_code,
        "createdAtMs": session.created_at_ms,
        "expiresAtMs": session.expires_at_ms,
        "updatedAtMs": session.updated_at_ms,
        "objects": session.objects.iter().map(|object| serde_json::json!({
            "sha256": hex::encode(object.sha256),
            "kind": object.kind.as_str(),
            "size": object.size,
            "verified": object.verified,
            "verifiedAtMs": object.verified_at_ms,
        })).collect::<Vec<_>>(),
    })
}

fn result_response_with_status(
    result: Result<serde_json::Value, PlatformError>,
    status: StatusCode,
    request_id: RequestId,
) -> Response {
    match result {
        Ok(value) => json_bytes(
            serde_json::to_vec(&value).unwrap_or_else(|_| b"{}".to_vec()),
            status,
        ),
        Err(error) => error_response(error, request_id),
    }
}

fn map_asset_error(error: &PlatformError) -> PlatformError {
    match error.code() {
        ErrorCode::ArtifactIntegrityError | ErrorCode::CacheEntryCorrupt => PlatformError::new(
            ErrorCode::AssetIntegrityError,
            "static asset failed integrity verification",
        ),
        ErrorCode::LimitInvalid => PlatformError::new(
            ErrorCode::AssetLimitExceeded,
            "static asset exceeds the configured object limit",
        ),
        _ => PlatformError::new(
            ErrorCode::AssetStorageUnavailable,
            "static asset provider is unavailable",
        ),
    }
}

fn upload_incomplete() -> PlatformError {
    PlatformError::new(
        ErrorCode::AssetUploadIncomplete,
        "static asset upload stream ended before verification",
    )
}

fn upload_conflict() -> PlatformError {
    PlatformError::new(
        ErrorCode::AssetUploadConflict,
        "static asset upload conflicts with its declared inventory",
    )
}

#[cfg(test)]
#[path = "upload_tests.rs"]
mod tests;
