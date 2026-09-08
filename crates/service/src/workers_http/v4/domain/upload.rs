use super::*;

pub(in crate::workers_http::v4) struct UploadInput {
    metadata: WorkerUploadMetadata,
    pub(in crate::workers_http::v4) vars: BTreeMap<String, serde_json::Value>,
    pub(in crate::workers_http::v4) secrets: BTreeMap<String, SecretString>,
    pub(in crate::workers_http::v4) bindings: BTreeMap<String, VersionBindingInput>,
    pub(in crate::workers_http::v4) services: BTreeMap<String, VersionServiceInput>,
    pub(in crate::workers_http::v4) runtime_features: VersionRuntimeFeatures,
    pub(in crate::workers_http::v4) crons: Vec<String>,
    pub(in crate::workers_http::v4) workflow_reservations: Vec<WorkflowDefinitionReservation>,
}

impl UploadInput {
    pub(in crate::workers_http::v4) fn new(metadata: WorkerUploadMetadata) -> Self {
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
        Self {
            runtime_features,
            metadata,
            vars: BTreeMap::new(),
            secrets: BTreeMap::new(),
            bindings: BTreeMap::new(),
            services: BTreeMap::new(),
            crons: Vec::new(),
            workflow_reservations: Vec::new(),
        }
    }

    pub(in crate::workers_http::v4) fn apply_inheritance(
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
            let kind = super::super::projection::wrangler_kind(binding.kind);
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
                            workflow_reservation_fence: None,
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
                BuiltinBindingKind::WorkerLoader => "worker_loader",
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
                BuiltinBindingKind::WorkerLoader => {
                    self.runtime_features
                        .worker_loaders
                        .push(binding.name.clone());
                }
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

    #[expect(
        clippy::too_many_arguments,
        reason = "binding validation keeps each authority and reservation control explicit"
    )]
    pub(in crate::workers_http::v4) fn apply_explicit_bindings(
        &mut self,
        api: &WorkerApiState,
        account_authority: &AccountAuthority,
        account: AccountId,
        worker: WorkerId,
        migration_tag: Option<&str>,
        allow_declared_do: bool,
        reserve_workflows: bool,
        reservation_owner: Option<&str>,
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
                WorkerUploadBinding::WorkerLoader { .. } => {
                    self.runtime_features.worker_loaders.push(name);
                }
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
                                && super::super::do_lifecycle::declares_live_class(
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
                    let (definition, reservation_fence) = if reserve_workflows {
                        let reservation = repository.reserve_definition(
                            account,
                            workflow_name,
                            class_name,
                            reservation_owner.ok_or_else(invariant)?,
                            now_ms,
                        )?;
                        let definition = reservation.definition.clone();
                        let fence = reservation.fence;
                        self.workflow_reservations.push(reservation);
                        (definition, Some(fence))
                    } else {
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
                        let Some(definition) = definition else {
                            continue;
                        };
                        (definition, None)
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
                                workflow_reservation_fence: reservation_fence,
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

    pub(in crate::workers_http::v4) fn release_workflow_reservations(
        &self,
        api: &WorkerApiState,
        account: AccountId,
        now_ms: i64,
    ) -> Result<(), PlatformError> {
        release_workflow_reservations(api, account, &self.workflow_reservations, now_ms)
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

    pub(in crate::workers_http::v4) async fn content(
        &self,
        api: &WorkerApiState,
        account_id: AccountId,
        script_name: &str,
        bundle: Option<Vec<u8>>,
        reservation_id: Option<&str>,
        now_ms: i64,
    ) -> Result<
        (
            VersionContent,
            Option<super::super::assets::AssetReservation>,
        ),
        PlatformError,
    > {
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
                super::super::assets::redeem_assets(
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
