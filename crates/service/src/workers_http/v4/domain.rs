//! Worker v4 domain adapter over the immutable Version/Deployment authority.

use super::model::{WorkerUploadBinding, WorkerUploadExport, WorkerUploadMetadata};
use super::multipart::ParsedWorkerUpload;
use crate::cloudflare_v4::V4ResourceKind;
use crate::cloudflare_v4::accounts::AccountAuthority;
use crate::workers_http::WorkerApiState;
use open_compute_core::{
    AccountId, BindingKind, CanonicalBindingConfig, CanonicalPermissions, ErrorCode, PlatformError,
    RequestId, ResourceId, SecretString, WorkerId,
};
use open_compute_storage::{
    BuiltinBindingKind, CatalogDirection, CatalogSort, DeploymentSource, DurableObjectRepository,
    QueueRepository, ResourceRepository, VersionSnapshot, WorkerRecord, WorkerRepository,
    WorkflowRepository,
};
use open_compute_workers::{
    CreateVersionOutcome, CreateVersionRequest, ModuleBindingKind, RuntimeValidator,
    ServiceDescriptorV1, VersionBindingInput, VersionBundle, VersionCachePolicyInput,
    VersionContent, VersionController, VersionModuleBindingInput, VersionRuntimeFeatures,
    VersionServiceInput,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

pub(super) use super::authority::{ensure_worker, resolve_account, worker_by_name};
pub(super) use super::cloning::clone_active;
use super::errors::{invalid, invariant, unsupported};

pub(super) async fn create_from_upload(
    api: &WorkerApiState,
    account_authority: &AccountAuthority,
    account_id: AccountId,
    worker: &WorkerRecord,
    upload: ParsedWorkerUpload,
    strict_inheritance: bool,
    deployment_source: Option<DeploymentSource>,
    request_id: RequestId,
    now_ms: i64,
) -> Result<CreateVersionOutcome, PlatformError> {
    let migration = super::do_lifecycle::prepare(
        api,
        account_id,
        worker.id,
        &upload.metadata,
        upload.bundle.as_deref(),
        now_ms,
    )?;
    let result = create_from_prepared_upload(
        api,
        account_authority,
        account_id,
        worker,
        upload,
        strict_inheritance,
        deployment_source,
        request_id,
        now_ms,
        migration.as_ref(),
    )
    .await;
    match result {
        Ok(outcome) => Ok(outcome),
        Err(error) => {
            if let Some(migration) = &migration {
                migration.rollback(api, worker.id, now_ms)?;
            }
            Err(error)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn create_from_prepared_upload(
    api: &WorkerApiState,
    account_authority: &AccountAuthority,
    account_id: AccountId,
    worker: &WorkerRecord,
    upload: ParsedWorkerUpload,
    strict_inheritance: bool,
    deployment_source: Option<DeploymentSource>,
    request_id: RequestId,
    now_ms: i64,
    migration: Option<&super::do_lifecycle::PreparedDoMigration>,
) -> Result<CreateVersionOutcome, PlatformError> {
    let mut input = UploadInput::new(upload.metadata)?;
    let previous = worker
        .active_version_id
        .map(|version| {
            WorkerRepository::new(api.storage.db())
                .version_snapshot(account_id, worker.id, version, false)
        })
        .transpose()?;
    input.apply_inheritance(api, previous.as_ref(), strict_inheritance)?;
    input.apply_explicit_bindings(
        api,
        account_authority,
        account_id,
        worker.id,
        migration.map(super::do_lifecycle::PreparedDoMigration::tag),
        false,
        true,
        now_ms,
    )?;
    let reservation_id = request_id.to_string();
    let (content, asset_session) = input
        .content(
            api,
            account_id,
            &worker.name,
            upload.bundle,
            Some(&reservation_id),
            now_ms,
        )
        .await?;
    let idempotency_key = match &asset_session {
        Some(reservation) => format!(
            "v4-assets/{}",
            reservation.operation_id.as_deref().ok_or_else(invariant)?
        ),
        None => format!("v4/{request_id}"),
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
    if let Some(migration) = migration {
        controller = controller.with_durable_object_migration(migration.plan().clone());
    }
    let outcome = controller
        .create_version(CreateVersionRequest {
            account_id,
            worker_id: worker.id,
            idempotency_key,
            content,
            vars: input.vars,
            secrets: input.secrets,
            bindings: input.bindings,
            services: input.services,
            runtime_features: input.runtime_features,
            queue_consumers: Vec::new(),
            crons: input.crons,
            deployment_source,
            request_id,
            now_ms,
        })
        .await;
    match outcome {
        Ok(outcome) => {
            if let Some(session) = asset_session {
                super::assets::consume_assets(api, &session, now_ms)?;
            }
            Ok(outcome)
        }
        Err(error) => {
            if let Some(session) = asset_session
                && error.code() != ErrorCode::IdempotencyConflict
            {
                super::assets::release_assets(api, &session, now_ms)?;
            }
            Err(error)
        }
    }
}

pub(super) async fn validate_new_upload(
    api: &WorkerApiState,
    account_authority: &AccountAuthority,
    account_id: AccountId,
    script_name: &str,
    upload: &ParsedWorkerUpload,
    strict_inheritance: bool,
    now_ms: i64,
) -> Result<(), PlatformError> {
    let mut input = UploadInput::new(upload.metadata.clone())?;
    input.apply_inheritance(api, None, strict_inheritance)?;
    input.apply_explicit_bindings(
        api,
        account_authority,
        account_id,
        WorkerId::generate(),
        None,
        true,
        false,
        now_ms,
    )?;
    input
        .content(
            api,
            account_id,
            script_name,
            upload.bundle.clone(),
            None,
            now_ms,
        )
        .await?;
    Ok(())
}

pub(super) struct UploadInput {
    metadata: WorkerUploadMetadata,
    pub(super) vars: BTreeMap<String, serde_json::Value>,
    pub(super) secrets: BTreeMap<String, SecretString>,
    pub(super) bindings: BTreeMap<String, VersionBindingInput>,
    pub(super) services: BTreeMap<String, VersionServiceInput>,
    pub(super) runtime_features: VersionRuntimeFeatures,
    pub(super) crons: Vec<String>,
}

impl UploadInput {
    pub(super) fn new(metadata: WorkerUploadMetadata) -> Result<Self, PlatformError> {
        let mut runtime_features = VersionRuntimeFeatures {
            compatibility_date: metadata.compatibility_date.clone(),
            compatibility_flags: metadata.compatibility_flags.clone(),
            annotations: metadata.annotations.clone(),
            ..VersionRuntimeFeatures::default()
        };
        runtime_features
            .annotations
            .insert("workers/triggered_by".to_owned(), "upload".to_owned());
        if let Some(cache) = &metadata.cache_options {
            runtime_features.cache.default.enabled = cache.enabled;
            runtime_features.cache.default.cross_version_cache = cache.cross_version_cache;
        }
        if let Some(exports) = &metadata.exports {
            for (name, export) in exports {
                if let WorkerUploadExport::Worker { cache } = export {
                    let policy = VersionCachePolicyInput {
                        enabled: cache.as_ref().is_some_and(|value| value.enabled),
                        cross_version_cache: false,
                    };
                    if name == "default" {
                        runtime_features.cache.default = policy;
                    } else {
                        runtime_features
                            .cache
                            .entrypoints
                            .insert(name.clone(), policy);
                    }
                }
            }
        }
        Ok(Self {
            runtime_features,
            metadata,
            vars: BTreeMap::new(),
            secrets: BTreeMap::new(),
            bindings: BTreeMap::new(),
            services: BTreeMap::new(),
            crons: Vec::new(),
        })
    }

    pub(super) fn apply_inheritance(
        &mut self,
        api: &WorkerApiState,
        previous: Option<&VersionSnapshot>,
        strict: bool,
    ) -> Result<(), PlatformError> {
        let explicit_inheritance = self
            .metadata
            .bindings
            .iter()
            .any(|binding| matches!(binding, WorkerUploadBinding::Inherit { .. }));
        if (!self.metadata.keep_bindings.is_empty() || explicit_inheritance) && !strict {
            return Err(invalid(
                "binding inheritance requires bindings_inherit=strict",
            ));
        }
        let requested = self
            .metadata
            .keep_bindings
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        if requested.is_empty() && !explicit_inheritance {
            return Ok(());
        }
        let Some(previous) = previous else {
            if explicit_inheritance {
                return Err(invalid("binding inheritance has no prior Version"));
            }
            // Fixed Wrangler supplies keep_bindings for explicitly uploaded
            // secrets even on the first deploy. There is nothing to inherit.
            return Ok(());
        };
        let explicit_names = self
            .metadata
            .bindings
            .iter()
            .filter(|binding| !matches!(binding, WorkerUploadBinding::Inherit { .. }))
            .map(WorkerUploadBinding::name)
            .collect::<BTreeSet<_>>();
        let inherited_names = self
            .metadata
            .bindings
            .iter()
            .filter_map(|binding| match binding {
                WorkerUploadBinding::Inherit { name } => Some(name.as_str()),
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        let inherit_name = |name: &str, kind: &str| {
            !explicit_names.contains(name)
                && (requested.contains(kind) || inherited_names.contains(name))
        };
        for (name, bytes) in &previous.vars {
            let value: serde_json::Value =
                serde_json::from_slice(bytes).map_err(|_| invariant())?;
            let kind = if value.is_string() {
                "plain_text"
            } else {
                "json"
            };
            if inherit_name(name, kind) {
                self.vars.insert(name.clone(), value);
            }
        }
        for secret in previous.secrets.values() {
            if inherit_name(&secret.name, "secret_text") {
                let plaintext = api.storage.crypto().decrypt(
                    &secret.envelope,
                    previous.account_id,
                    previous.worker.id,
                    previous.version.id,
                    &secret.name,
                    &secret.revision_id,
                )?;
                let text = std::str::from_utf8(plaintext.expose()).map_err(|_| invariant())?;
                self.secrets
                    .insert(secret.name.clone(), SecretString::new(text));
            }
        }
        for binding in &previous.bindings {
            let kind = super::projection::wrangler_kind(binding.kind);
            if inherit_name(&binding.name, kind) {
                self.bindings.insert(
                    binding.name.clone(),
                    VersionBindingInput {
                        kind: binding.kind,
                        id: binding.resource_id,
                        permissions: binding.permissions,
                        config: binding.config.clone(),
                    },
                );
            }
        }
        for binding in &previous.queue_bindings {
            if inherit_name(&binding.name, "queue") {
                self.bindings.insert(
                    binding.name.clone(),
                    VersionBindingInput {
                        kind: BindingKind::QueueProducer,
                        id: ResourceId::from_uuid(binding.queue_id.as_uuid())
                            .map_err(|_| invariant())?,
                        permissions: CanonicalPermissions::default(),
                        config: CanonicalBindingConfig::default(),
                    },
                );
            }
        }
        for binding in &previous.workflow_bindings {
            if inherit_name(&binding.descriptor.name, "workflow") {
                self.bindings.insert(
                    binding.descriptor.name.clone(),
                    VersionBindingInput {
                        kind: BindingKind::Workflow,
                        id: ResourceId::from_uuid(binding.descriptor.definition_id.as_uuid())
                            .map_err(|_| invariant())?,
                        permissions: CanonicalPermissions::default(),
                        config: CanonicalBindingConfig {
                            workflow_class_name: Some(binding.descriptor.class_name.clone()),
                            workflow_schedules: binding.descriptor.schedules.clone(),
                        },
                    },
                );
            }
        }
        for service in &previous.services {
            if inherit_name(&service.binding_name, "service") {
                let props = service
                    .props_json
                    .as_deref()
                    .map(serde_json::from_slice)
                    .transpose()
                    .map_err(|_| invariant())?;
                let descriptor = ServiceDescriptorV1::new(
                    service.binding_name.clone(),
                    service.target_worker_id,
                    service.entrypoint.clone(),
                    props,
                )
                .map_err(|_| invariant())?;
                let canonical_props = descriptor
                    .props
                    .as_ref()
                    .map(serde_json::to_vec)
                    .transpose()
                    .map_err(|_| invariant())?;
                if canonical_props != service.props_json
                    || descriptor.sha256().map_err(|_| invariant())? != service.descriptor_sha256
                {
                    return Err(invariant());
                }
                self.services.insert(
                    service.binding_name.clone(),
                    VersionServiceInput {
                        target_worker_id: service.target_worker_id,
                        entrypoint: service.entrypoint.clone(),
                        props: descriptor.props,
                    },
                );
            }
        }
        for binding in &previous.builtin_bindings {
            let kind = match binding.kind {
                BuiltinBindingKind::Ai => "ai",
                BuiltinBindingKind::Images => "images",
                BuiltinBindingKind::VersionMetadata => "version_metadata",
                BuiltinBindingKind::WasmModule => "wasm_module",
                BuiltinBindingKind::TextBlob => "text_blob",
                BuiltinBindingKind::DataBlob => "data_blob",
            };
            if !inherit_name(&binding.name, kind) {
                continue;
            }
            match binding.kind {
                BuiltinBindingKind::Ai => {
                    self.runtime_features.ai = Some(open_compute_workers::VersionAiInput {
                        binding: binding.name.clone(),
                    });
                }
                BuiltinBindingKind::Images => {
                    self.runtime_features.images = Some(open_compute_workers::VersionImagesInput {
                        binding: binding.name.clone(),
                    });
                }
                BuiltinBindingKind::VersionMetadata => {
                    self.runtime_features.version_metadata =
                        Some(open_compute_workers::VersionVersionMetadataInput {
                            binding: binding.name.clone(),
                            tag: binding.tag.clone(),
                        });
                }
                BuiltinBindingKind::WasmModule
                | BuiltinBindingKind::TextBlob
                | BuiltinBindingKind::DataBlob => {
                    let module = binding.tag.clone().ok_or_else(invariant)?;
                    let kind = match binding.kind {
                        BuiltinBindingKind::WasmModule => ModuleBindingKind::WasmModule,
                        BuiltinBindingKind::TextBlob => ModuleBindingKind::TextBlob,
                        BuiltinBindingKind::DataBlob => ModuleBindingKind::DataBlob,
                        _ => return Err(invariant()),
                    };
                    self.runtime_features.module_bindings.insert(
                        binding.name.clone(),
                        VersionModuleBindingInput { module, kind },
                    );
                }
            }
        }
        for name in inherited_names {
            let found = self.vars.contains_key(name)
                || self.secrets.contains_key(name)
                || self.bindings.contains_key(name)
                || self.services.contains_key(name)
                || self.runtime_features.module_bindings.contains_key(name)
                || self
                    .runtime_features
                    .ai
                    .as_ref()
                    .is_some_and(|value| value.binding == name)
                || self
                    .runtime_features
                    .images
                    .as_ref()
                    .is_some_and(|value| value.binding == name)
                || self
                    .runtime_features
                    .version_metadata
                    .as_ref()
                    .is_some_and(|value| value.binding == name);
            if !found {
                return Err(invalid(
                    "inherited binding was not found in the prior Version",
                ));
            }
        }
        Ok(())
    }

    fn apply_explicit_bindings(
        &mut self,
        api: &WorkerApiState,
        account_authority: &AccountAuthority,
        account: AccountId,
        worker: WorkerId,
        migration_tag: Option<&str>,
        allow_declared_do: bool,
        reserve_workflows: bool,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        let bindings = self.metadata.bindings.clone();
        for binding in &bindings {
            let name = binding.name().to_owned();
            match binding {
                WorkerUploadBinding::PlainText { text, .. } => {
                    self.vars
                        .insert(name, serde_json::Value::String(text.clone()));
                }
                WorkerUploadBinding::Json { json, .. } => {
                    self.vars.insert(name, json.clone());
                }
                WorkerUploadBinding::SecretText { text, .. } => {
                    self.secrets.insert(name, text.clone());
                }
                WorkerUploadBinding::KvNamespace { namespace_id, .. } => self.resource(
                    api,
                    account_authority,
                    account,
                    name,
                    BindingKind::KvNamespace,
                    namespace_id.as_str(),
                )?,
                WorkerUploadBinding::R2Bucket { bucket_name, .. } => self.resource(
                    api,
                    account_authority,
                    account,
                    name,
                    BindingKind::R2Bucket,
                    bucket_name.as_str(),
                )?,
                WorkerUploadBinding::D1 { id, .. } => self.resource(
                    api,
                    account_authority,
                    account,
                    name,
                    BindingKind::D1Database,
                    id.as_str(),
                )?,
                WorkerUploadBinding::Vectorize { index_name, .. } => self.resource(
                    api,
                    account_authority,
                    account,
                    name,
                    BindingKind::VectorizeIndex,
                    index_name.as_str(),
                )?,
                WorkerUploadBinding::AiSearchNamespace { namespace, .. } => self.resource(
                    api,
                    account_authority,
                    account,
                    name,
                    BindingKind::AiSearchNamespace,
                    namespace.as_str(),
                )?,
                WorkerUploadBinding::AiSearch { instance_name, .. } => self.resource(
                    api,
                    account_authority,
                    account,
                    name,
                    BindingKind::AiSearchInstance,
                    instance_name.as_str(),
                )?,
                WorkerUploadBinding::Ai { .. } => {
                    self.runtime_features.ai =
                        Some(open_compute_workers::VersionAiInput { binding: name });
                }
                WorkerUploadBinding::Images { .. } => {
                    self.runtime_features.images =
                        Some(open_compute_workers::VersionImagesInput { binding: name });
                }
                WorkerUploadBinding::VersionMetadata { .. } => {
                    self.runtime_features.version_metadata =
                        Some(open_compute_workers::VersionVersionMetadataInput {
                            binding: name,
                            tag: self.metadata.annotations.get("workers/tag").cloned(),
                        });
                }
                WorkerUploadBinding::DurableObjectNamespace {
                    class_name,
                    script_name,
                    ..
                } => {
                    if script_name.is_some() {
                        return Err(unsupported(
                            "cross-Script Durable Object bindings are unsupported",
                        ));
                    }
                    let namespace = match DurableObjectRepository::new(&api.storage)
                        .namespace_for_worker_upload(account, worker, class_name, migration_tag)
                    {
                        Ok(value) => value,
                        Err(error)
                            if allow_declared_do
                                && super::do_lifecycle::declares_live_class(
                                    &self.metadata,
                                    class_name,
                                ) =>
                        {
                            let _ = error;
                            continue;
                        }
                        Err(_) => {
                            return Err(invalid("Durable Object namespace was not found"));
                        }
                    };
                    self.bindings.insert(
                        name,
                        VersionBindingInput {
                            kind: BindingKind::DoNamespace,
                            id: namespace.resource.id,
                            permissions: CanonicalPermissions::default(),
                            config: CanonicalBindingConfig::default(),
                        },
                    );
                }
                WorkerUploadBinding::Queue { queue_name, .. } => {
                    let queue = QueueRepository::new(api.storage.db())
                        .list(
                            account,
                            Some(queue_name.as_str()),
                            None,
                            CatalogSort::Name,
                            CatalogDirection::Asc,
                            None,
                            100,
                        )?
                        .items
                        .into_iter()
                        .find(|value| value.name == *queue_name)
                        .ok_or_else(|| invalid("Queue was not found"))?;
                    self.bindings.insert(
                        name,
                        VersionBindingInput {
                            kind: BindingKind::QueueProducer,
                            id: ResourceId::from_uuid(queue.id.as_uuid())
                                .map_err(|_| invariant())?,
                            permissions: CanonicalPermissions::default(),
                            config: CanonicalBindingConfig::default(),
                        },
                    );
                }
                WorkerUploadBinding::Workflow {
                    workflow_name,
                    class_name,
                    script_name,
                    ..
                } => {
                    if script_name.is_some() {
                        return Err(unsupported(
                            "cross-Script Workflow bindings are unsupported",
                        ));
                    }
                    let class_name = class_name
                        .as_deref()
                        .ok_or_else(|| invalid("Workflow class name is required"))?;
                    let repository = WorkflowRepository::new(api.storage.db());
                    let definition = repository
                        .definitions(
                            account,
                            Some(workflow_name.as_str()),
                            None,
                            CatalogSort::Name,
                            CatalogDirection::Asc,
                            None,
                            100,
                        )?
                        .items
                        .into_iter()
                        .find(|value| value.name == *workflow_name);
                    let definition = match definition {
                        Some(definition) => definition,
                        None if reserve_workflows => repository.reserve_definition(
                            account,
                            workflow_name,
                            class_name,
                            now_ms,
                        )?,
                        None => continue,
                    };
                    self.bindings.insert(
                        name,
                        VersionBindingInput {
                            kind: BindingKind::Workflow,
                            id: ResourceId::from_uuid(definition.id.as_uuid())
                                .map_err(|_| invariant())?,
                            permissions: CanonicalPermissions::default(),
                            config: CanonicalBindingConfig {
                                workflow_class_name: Some(class_name.to_owned()),
                                workflow_schedules: Vec::new(),
                            },
                        },
                    );
                }
                WorkerUploadBinding::Service {
                    service,
                    entrypoint,
                    props,
                    ..
                } => {
                    let target = worker_by_name(api, account, service.as_str())?;
                    self.services.insert(
                        name,
                        VersionServiceInput {
                            target_worker_id: target.id,
                            entrypoint: entrypoint.clone(),
                            props: props.clone(),
                        },
                    );
                }
                WorkerUploadBinding::Assets { .. } => {}
                WorkerUploadBinding::WasmModule { part, .. }
                | WorkerUploadBinding::TextBlob { part, .. }
                | WorkerUploadBinding::DataBlob { part, .. } => {
                    let kind = match binding {
                        WorkerUploadBinding::WasmModule { .. } => ModuleBindingKind::WasmModule,
                        WorkerUploadBinding::TextBlob { .. } => ModuleBindingKind::TextBlob,
                        WorkerUploadBinding::DataBlob { .. } => ModuleBindingKind::DataBlob,
                        _ => return Err(invariant()),
                    };
                    self.runtime_features.module_bindings.insert(
                        name,
                        VersionModuleBindingInput {
                            module: part.clone(),
                            kind,
                        },
                    );
                }
                WorkerUploadBinding::Inherit { .. } => {}
            }
        }
        Ok(())
    }

    fn resource(
        &mut self,
        api: &WorkerApiState,
        account_authority: &AccountAuthority,
        account: AccountId,
        name: String,
        kind: BindingKind,
        external: &str,
    ) -> Result<(), PlatformError> {
        let resource = ResourceRepository::new(api.storage.db())
            .list(account, Some(kind))?
            .into_iter()
            .find(|resource| match kind {
                BindingKind::KvNamespace => account_authority.matches_public_resource_id(
                    V4ResourceKind::KvNamespace,
                    resource.id,
                    external,
                ),
                BindingKind::D1Database => account_authority.matches_public_resource_id(
                    V4ResourceKind::D1Database,
                    resource.id,
                    external,
                ),
                _ => resource.name == external,
            })
            .ok_or_else(|| invalid("binding resource was not found"))?;
        self.bindings.insert(
            name,
            VersionBindingInput {
                kind,
                id: resource.id,
                permissions: CanonicalPermissions::default(),
                config: CanonicalBindingConfig::default(),
            },
        );
        Ok(())
    }

    async fn content(
        &self,
        api: &WorkerApiState,
        account_id: AccountId,
        script_name: &str,
        bundle: Option<Vec<u8>>,
        reservation_id: Option<&str>,
        now_ms: i64,
    ) -> Result<(VersionContent, Option<super::assets::AssetReservation>), PlatformError> {
        let asset_binding = self
            .metadata
            .bindings
            .iter()
            .filter_map(|binding| match binding {
                WorkerUploadBinding::Assets { name } => Some(name.clone()),
                _ => None,
            })
            .next();
        let redeemed = self
            .metadata
            .assets
            .as_ref()
            .map(|assets| {
                super::assets::redeem_assets(
                    api,
                    &assets.jwt,
                    account_id,
                    script_name,
                    reservation_id,
                    asset_binding,
                    &assets.config,
                    now_ms,
                )
            })
            .transpose()?;
        let (assets, session) = match redeemed {
            Some((assets, session)) => (Some(assets), Some(session)),
            None => (None, None),
        };
        match bundle {
            Some(bundle) => Ok((
                VersionContent::Worker {
                    bundle: VersionBundle::Bytes(bundle),
                    assets,
                },
                session,
            )),
            None => Ok((
                VersionContent::AssetsOnly {
                    assets: assets.ok_or_else(|| invalid("Worker bundle is missing"))?,
                },
                session,
            )),
        }
    }
}

#[cfg(test)]
#[path = "domain_tests.rs"]
mod tests;
