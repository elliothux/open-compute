//! Immutable Version content cloning for metadata-only mutations.

use super::domain::UploadInput;
use super::model::WorkerUploadMetadata;
use crate::workers_http::WorkerApiState;
use open_compute_artifacts::{ARTIFACT_KEY_VERSION, ArtifactRef};
use open_compute_core::{AccountId, ErrorCode, PlatformError, RequestId, SecretString};
use open_compute_storage::{
    CronRepository, DeploymentSource, QueueConsumerRepository, VersionSnapshot, WorkerRecord,
    WorkerRepository,
};
use open_compute_workers::{
    AssetManifestV1, AssetRoutingConfigV1, CreateVersionOutcome, CreateVersionRequest,
    QueueConsumerInput, RuntimeValidator, VersionAssets, VersionBundle, VersionCachePolicyInput,
    VersionContent, VersionController,
};
use std::collections::BTreeMap;
use std::sync::Arc;

pub(super) async fn clone_content(
    api: &WorkerApiState,
    snapshot: &VersionSnapshot,
) -> Result<VersionContent, PlatformError> {
    let assets = snapshot
        .assets
        .as_ref()
        .map(|stored| -> Result<VersionAssets, PlatformError> {
            Ok(VersionAssets {
                manifest: serde_json::from_slice::<AssetManifestV1>(&stored.manifest_json)
                    .map_err(|_| invariant())?,
                routing: serde_json::from_slice::<AssetRoutingConfigV1>(
                    &stored.routing_config_json,
                )
                .map_err(|_| invariant())?,
            })
        })
        .transpose()?;
    if snapshot.version.content_kind == open_compute_storage::VersionContentKind::AssetsOnly {
        return Ok(VersionContent::AssetsOnly {
            assets: assets.ok_or_else(invariant)?,
        });
    }
    let digest = snapshot.version.artifact_sha256.ok_or_else(invariant)?;
    let size = snapshot.version.artifact_size.ok_or_else(invariant)?;
    let artifact = ArtifactRef::new(ARTIFACT_KEY_VERSION, &hex::encode(digest), size)?;
    let bytes = api.artifacts.open(&artifact).await?;
    Ok(VersionContent::Worker {
        bundle: VersionBundle::Bytes(bytes.to_vec()),
        assets,
    })
}

/// Clone the active immutable Version, changing only secret and Cron declarations.
pub(super) async fn clone_active(
    api: &WorkerApiState,
    account_id: AccountId,
    worker: &WorkerRecord,
    secret_updates: BTreeMap<String, Option<SecretString>>,
    crons: Option<Vec<String>>,
    request_id: RequestId,
    now_ms: i64,
) -> Result<CreateVersionOutcome, PlatformError> {
    let active = worker.active_version_id.ok_or_else(|| {
        PlatformError::new(
            ErrorCode::VersionNotReady,
            "Script has no active Version to update",
        )
    })?;
    let snapshot = WorkerRepository::new(api.storage.db())
        .version_snapshot(account_id, worker.id, active, false)?;
    let content = clone_content(api, &snapshot).await?;
    let mut input = UploadInput::new(WorkerUploadMetadata {
        main_module: snapshot.version.main_module.clone(),
        body_part: None,
        compatibility_date: snapshot.version.compatibility_date.clone(),
        compatibility_flags: snapshot.version.compatibility_flags.clone(),
        bindings: Vec::new(),
        keep_bindings: [
            "plain_text",
            "json",
            "secret_text",
            "kv_namespace",
            "r2_bucket",
            "d1",
            "durable_object_namespace",
            "vectorize",
            "ai_search_namespace",
            "ai_search",
            "queue",
            "workflow",
            "service",
            "ai",
            "images",
            "version_metadata",
            "wasm_module",
            "text_blob",
            "data_blob",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        annotations: BTreeMap::new(),
        assets: None,
        cache_options: None,
        exports: None,
        migrations: None,
    })?;
    input.apply_inheritance(api, Some(&snapshot), true)?;
    for (name, value) in secret_updates {
        match value {
            Some(value) => {
                input.secrets.insert(name, value);
            }
            None if input.secrets.remove(&name).is_none() => {
                return Err(PlatformError::new(
                    ErrorCode::BindingNotFound,
                    "Secret binding was not found",
                ));
            }
            None => {}
        }
    }
    for policy in &snapshot.cache_policies {
        let value = VersionCachePolicyInput {
            enabled: policy.enabled,
            cross_version_cache: policy.cross_version_cache,
        };
        if let Some(entrypoint) = &policy.entrypoint {
            input
                .runtime_features
                .cache
                .entrypoints
                .insert(entrypoint.clone(), value);
        } else {
            input.runtime_features.cache.default = value;
        }
    }
    let queue_consumers = QueueConsumerRepository::new(api.storage.db())
        .version_declarations(active)?
        .into_iter()
        .map(|declaration| QueueConsumerInput {
            queue: declaration.queue_id,
            entrypoint: declaration.entrypoint,
            config: declaration.config,
            dead_letter_queue: declaration.dlq_queue_id,
        })
        .collect();
    let crons = crons.unwrap_or(
        CronRepository::new(api.storage.db())
            .version_config(active)?
            .declarations
            .into_iter()
            .map(|declaration| declaration.expression)
            .collect(),
    );
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
    controller
        .create_version(CreateVersionRequest {
            account_id,
            worker_id: worker.id,
            idempotency_key: format!("v4/{request_id}"),
            content,
            vars: input.vars,
            secrets: input.secrets,
            bindings: input.bindings,
            services: input.services,
            runtime_features: input.runtime_features,
            queue_consumers,
            crons,
            deployment_source: Some(DeploymentSource::VersionsApi),
            request_id,
            now_ms,
        })
        .await
}

fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::VersionInvariantViolation,
        "persisted Version authority is inconsistent",
    )
}
