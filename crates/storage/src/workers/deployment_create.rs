//! Atomic immutable deployment creation and product metadata insertion.

use super::*;
use sha2::{Digest, Sha256};

/// All product metadata committed with an immutable deployment in one control transaction.
#[derive(Clone, Debug, Default)]
pub struct NewDeploymentProducts<'a> {
    /// Optional immutable static-asset metadata.
    pub assets: Option<&'a crate::NewDeploymentAssets>,
    /// Static manifest/blob object references derived from the canonical manifest.
    pub asset_object_refs: &'a [crate::NewDeploymentObjectRef],
    /// Frozen KV/R2/D1/Durable Object resource bindings.
    pub bindings: &'a [crate::NewDeploymentBinding],
    /// Frozen Queue producer bindings.
    pub queue_bindings: &'a [crate::NewQueueProducerBinding],
    /// Frozen Workflow caller bindings.
    pub workflow_bindings: &'a [crate::WorkflowBindingRecord],
    /// Queue push-consumer declarations.
    pub queue_consumers: &'a [crate::NewQueueConsumerDeclaration],
    /// Optional immutable Cron declaration set.
    pub cron: Option<&'a crate::NewCronConfig>,
}

impl WorkerRepository<'_> {
    /// Insert deployment metadata, bindings, Queue consumers, and Cron declarations atomically.
    pub fn insert_staging_deployment(
        &self,
        input: &NewDeployment,
        products: &NewDeploymentProducts<'_>,
        max_retained: u32,
    ) -> Result<DeploymentRecord, PlatformError> {
        if max_retained == 0 {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "deployment count limit must be greater than zero",
            ));
        }
        validate_deployment_shape(input, products)?;
        let flags_json = serde_json::to_vec(&input.compatibility_flags).map_err(|_| invariant())?;
        let limits_json = serde_json::to_vec(&input.limits).map_err(|_| invariant())?;
        self.db.with_immediate(|tx| {
            require_live_worker(tx, input.account_id, input.worker_id)?;
            let retained_count: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM worker_deployments
                     WHERE worker_id = ?1 AND deleted_at_ms IS NULL",
                    [input.worker_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(|_| db_error())?;
            if retained_count >= i64::from(max_retained) {
                return Err(PlatformError::new(
                    ErrorCode::QuotaExceeded,
                    "Worker deployment count quota was exceeded",
                ));
            }
            let version: i64 = tx
                .query_row(
                    "SELECT COALESCE(MAX(version_number), 0) + 1
                 FROM worker_deployments WHERE worker_id = ?1",
                    [input.worker_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(|_| db_error())?;
            tx.execute(
                "INSERT INTO worker_deployments
                 (id, worker_id, version_number, content_kind, state, artifact_sha256, artifact_size,
                  artifact_schema_version, main_module, compatibility_date,
                  compatibility_flags_json, limits_json, worker_code_sha256,
                  loader_schema_version, created_at_ms, ready_at_ms, rejected_at_ms,
                  rejection_code, deleted_at_ms)
                 VALUES (?1, ?2, ?3, ?4, 'staging', ?5, ?6, ?7, ?8, ?9,
                         ?10, ?11, ?12, ?13, ?14, NULL, NULL, NULL, NULL)",
                params![
                    input.id.to_string(),
                    input.worker_id.to_string(),
                    version,
                    input.content_kind.as_str(),
                    input.artifact_sha256.as_ref().map(<[u8; 32]>::as_slice),
                    input.artifact_size
                        .map(i64::try_from)
                        .transpose()
                        .map_err(|_| PlatformError::new(
                            ErrorCode::BundleTooLarge,
                            "bundle size exceeds SQLite integer range",
                        ))?,
                    input.artifact_schema_version.map(i64::from),
                    input.main_module,
                    input.compatibility_date,
                    flags_json,
                    limits_json,
                    input.worker_code_sha256.as_slice(),
                    LOADER_SCHEMA_VERSION,
                    input.now_ms,
                ],
            )
            .map_err(|_| db_error())?;
            if let (Some(digest), Some(size)) = (input.artifact_sha256, input.artifact_size) {
                crate::assets::insert_bundle_object_ref(tx, input.id, &digest, size, input.now_ms)?;
            }
            if let Some(assets) = products.assets {
                crate::assets::insert_deployment_assets(
                    tx,
                    input.id,
                    assets,
                    products.asset_object_refs,
                    input.now_ms,
                )?;
            }
            for (name, value) in &input.vars {
                tx.execute(
                    "INSERT INTO deployment_vars (deployment_id, name, value_json)
                     VALUES (?1, ?2, ?3)",
                    params![input.id.to_string(), name, value],
                )
                .map_err(|_| db_error())?;
            }
            for secret in input.secrets.values() {
                tx.execute(
                    "INSERT INTO deployment_secrets
                     (deployment_id, name, revision_id, key_id, algorithm, nonce, ciphertext)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        input.id.to_string(),
                        secret.name,
                        secret.revision_id,
                        secret.envelope.key_id,
                        secret.envelope.algorithm,
                        secret.envelope.nonce,
                        secret.envelope.ciphertext,
                    ],
                )
                .map_err(|_| db_error())?;
            }
            crate::bindings::insert_staging_bindings(
                tx,
                input.id,
                products.bindings,
                input.now_ms,
            )?;
            crate::queues::insert_staging_bindings(
                tx,
                input.id,
                products.queue_bindings,
                input.now_ms,
            )?;
            crate::queue_consumers::insert_staging_declarations(
                tx,
                input.id,
                products.queue_consumers,
                input.now_ms,
            )?;
            if let Some(cron) = products.cron {
                crate::cron::insert_staging_config(tx, input.id, cron, input.now_ms)?;
            }
            crate::workflows::bindings::insert_workflow_bindings(
                tx,
                input.id,
                products.workflow_bindings,
            )?;
            audit(
                tx,
                input.account_id,
                "deployment.create",
                "deployment",
                &input.id.to_string(),
                input.request_id,
                format!("{{\"state\":\"staging\",\"version\":{version}}}").as_bytes(),
                input.now_ms,
            )?;
            Ok(DeploymentRecord {
                id: input.id,
                worker_id: input.worker_id,
                version_number: u64::try_from(version).map_err(|_| invariant())?,
                content_kind: input.content_kind,
                state: DeploymentState::Staging,
                artifact_sha256: input.artifact_sha256,
                artifact_size: input.artifact_size,
                artifact_schema_version: input.artifact_schema_version,
                main_module: input.main_module.clone(),
                compatibility_date: input.compatibility_date.clone(),
                compatibility_flags: input.compatibility_flags.clone(),
                limits: input.limits.clone(),
                worker_code_sha256: input.worker_code_sha256,
                loader_schema_version: u32::try_from(LOADER_SCHEMA_VERSION)
                    .map_err(|_| invariant())?,
                created_at_ms: input.now_ms,
                ready_at_ms: None,
                rejected_at_ms: None,
                rejection_code: None,
                deleted_at_ms: None,
            })
        })
    }
}

fn validate_deployment_shape(
    input: &NewDeployment,
    products: &NewDeploymentProducts<'_>,
) -> Result<(), PlatformError> {
    let has_complete_bundle = input.artifact_sha256.is_some()
        && input.artifact_size.is_some()
        && input.artifact_schema_version.is_some()
        && input.main_module.is_some();
    let has_any_bundle = input.artifact_sha256.is_some()
        || input.artifact_size.is_some()
        || input.artifact_schema_version.is_some()
        || input.main_module.is_some();
    match input.content_kind {
        DeploymentContentKind::Worker if !has_complete_bundle => return Err(invariant()),
        DeploymentContentKind::AssetsOnly
            if has_any_bundle
                || products.assets.is_none()
                || !input.vars.is_empty()
                || !input.secrets.is_empty()
                || !products.bindings.is_empty()
                || !products.queue_bindings.is_empty()
                || !products.workflow_bindings.is_empty()
                || !products.queue_consumers.is_empty()
                || products.cron.is_some() =>
        {
            return Err(invariant());
        }
        DeploymentContentKind::Worker | DeploymentContentKind::AssetsOnly => {}
    }
    match products.assets {
        None if !products.asset_object_refs.is_empty() => Err(invariant()),
        None => Ok(()),
        Some(assets) => {
            let manifest_size =
                u64::try_from(assets.manifest_json.len()).map_err(|_| invariant())?;
            let manifest_sha256: [u8; 32] = Sha256::digest(&assets.manifest_json).into();
            if manifest_sha256 != assets.manifest_sha256
                || assets.logical_file_count == 0
                || products
                    .asset_object_refs
                    .iter()
                    .filter(|object| object.kind == crate::DeploymentObjectKind::AssetManifest)
                    .count()
                    != 1
                || !products.asset_object_refs.iter().any(|object| {
                    object.kind == crate::DeploymentObjectKind::AssetManifest
                        && object.sha256 == assets.manifest_sha256
                        && object.size == manifest_size
                })
            {
                return Err(invariant());
            }
            Ok(())
        }
    }
}
