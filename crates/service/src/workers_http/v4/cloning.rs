//! Immutable Version content cloning for metadata-only mutations.

use super::domain::UploadInput;
use super::model::{WorkerUploadBinding, WorkerUploadMetadata, WorkerUploadResourceLimits};
use crate::cloudflare_v4::accounts::V4InstanceContext;
use crate::workers_http::WorkerApiState;
use open_compute_artifacts::{ARTIFACT_KEY_VERSION, ArtifactRef};
use open_compute_core::{ErrorCode, PlatformError, RequestId, SecretString};
use open_compute_storage::{
    CronRepository, DeploymentSource, EffectiveResourceLimits, QueueConsumerRepository,
    VersionSnapshot, WorkerRecord, WorkerRepository,
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

/// Requested changes for cloning one immutable Version.
pub(super) struct CloneVersionOptions<'a> {
    pub(super) source_version: open_compute_core::VersionId,
    pub(super) deployment_source: Option<DeploymentSource>,
    pub(super) secret_updates: BTreeMap<String, Option<SecretString>>,
    pub(super) crons: Option<Vec<String>>,
    pub(super) resource_limits: Option<EffectiveResourceLimits>,
    pub(super) binding_patch: Option<(&'a V4InstanceContext, Vec<WorkerUploadBinding>)>,
    pub(super) annotations: BTreeMap<String, String>,
    pub(super) request_id: RequestId,
    pub(super) now_ms: i64,
}

/// Clone an immutable Version with the requested changes.
pub(super) async fn clone_version(
    api: &WorkerApiState,
    worker: &WorkerRecord,
    options: CloneVersionOptions<'_>,
) -> Result<CreateVersionOutcome, PlatformError> {
    let CloneVersionOptions {
        source_version,
        deployment_source,
        secret_updates,
        crons,
        resource_limits,
        binding_patch,
        annotations,
        request_id,
        now_ms,
    } = options;
    let snapshot = WorkerRepository::new(api.storage.db()).version_snapshot(
        worker.instance_id,
        worker.id,
        source_version,
        false,
    )?;
    let content = clone_content(api, &snapshot).await?;
    let resource_limits = resource_limits.unwrap_or(snapshot.version.resource_limits);
    let replace_bindings = binding_patch.is_some();
    let mut input = UploadInput::new(WorkerUploadMetadata {
        main_module: snapshot.version.main_module.clone(),
        body_part: None,
        compatibility_date: snapshot.version.compatibility_date.clone(),
        compatibility_flags: snapshot.version.compatibility_flags.clone(),
        package_dependencies: Vec::new(),
        limits: Some(WorkerUploadResourceLimits {
            cpu_ms: Some(resource_limits.cpu_ms),
            sub_requests: Some(resource_limits.sub_requests),
        }),
        bindings: binding_patch
            .as_ref()
            .map_or_else(Vec::new, |(_, bindings)| bindings.clone()),
        keep_bindings: if replace_bindings {
            Vec::new()
        } else {
            [
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
                "artifacts",
                "queue",
                "workflow",
                "service",
                "ai",
                "images",
                "version_metadata",
                "worker_loader",
                "wasm_module",
                "text_blob",
                "data_blob",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect()
        },
        annotations,
        assets: None,
        observability: None,
        cache_options: None,
        exports: None,
        migrations: None,
    });
    input.apply_inheritance(api, Some(&snapshot), true)?;
    if let Some((authority, _)) = binding_patch {
        let reservation_owner = request_id.to_string();
        if let Err(error) = input.apply_explicit_bindings(
            api,
            authority,
            worker.instance_id,
            worker.id,
            None,
            false,
            true,
            Some(&reservation_owner),
            now_ms,
        ) {
            input.release_workflow_reservations(api, worker.instance_id, now_ms)?;
            return Err(error);
        }
    }
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
        .version_declarations(source_version)?
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
            .version_config(source_version)?
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
    let workflow_reservations = std::mem::take(&mut input.workflow_reservations);
    let outcome = controller
        .create_version(CreateVersionRequest {
            instance_id: worker.instance_id,
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
            deployment_source,
            observability: None,
            request_id,
            now_ms,
        })
        .await;
    if outcome.is_err() {
        super::domain::release_workflow_reservations(
            api,
            worker.instance_id,
            &workflow_reservations,
            now_ms,
        )?;
    }
    outcome
}

fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::VersionInvariantViolation,
        "persisted Version authority is inconsistent",
    )
}
