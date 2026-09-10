use super::*;
use open_compute_core::{AccountId, VersionId, WorkerId};
use open_compute_storage::VersionSnapshot;

pub(super) type ResolvedAssets = Option<(AssetManifestV1, AssetRoutingConfigV1)>;

#[derive(Clone, Copy)]
pub(super) struct ResolutionIdentity {
    pub(super) account_id: AccountId,
    pub(super) worker_id: WorkerId,
    pub(super) version_id: VersionId,
    pub(super) created_at_ms: i64,
}

pub(super) struct ResolvedEnvironment {
    pub(super) vars: BTreeMap<String, serde_json::Value>,
    pub(super) secret_descriptors: Vec<SecretDescriptor>,
}

pub(super) struct ResolvedResourceBindings {
    pub(super) descriptors: Vec<BindingDescriptorV1>,
    pub(super) runtime: Vec<RuntimeBinding>,
}

pub(super) struct ResolvedQueueBindings {
    pub(super) descriptors: Vec<QueueProducerBindingDescriptorV1>,
    pub(super) runtime: Vec<RuntimeQueueBinding>,
}

pub(super) struct ResolvedWorkflowBindings {
    pub(super) descriptors: Vec<open_compute_storage::WorkflowBindingDescriptor>,
    pub(super) runtime: Vec<RuntimeWorkflowBinding>,
}

pub(super) struct ResolvedServices {
    pub(super) descriptors: Vec<ServiceDescriptorV1>,
    pub(super) runtime: Vec<RuntimeServiceBinding>,
}

pub(super) struct ResolvedBuiltins {
    pub(super) descriptors: Vec<BuiltinBindingDescriptorV1>,
    pub(super) worker_loaders: Vec<RuntimeWorkerLoaderBinding>,
    pub(super) ai_binding: Option<RuntimeAiBinding>,
    pub(super) images_binding: Option<RuntimeImagesBinding>,
    pub(super) version_metadata_binding: Option<RuntimeVersionMetadataBinding>,
    pub(super) module_bindings: Vec<RuntimeModuleBinding>,
}

pub(super) fn validate_expected_digest(value: &str) -> Result<(), PlatformError> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        Ok(())
    } else {
        Err(invariant())
    }
}

pub(super) fn validate_scope(
    snapshot: &VersionSnapshot,
    scope: RuntimeScope,
) -> Result<(), PlatformError> {
    match scope {
        RuntimeScope::Runtime if snapshot.version.state != VersionState::Ready => Err(not_ready()),
        RuntimeScope::Validation if snapshot.version.state != VersionState::Validating => {
            Err(not_ready())
        }
        RuntimeScope::Probe
            if !matches!(
                snapshot.version.state,
                VersionState::Validating | VersionState::Ready
            ) =>
        {
            Err(not_ready())
        }
        RuntimeScope::Runtime | RuntimeScope::Validation | RuntimeScope::Probe => Ok(()),
    }
}

pub(super) fn resolve_assets(snapshot: &VersionSnapshot) -> Result<ResolvedAssets, PlatformError> {
    snapshot
        .assets
        .as_ref()
        .map(|stored| {
            let manifest = serde_json::from_slice::<AssetManifestV1>(&stored.manifest_json)
                .map_err(|_| invariant())?;
            let routing =
                serde_json::from_slice::<AssetRoutingConfigV1>(&stored.routing_config_json)
                    .map_err(|_| invariant())?;
            if manifest.sha256()? != stored.manifest_sha256
                || manifest.canonical_bytes()? != stored.manifest_json
                || routing.canonical_bytes()? != stored.routing_config_json
                || routing.binding != stored.binding_name
            {
                return Err(invariant());
            }
            Ok((manifest, routing))
        })
        .transpose()
}

pub(super) async fn resolve_bundle(
    source: &RuntimeSource,
    snapshot: &VersionSnapshot,
    assets: &ResolvedAssets,
    scope: RuntimeScope,
) -> Result<Option<CanonicalBundle>, PlatformError> {
    match snapshot.version.content_kind {
        VersionContentKind::Worker => {
            let artifact_sha256 = snapshot.version.artifact_sha256.ok_or_else(invariant)?;
            let artifact_size = snapshot.version.artifact_size.ok_or_else(invariant)?;
            let main_module = snapshot
                .version
                .main_module
                .as_deref()
                .ok_or_else(invariant)?;
            let artifact = ArtifactRef::new(
                ARTIFACT_KEY_VERSION,
                &hex::encode(artifact_sha256),
                artifact_size,
            )?;
            let bytes = match &source.cache {
                Some(cache) => {
                    let mut pinned = cache
                        .acquire(&source.artifacts, &artifact)
                        .await
                        .map_err(map_artifact_error)?;
                    pinned.read_all().map_err(map_artifact_error)?
                }
                None => source
                    .artifacts
                    .open(&artifact)
                    .await
                    .map_err(map_artifact_error)?
                    .to_vec(),
            };
            let bundle = CanonicalBundle::parse(bytes, source.limits)?;
            if bundle.sha256() != artifact_sha256 || bundle.manifest().main_module != main_module {
                return Err(invariant());
            }
            Ok(Some(bundle))
        }
        VersionContentKind::AssetsOnly if scope == RuntimeScope::Runtime => {
            if assets.is_none() {
                return Err(invariant());
            }
            Ok(None)
        }
        VersionContentKind::AssetsOnly => Err(not_ready()),
    }
}

pub(super) fn resolve_environment(
    snapshot: &VersionSnapshot,
) -> Result<ResolvedEnvironment, PlatformError> {
    if snapshot.vars.len().saturating_add(snapshot.secrets.len()) > MAX_VARIABLES {
        return Err(invariant());
    }
    let mut vars = BTreeMap::new();
    for (name, raw) in &snapshot.vars {
        let value = serde_json::from_slice(raw).map_err(|_| invariant())?;
        vars.insert(name.clone(), value);
    }
    let (vars, encoded_vars) = canonicalize_vars(vars).map_err(|_| invariant())?;
    if encoded_vars != snapshot.vars {
        return Err(invariant());
    }
    let secret_descriptors = snapshot
        .secrets
        .values()
        .map(|secret| SecretDescriptor {
            name: secret.name.clone(),
            revision_id: secret.revision_id.clone(),
            ciphertext_sha256: ciphertext_sha256(
                &secret.envelope.nonce,
                &secret.envelope.ciphertext,
            ),
        })
        .collect();
    Ok(ResolvedEnvironment {
        vars,
        secret_descriptors,
    })
}

pub(super) fn resolve_resource_bindings(
    source: &RuntimeSource,
    snapshot: &VersionSnapshot,
    scope: RuntimeScope,
) -> Result<ResolvedResourceBindings, PlatformError> {
    let mut descriptors = Vec::with_capacity(snapshot.bindings.len());
    let mut runtime = Vec::with_capacity(snapshot.bindings.len());
    for binding in &snapshot.bindings {
        let descriptor = BindingDescriptorV1::new(
            binding.id,
            binding.name.clone(),
            binding.kind,
            binding.resource_id,
            binding.resource_spec_generation,
            binding.capability_version,
            binding.permissions,
            binding.config.clone(),
        )?;
        let digest = descriptor.sha256()?;
        if digest != binding.descriptor_sha256 {
            return Err(invariant());
        }
        descriptors.push(descriptor.clone());
        runtime.push(RuntimeBinding {
            descriptor,
            descriptor_sha256: hex::encode(digest),
            durable_object_identity: resolve_durable_object_identity(source, binding, scope)?,
        });
    }
    for binding in open_compute_storage::CloudflareArtifactsRepository::new(source.storage.db())
        .version_bindings(snapshot.version.id)?
    {
        let descriptor = BindingDescriptorV1::new(
            binding.id,
            binding.name,
            BindingKind::ArtifactsNamespace,
            binding.namespace_id,
            binding.namespace_generation,
            binding.capability_version,
            binding.permissions,
            CanonicalBindingConfig::default(),
        )?;
        let digest = descriptor.sha256()?;
        if digest != binding.descriptor_sha256 {
            return Err(invariant());
        }
        descriptors.push(descriptor.clone());
        runtime.push(RuntimeBinding {
            descriptor,
            descriptor_sha256: hex::encode(digest),
            durable_object_identity: None,
        });
    }
    Ok(ResolvedResourceBindings {
        descriptors,
        runtime,
    })
}

fn resolve_durable_object_identity(
    source: &RuntimeSource,
    binding: &open_compute_storage::VersionBindingRecord,
    scope: RuntimeScope,
) -> Result<Option<DurableObjectFacadeIdentity>, PlatformError> {
    if binding.kind != BindingKind::DoNamespace || scope != RuntimeScope::Runtime {
        return Ok(None);
    }
    let (prefix, key) =
        DurableObjectRepository::new(&source.storage).facade_identity(binding.resource_id)?;
    Ok(Some(DurableObjectFacadeIdentity {
        namespace_prefix: hex::encode(prefix),
        namespace_name_key: SecretString::new(
            base64::engine::general_purpose::STANDARD.encode(key),
        ),
    }))
}

pub(super) fn resolve_queue_bindings(
    snapshot: &VersionSnapshot,
) -> Result<ResolvedQueueBindings, PlatformError> {
    let mut descriptors = Vec::with_capacity(snapshot.queue_bindings.len());
    let mut runtime = Vec::with_capacity(snapshot.queue_bindings.len());
    for binding in &snapshot.queue_bindings {
        let descriptor = QueueProducerBindingDescriptorV1::new(
            binding.id,
            binding.name.clone(),
            binding.queue_id,
            binding.queue_lifecycle_generation,
            binding.capability_version,
        )?;
        let digest = descriptor.sha256()?;
        if digest != binding.descriptor_sha256 {
            return Err(invariant());
        }
        descriptors.push(descriptor.clone());
        runtime.push(RuntimeQueueBinding {
            descriptor,
            descriptor_sha256: hex::encode(digest),
        });
    }
    Ok(ResolvedQueueBindings {
        descriptors,
        runtime,
    })
}

pub(super) fn resolve_workflow_bindings(
    snapshot: &VersionSnapshot,
) -> Result<ResolvedWorkflowBindings, PlatformError> {
    let mut descriptors = Vec::with_capacity(snapshot.workflow_bindings.len());
    let mut runtime = Vec::with_capacity(snapshot.workflow_bindings.len());
    for binding in &snapshot.workflow_bindings {
        let digest = binding.descriptor.sha256()?;
        if digest != binding.descriptor_sha256 {
            return Err(invariant());
        }
        descriptors.push(binding.descriptor.clone());
        runtime.push(RuntimeWorkflowBinding {
            descriptor: binding.descriptor.clone(),
            descriptor_sha256: hex::encode(digest),
        });
    }
    Ok(ResolvedWorkflowBindings {
        descriptors,
        runtime,
    })
}

pub(super) fn resolve_scheduled_targets(
    source: &RuntimeSource,
    snapshot: &VersionSnapshot,
    version_id: VersionId,
    bindings: &[RuntimeWorkflowBinding],
) -> Result<Vec<RuntimeScheduledTarget>, PlatformError> {
    if snapshot.version.content_kind != VersionContentKind::Worker {
        return Ok(Vec::new());
    }
    let cron = open_compute_storage::CronRepository::new(source.storage.db())
        .version_config(version_id)?;
    for declaration in &cron.declarations {
        for name in &declaration.workflow_bindings {
            let Some(binding) = bindings
                .iter()
                .find(|binding| binding.descriptor.name == *name)
            else {
                return Err(invariant());
            };
            if binding
                .descriptor
                .schedules
                .binary_search(&declaration.expression)
                .is_err()
            {
                return Err(invariant());
            }
        }
    }
    for binding in bindings {
        if binding.descriptor.schedules.iter().any(|expression| {
            !cron.declarations.iter().any(|declaration| {
                declaration.expression == *expression
                    && declaration
                        .workflow_bindings
                        .binary_search(&binding.descriptor.name)
                        .is_ok()
            })
        }) {
            return Err(invariant());
        }
    }
    Ok(cron
        .declarations
        .into_iter()
        .map(|declaration| RuntimeScheduledTarget {
            cron: declaration.expression,
            scheduled_handler: declaration.scheduled_handler,
            workflow_bindings: declaration.workflow_bindings,
        })
        .collect())
}

pub(super) fn resolve_services(
    snapshot: &VersionSnapshot,
) -> Result<ResolvedServices, PlatformError> {
    let mut descriptors = Vec::with_capacity(snapshot.services.len());
    let mut runtime = Vec::with_capacity(snapshot.services.len());
    for service in &snapshot.services {
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
        if canonical_props != service.props_json {
            return Err(invariant());
        }
        let digest = descriptor.sha256().map_err(|_| invariant())?;
        if digest != service.descriptor_sha256 {
            return Err(invariant());
        }
        descriptors.push(descriptor.clone());
        runtime.push(RuntimeServiceBinding {
            descriptor,
            descriptor_sha256: hex::encode(digest),
        });
    }
    Ok(ResolvedServices {
        descriptors,
        runtime,
    })
}

pub(super) fn resolve_cache_policy(
    snapshot: &VersionSnapshot,
) -> Result<CachePolicyDescriptorV1, PlatformError> {
    let mut descriptor = CachePolicyDescriptorV1::default();
    for policy in &snapshot.cache_policies {
        match &policy.entrypoint {
            None => {
                descriptor.enabled = policy.enabled;
                descriptor.cross_version_cache = policy.cross_version_cache;
            }
            Some(name) => {
                descriptor.entrypoints.insert(
                    name.clone(),
                    CacheEntrypointPolicyV1 {
                        enabled: policy.enabled,
                        cross_version_cache: policy.cross_version_cache,
                    },
                );
            }
        }
    }
    descriptor.validate()?;
    Ok(descriptor)
}

pub(super) fn resolve_builtin_bindings(
    snapshot: &VersionSnapshot,
    bundle: Option<&CanonicalBundle>,
    identity: ResolutionIdentity,
) -> Result<ResolvedBuiltins, PlatformError> {
    let mut resolved = ResolvedBuiltins {
        descriptors: Vec::with_capacity(snapshot.builtin_bindings.len()),
        worker_loaders: Vec::new(),
        ai_binding: None,
        images_binding: None,
        version_metadata_binding: None,
        module_bindings: Vec::new(),
    };
    for binding in &snapshot.builtin_bindings {
        let kind = builtin_descriptor_kind(binding.kind);
        let descriptor =
            BuiltinBindingDescriptorV1::new(binding.name.clone(), kind, binding.tag.clone())?;
        let digest = descriptor.sha256()?;
        if digest != binding.descriptor_sha256 {
            return Err(invariant());
        }
        resolve_builtin_runtime(&mut resolved, binding, digest, bundle, identity)?;
        resolved.descriptors.push(descriptor);
    }
    Ok(resolved)
}

fn builtin_descriptor_kind(kind: BuiltinBindingKind) -> BuiltinBindingDescriptorKindV1 {
    match kind {
        BuiltinBindingKind::WorkerLoader => BuiltinBindingDescriptorKindV1::WorkerLoader,
        BuiltinBindingKind::Ai => BuiltinBindingDescriptorKindV1::Ai,
        BuiltinBindingKind::Images => BuiltinBindingDescriptorKindV1::Images,
        BuiltinBindingKind::VersionMetadata => BuiltinBindingDescriptorKindV1::VersionMetadata,
        BuiltinBindingKind::WasmModule => BuiltinBindingDescriptorKindV1::WasmModule,
        BuiltinBindingKind::TextBlob => BuiltinBindingDescriptorKindV1::TextBlob,
        BuiltinBindingKind::DataBlob => BuiltinBindingDescriptorKindV1::DataBlob,
    }
}

fn resolve_builtin_runtime(
    resolved: &mut ResolvedBuiltins,
    binding: &open_compute_storage::VersionBuiltinBindingRecord,
    digest: [u8; 32],
    bundle: Option<&CanonicalBundle>,
    identity: ResolutionIdentity,
) -> Result<(), PlatformError> {
    let descriptor_sha256 = hex::encode(digest);
    match binding.kind {
        BuiltinBindingKind::WorkerLoader => {
            resolved.worker_loaders.push(RuntimeWorkerLoaderBinding {
                name: binding.name.clone(),
                namespace_key: worker_loader_namespace_key(
                    identity.account_id,
                    identity.worker_id,
                    &binding.name,
                ),
            });
        }
        BuiltinBindingKind::Ai => {
            resolved.ai_binding = Some(RuntimeAiBinding {
                name: binding.name.clone(),
                descriptor_sha256,
            });
        }
        BuiltinBindingKind::Images => {
            resolved.images_binding = Some(RuntimeImagesBinding {
                name: binding.name.clone(),
                descriptor_sha256,
            });
        }
        BuiltinBindingKind::VersionMetadata => {
            resolved.version_metadata_binding = Some(RuntimeVersionMetadataBinding {
                name: binding.name.clone(),
                id: identity.version_id.to_string(),
                tag: binding.tag.clone(),
                timestamp_ms: identity.created_at_ms,
                descriptor_sha256,
            });
        }
        BuiltinBindingKind::WasmModule
        | BuiltinBindingKind::TextBlob
        | BuiltinBindingKind::DataBlob => {
            resolved
                .module_bindings
                .push(resolve_module_binding(binding, bundle)?);
        }
    }
    Ok(())
}

fn resolve_module_binding(
    binding: &open_compute_storage::VersionBuiltinBindingRecord,
    bundle: Option<&CanonicalBundle>,
) -> Result<RuntimeModuleBinding, PlatformError> {
    let module_type = match binding.kind {
        BuiltinBindingKind::WasmModule => ModuleType::Wasm,
        BuiltinBindingKind::TextBlob => ModuleType::Text,
        BuiltinBindingKind::DataBlob => ModuleType::Data,
        _ => return Err(invariant()),
    };
    let module_name = binding.tag.as_deref().ok_or_else(invariant)?;
    let bundle = bundle.ok_or_else(invariant)?;
    let module = bundle
        .manifest()
        .modules
        .iter()
        .find(|module| module.name == module_name && module.module_type == module_type)
        .ok_or_else(invariant)?;
    Ok(RuntimeModuleBinding {
        name: binding.name.clone(),
        module_type,
        bytes: bundle.module_bytes(module)?.to_vec(),
    })
}

pub(super) fn runtime_modules(
    bundle: Option<&CanonicalBundle>,
) -> Result<Vec<RuntimeModule>, PlatformError> {
    let Some(bundle) = bundle else {
        return Ok(Vec::new());
    };
    bundle
        .manifest()
        .modules
        .iter()
        .filter(|module| module.module_type != ModuleType::SourceMap)
        .map(|module| {
            Ok(RuntimeModule {
                name: module.name.clone(),
                module_type: module.module_type,
                bytes: bundle.module_bytes(module)?.to_vec(),
            })
        })
        .collect()
}

pub(super) fn decrypt_secrets(
    source: &RuntimeSource,
    snapshot: &VersionSnapshot,
    scope: RuntimeScope,
    identity: ResolutionIdentity,
    vars: &BTreeMap<String, serde_json::Value>,
) -> Result<BTreeMap<String, SecretString>, PlatformError> {
    let mut secrets = BTreeMap::new();
    if scope != RuntimeScope::Runtime {
        return Ok(secrets);
    }
    for secret in snapshot.secrets.values() {
        let plaintext = source.storage.crypto().decrypt(
            &secret.envelope,
            identity.account_id,
            identity.worker_id,
            identity.version_id,
            &secret.name,
            &secret.revision_id,
        )?;
        let text = std::str::from_utf8(plaintext.expose()).map_err(|_| {
            PlatformError::new(ErrorCode::SecretInvalid, "secret is not valid UTF-8")
        })?;
        secrets.insert(secret.name.clone(), SecretString::new(text));
    }
    crate::pipeline::validate_secret_set(&secrets, vars).map_err(|_| invariant())?;
    Ok(secrets)
}
