//! Release-owned operator dashboard bootstrap and dispatch

use crate::embedded_dashboard::{embedded_dashboard_assets_sha256, embedded_dashboard_files};
use crate::runtime_bridge::{DispatchTarget, WorkerdTransport};
use bytes::Bytes;
use futures::stream;
use open_compute_artifacts::{ARTIFACT_KEY_VERSION, ArtifactRef, ArtifactStore};
use open_compute_core::{AccountId, ErrorCode, PlatformError, RequestId, VersionId};
use open_compute_storage::{
    PlatformStorage, SystemOwnedVersionKind, SystemOwnedVersionRecord, VersionAssetsRepository,
    VersionState, WorkerOwnership, WorkerRepository,
};
use open_compute_workers::{
    AssetEntryV1, AssetManifestV1, AssetRoutingConfigV1, BundleLimits, CreateVersionOutcome,
    CreateVersionRequest, HtmlHandling, NotFoundHandling, RunWorkerFirst, RuntimeValidator,
    VersionAssets, VersionContent, VersionController,
};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

pub use open_compute_storage::SYSTEM_DASHBOARD_WORKER_NAME;

/// Frozen dashboard version target installed at startup.
#[derive(Clone, Debug)]
pub struct DashboardDispatch {
    target: DispatchTarget,
    transport: WorkerdTransport,
}

impl DashboardDispatch {
    pub(crate) fn new(target: DispatchTarget, transport: WorkerdTransport) -> Self {
        Self { target, transport }
    }

    /// Dispatch one unauthenticated dashboard request through stock workerd assets routing.
    pub async fn dispatch(
        &self,
        request: axum::http::Request<axum::body::Body>,
    ) -> Result<axum::response::Response, PlatformError> {
        self.transport.dispatch(self.target.clone(), request).await
    }
}

/// Bootstrap or refresh the system-owned dashboard version when enabled.
pub async fn bootstrap_dashboard(
    storage: Arc<PlatformStorage>,
    artifacts: ArtifactStore,
    transport: WorkerdTransport,
    account_id: AccountId,
    bundle_limits: BundleLimits,
) -> Result<DashboardDispatch, PlatformError> {
    let repo = WorkerRepository::new(storage.db());
    let request_id = RequestId::generate();
    let now = now_ms();
    let worker = repo.ensure_system_dashboard_worker(account_id, request_id, now)?;
    if worker.ownership != WorkerOwnership::System {
        return Err(PlatformError::new(
            ErrorCode::Internal,
            "dashboard worker must be system-owned",
        ));
    }

    let assets_sha256 = decode_assets_sha256(embedded_dashboard_assets_sha256())?;
    let version = if let Some(pin) =
        repo.get_system_owned_version(SystemOwnedVersionKind::Dashboard)?
        && pin.assets_sha256 == assets_sha256
        && worker.active_version_id == pin.active_version_id
        && let Some(active) = worker.active_version_id
    {
        let version = repo.get_worker_version(account_id, worker.id, active)?;
        if version.state == VersionState::Ready && version.deleted_at_ms.is_none() {
            ensure_dashboard_artifacts(&storage, &artifacts, version.id).await?;
            version
        } else {
            create_dashboard_version(
                &storage,
                &artifacts,
                &transport,
                account_id,
                worker.id,
                bundle_limits,
            )
            .await?
        }
    } else {
        create_dashboard_version(
            &storage,
            &artifacts,
            &transport,
            account_id,
            worker.id,
            bundle_limits,
        )
        .await?
    };

    let pinned = SystemOwnedVersionRecord {
        kind: SystemOwnedVersionKind::Dashboard,
        account_id,
        worker_id: worker.id,
        active_version_id: Some(version.id),
        assets_sha256,
        updated_at_ms: now_ms(),
    };
    repo.pin_system_owned_version(&pinned)?;

    let worker = repo.get_worker(account_id, worker.id)?;
    let target = DispatchTarget {
        account_id,
        worker_id: worker.id,
        version_id: version.id,
        worker_code_sha256: hex::encode(version.worker_code_sha256),
        entrypoint: None,
        route_generation: i64::try_from(worker.route_generation).unwrap_or(i64::MAX),
        request_id: RequestId::generate(),
    };
    Ok(DashboardDispatch::new(target, transport))
}

async fn create_dashboard_version(
    storage: &PlatformStorage,
    artifacts: &ArtifactStore,
    transport: &WorkerdTransport,
    account_id: AccountId,
    worker_id: open_compute_core::WorkerId,
    bundle_limits: BundleLimits,
) -> Result<open_compute_storage::VersionRecord, PlatformError> {
    let assets = upload_embedded_assets(artifacts).await?;
    let idempotency_key = format!("system-dashboard:{}", embedded_dashboard_assets_sha256());
    let validator: Arc<dyn RuntimeValidator> = Arc::new(transport.clone());
    let controller = VersionController::new(storage, artifacts.clone(), validator, bundle_limits);
    let outcome = controller
        .create_version(CreateVersionRequest {
            account_id,
            worker_id,
            idempotency_key,
            content: VersionContent::AssetsOnly { assets },
            vars: Default::default(),
            secrets: Default::default(),
            bindings: Default::default(),
            services: Default::default(),
            runtime_features: Default::default(),
            queue_consumers: Vec::new(),
            crons: Default::default(),
            promote: true,
            request_id: RequestId::generate(),
            now_ms: now_ms(),
        })
        .await?;
    match outcome {
        CreateVersionOutcome::Applied(result) => Ok(result.version),
        CreateVersionOutcome::Replay(_) => {
            let repo = WorkerRepository::new(storage.db());
            let active = repo
                .get_worker(account_id, worker_id)?
                .active_version_id
                .ok_or_else(|| {
                    PlatformError::new(
                        ErrorCode::Internal,
                        "dashboard version replay has no active version",
                    )
                })?;
            let version = repo.get_worker_version(account_id, worker_id, active)?;
            ensure_dashboard_artifacts(storage, artifacts, version.id).await?;
            Ok(version)
        }
    }
}

async fn ensure_dashboard_artifacts(
    storage: &PlatformStorage,
    artifacts: &ArtifactStore,
    version_id: VersionId,
) -> Result<(), PlatformError> {
    if dashboard_artifacts_available(storage, artifacts, version_id).await? {
        return Ok(());
    }
    upload_embedded_assets(artifacts).await?;
    Ok(())
}

async fn dashboard_artifacts_available(
    storage: &PlatformStorage,
    artifacts: &ArtifactStore,
    version_id: VersionId,
) -> Result<bool, PlatformError> {
    let blobs = VersionAssetsRepository::new(storage.db()).list_asset_blobs(version_id)?;
    if blobs.is_empty() {
        return Ok(false);
    }
    for (sha256, size) in blobs {
        let artifact =
            ArtifactRef::new(ARTIFACT_KEY_VERSION, &hex::encode(sha256), size).map_err(|_| {
                PlatformError::new(ErrorCode::Internal, "dashboard asset reference is invalid")
            })?;
        if artifacts.head(&artifact).await.is_err() {
            return Ok(false);
        }
    }
    Ok(true)
}

async fn upload_embedded_assets(artifacts: &ArtifactStore) -> Result<VersionAssets, PlatformError> {
    let mut entries = Vec::with_capacity(embedded_dashboard_files().len());
    for (relative, bytes) in embedded_dashboard_files() {
        let path = format!("/{}", relative.trim_start_matches('/'));
        let digest = hex::encode(Sha256::digest(bytes));
        artifacts
            .put_verified(
                stream::once(
                    async move { Ok::<Bytes, std::io::Error>(Bytes::copy_from_slice(bytes)) },
                ),
                &digest,
                bytes.len() as u64,
            )
            .await?;
        entries.push(AssetEntryV1 {
            path,
            sha256: digest,
            size: bytes.len() as u64,
            content_type: content_type(relative),
        });
    }
    entries.sort_by(|left, right| left.path.as_bytes().cmp(right.path.as_bytes()));
    Ok(VersionAssets {
        manifest: AssetManifestV1 {
            schema_version: 1,
            entries,
        },
        routing: AssetRoutingConfigV1 {
            schema_version: 1,
            binding: None,
            run_worker_first: RunWorkerFirst::All(false),
            html_handling: HtmlHandling::AutoTrailingSlash,
            not_found_handling: NotFoundHandling::SinglePageApplication,
            headers: Vec::new(),
            redirects: Vec::new(),
        },
    })
}

pub(crate) fn decode_assets_sha256(value: &str) -> Result<[u8; 32], PlatformError> {
    let bytes = hex::decode(value).map_err(|_| {
        PlatformError::new(ErrorCode::Internal, "embedded dashboard digest is invalid")
    })?;
    bytes.try_into().map_err(|_| {
        PlatformError::new(
            ErrorCode::Internal,
            "embedded dashboard digest length is invalid",
        )
    })
}

pub(crate) fn content_type(path: &str) -> String {
    let filename = path.rsplit('/').next().unwrap_or(path);
    let extension = filename
        .rsplit('.')
        .next()
        .filter(|_| filename.contains('.'))
        .unwrap_or("")
        .to_ascii_lowercase();
    match extension.as_str() {
        "css" => "text/css; charset=utf-8",
        "htm" | "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" | "map" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "txt" => "text/plain; charset=utf-8",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
    .to_owned()
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
        })
}
