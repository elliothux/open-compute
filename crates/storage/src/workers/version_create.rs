//! Atomic immutable version creation and product metadata insertion.

use super::*;
use sha2::{Digest, Sha256};

/// All product metadata committed with an immutable version in one control transaction.
#[derive(Clone, Debug, Default)]
pub struct NewVersionProducts<'a> {
    /// Optional immutable static-asset metadata.
    pub assets: Option<&'a crate::NewVersionAssets>,
    /// Static manifest/blob object references derived from the canonical manifest.
    pub asset_object_refs: &'a [crate::NewVersionObjectRef],
    /// Frozen KV/R2/D1/Durable Object resource bindings.
    pub bindings: &'a [crate::NewVersionBinding],
    /// Frozen Queue producer bindings.
    pub queue_bindings: &'a [crate::NewQueueProducerBinding],
    /// Frozen Workflow caller bindings.
    pub workflow_bindings: &'a [crate::WorkflowBindingRecord],
    /// Frozen cross-Worker Service declarations.
    pub services: &'a [crate::NewVersionService],
    /// Immutable automatic-cache policies.
    pub cache_policies: &'a [crate::VersionCachePolicyRecord],
    /// Platform-provided Images and Version Metadata bindings.
    pub builtin_bindings: &'a [crate::VersionBuiltinBindingRecord],
    /// Queue push-consumer declarations.
    pub queue_consumers: &'a [crate::NewQueueConsumerDeclaration],
    /// Optional immutable Cron declaration set.
    pub cron: Option<&'a crate::NewCronConfig>,
}

impl WorkerRepository<'_> {
    /// Insert version metadata, bindings, Queue consumers, and Cron declarations atomically.
    pub fn insert_staging_version(
        &self,
        input: &NewVersion,
        products: &NewVersionProducts<'_>,
        max_retained: u32,
    ) -> Result<VersionRecord, PlatformError> {
        if max_retained == 0 {
            return Err(PlatformError::new(
                ErrorCode::LimitInvalid,
                "version count limit must be greater than zero",
            ));
        }
        validate_version_shape(input, products)?;
        self.db.with_immediate(|tx| {
            require_live_worker(tx, input.account_id, input.worker_id)?;
            let retained_count: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM worker_versions
                     WHERE worker_id = ?1 AND deleted_at_ms IS NULL",
                    [input.worker_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(|_| db_error())?;
            if retained_count >= i64::from(max_retained) {
                return Err(PlatformError::new(
                    ErrorCode::QuotaExceeded,
                    "Worker version count quota was exceeded",
                ));
            }
            let version: i64 = tx
                .query_row(
                    "SELECT COALESCE(MAX(version_number), 0) + 1
                 FROM worker_versions WHERE worker_id = ?1",
                    [input.worker_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(|_| db_error())?;
            tx.execute(
                "INSERT INTO worker_versions
                 (id, worker_id, version_number, content_kind, state, artifact_sha256, artifact_size,
                  artifact_schema_version, main_module, worker_code_sha256,
                  loader_schema_version, created_at_ms, ready_at_ms, rejected_at_ms,
                  rejection_code, deleted_at_ms, compatibility_date, compatibility_flags_json)
                 VALUES (?1, ?2, ?3, ?4, 'staging', ?5, ?6, ?7, ?8,
                         ?9, ?10, ?11, NULL, NULL, NULL, NULL, ?12, ?13)",
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
                    input.worker_code_sha256.as_slice(),
                    LOADER_SCHEMA_VERSION,
                    input.now_ms,
                    input.compatibility_date,
                    serde_json::to_vec(&input.compatibility_flags).map_err(|_| invariant())?,
                ],
            )
            .map_err(|_| db_error())?;
            if let (Some(digest), Some(size)) = (input.artifact_sha256, input.artifact_size) {
                crate::assets::insert_bundle_object_ref(tx, input.id, &digest, size, input.now_ms)?;
            }
            if let Some(assets) = products.assets {
                crate::assets::insert_version_assets(
                    tx,
                    input.id,
                    assets,
                    products.asset_object_refs,
                    input.now_ms,
                )?;
            }
            for (name, value) in &input.vars {
                tx.execute(
                    "INSERT INTO version_vars (version_id, name, value_json)
                     VALUES (?1, ?2, ?3)",
                    params![input.id.to_string(), name, value],
                )
                .map_err(|_| db_error())?;
            }
            for secret in input.secrets.values() {
                tx.execute(
                    "INSERT INTO version_secrets
                     (version_id, name, revision_id, key_id, algorithm, nonce, ciphertext)
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
            crate::services::insert_staging_services(
                tx,
                input.id,
                products.services,
                input.now_ms,
            )?;
            crate::runtime_features::insert_runtime_features(
                tx,
                input.id,
                products.cache_policies,
                products.builtin_bindings,
            )?;
            audit(
                tx,
                input.account_id,
                "version.create",
                "version",
                &input.id.to_string(),
                input.request_id,
                format!("{{\"state\":\"staging\",\"version\":{version}}}").as_bytes(),
                input.now_ms,
            )?;
            Ok(VersionRecord {
                id: input.id,
                worker_id: input.worker_id,
                version_number: u64::try_from(version).map_err(|_| invariant())?,
                content_kind: input.content_kind,
                state: VersionState::Staging,
                artifact_sha256: input.artifact_sha256,
                artifact_size: input.artifact_size,
                artifact_schema_version: input.artifact_schema_version,
                main_module: input.main_module.clone(),
                worker_code_sha256: input.worker_code_sha256,
                loader_schema_version: u32::try_from(LOADER_SCHEMA_VERSION)
                    .map_err(|_| invariant())?,
                compatibility_date: input.compatibility_date.clone(),
                compatibility_flags: input.compatibility_flags.clone(),
                created_at_ms: input.now_ms,
                ready_at_ms: None,
                rejected_at_ms: None,
                rejection_code: None,
                deleted_at_ms: None,
            })
        })
    }
}

fn validate_version_shape(
    input: &NewVersion,
    products: &NewVersionProducts<'_>,
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
        VersionContentKind::Worker if !has_complete_bundle => return Err(invariant()),
        VersionContentKind::AssetsOnly
            if has_any_bundle
                || products.assets.is_none()
                || !input.vars.is_empty()
                || !input.secrets.is_empty()
                || !products.bindings.is_empty()
                || !products.queue_bindings.is_empty()
                || !products.workflow_bindings.is_empty()
                || !products.services.is_empty()
                || !products.cache_policies.is_empty()
                || !products.builtin_bindings.is_empty()
                || !products.queue_consumers.is_empty()
                || products.cron.is_some() =>
        {
            return Err(invariant());
        }
        VersionContentKind::Worker | VersionContentKind::AssetsOnly => {}
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
                    .filter(|object| object.kind == crate::VersionObjectKind::AssetManifest)
                    .count()
                    != 1
                || !products.asset_object_refs.iter().any(|object| {
                    object.kind == crate::VersionObjectKind::AssetManifest
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
