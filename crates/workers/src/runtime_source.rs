//! Scoped immutable `RuntimeSource` assembly from `SQLite` and verified artifacts.

use crate::assets::{AssetManifestV1, AssetRoutingConfigV1};
use crate::bundle::{BundleLimits, CanonicalBundle, ModuleType};
use crate::descriptor::{
    BindingDescriptorV1, BuiltinBindingDescriptorKindV1, BuiltinBindingDescriptorV1,
    CacheEntrypointPolicyV1, CachePolicyDescriptorV1, QueueProducerBindingDescriptorV1,
    SecretDescriptor, ServiceDescriptorV1, WorkerCodeDescriptorV1, ciphertext_sha256,
    parse_loader_key,
};
use crate::environment::{MAX_VARIABLES, canonicalize_vars};
use base64::Engine as _;
use open_compute_artifacts::{ARTIFACT_KEY_VERSION, ArtifactCache, ArtifactRef, ArtifactStore};
use open_compute_core::{BindingKind, ErrorCode, PlatformError, SecretString};
use open_compute_storage::{
    BuiltinBindingKind, DurableObjectRepository, PlatformStorage, VersionContentKind, VersionState,
    WorkerRepository,
};
use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::Arc;
use zeroize::Zeroize;

/// `RuntimeSource` authorization scope.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeScope {
    /// Only immutable ready versions.
    Runtime,
    /// Only a currently validating version; secrets are omitted.
    Validation,
    /// A validating or ready version used to prove a named export; secrets are omitted.
    Probe,
}

/// One verified module returned to the loader host.
#[derive(Clone, Eq, PartialEq)]
pub struct RuntimeModule {
    /// Canonical logical name.
    pub name: String,
    /// Module type.
    pub module_type: ModuleType,
    /// Raw verified bytes.
    pub bytes: Vec<u8>,
}

/// One verified service-worker global backed by an immutable module part.
#[derive(Clone, Eq, PartialEq)]
pub struct RuntimeModuleBinding {
    /// Tenant global binding name.
    pub name: String,
    /// Exact module representation (`Wasm`, `Text`, or `Data`).
    pub module_type: ModuleType,
    /// Raw verified module bytes.
    pub bytes: Vec<u8>,
}

/// One verified binding descriptor and its persisted canonical digest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeBinding {
    /// Canonical descriptor supplied only to the loader-side binding factory.
    pub descriptor: BindingDescriptorV1,
    /// Lowercase SHA-256 expected by the private backend.
    pub descriptor_sha256: String,
    /// Namespace-local synchronous ID material, present only for Durable Objects.
    pub durable_object_identity: Option<DurableObjectFacadeIdentity>,
}

/// One verified immutable Queue producer binding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeQueueBinding {
    /// Canonical Queue producer descriptor supplied only to the loader binding factory.
    pub descriptor: QueueProducerBindingDescriptorV1,
    /// Lowercase canonical descriptor SHA-256.
    pub descriptor_sha256: String,
}

/// Secret-bearing Durable Object facade material supplied only to the loaded-isolate factory.
#[derive(Clone)]
pub struct DurableObjectFacadeIdentity {
    /// Eight-byte namespace prefix encoded as lowercase hexadecimal.
    pub namespace_prefix: String,
    /// Namespace-specific HMAC key encoded as standard base64.
    pub namespace_name_key: SecretString,
}

impl PartialEq for DurableObjectFacadeIdentity {
    fn eq(&self, other: &Self) -> bool {
        self.namespace_prefix == other.namespace_prefix
            && self.namespace_name_key.expose() == other.namespace_name_key.expose()
    }
}

impl Eq for DurableObjectFacadeIdentity {}

impl std::fmt::Debug for DurableObjectFacadeIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DurableObjectFacadeIdentity")
            .field("namespace_prefix", &self.namespace_prefix)
            .field("namespace_name_key", &"[REDACTED]")
            .finish()
    }
}

impl std::fmt::Debug for RuntimeModule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeModule")
            .field("name", &self.name)
            .field("module_type", &self.module_type)
            .field("size", &self.bytes.len())
            .finish()
    }
}

impl std::fmt::Debug for RuntimeModuleBinding {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RuntimeModuleBinding")
            .field("name", &self.name)
            .field("module_type", &self.module_type)
            .field("size", &self.bytes.len())
            .finish()
    }
}

/// Verified Workflow facade descriptor with its independently checked canonical digest.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeWorkflowBinding {
    /// Frozen catalog binding identity.
    #[serde(flatten)]
    pub descriptor: open_compute_storage::WorkflowBindingDescriptor,
    /// Canonical digest used by the trusted private binding backend.
    pub descriptor_sha256: String,
}

/// Verified immutable target set for one native scheduled event expression.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeScheduledTarget {
    /// Exact version-declared cron expression.
    pub cron: String,
    /// Whether the tenant default scheduled handler is invoked.
    pub scheduled_handler: bool,
    /// Direct Workflow bindings invoked for the logical slot.
    pub workflow_bindings: Vec<String>,
}

/// Verified dynamic Service declaration supplied to the trusted loader host.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeServiceBinding {
    /// Canonical immutable declaration.
    #[serde(flatten)]
    pub descriptor: ServiceDescriptorV1,
    /// Independently verified descriptor digest.
    pub descriptor_sha256: String,
}

/// Verified automatic response-cache policy projected to the loaded isolate wrapper.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeCachePolicy {
    /// Default export policy.
    pub enabled: bool,
    /// Default cross-version cache scope.
    pub cross_version_cache: bool,
    /// Whether automatic lookup availability failures bypass to tenant code.
    pub fail_open: bool,
    /// Named entrypoint overrides.
    pub entrypoints: BTreeMap<String, CacheEntrypointPolicyV1>,
}

/// Verified platform-provided Images binding.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeImagesBinding {
    /// Tenant environment name.
    pub name: String,
    /// Independently verified canonical descriptor digest.
    pub descriptor_sha256: String,
}

/// Verified standard Workers AI binding limited to Markdown Conversion.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeAiBinding {
    /// Tenant environment name.
    pub name: String,
    /// Independently verified canonical descriptor digest.
    pub descriptor_sha256: String,
}

/// Verified immutable version Version Metadata binding.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeVersionMetadataBinding {
    /// Tenant environment name.
    pub name: String,
    /// Immutable version ID.
    pub id: String,
    /// Optional application release tag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// Immutable version creation timestamp in Unix milliseconds.
    pub timestamp_ms: i64,
    /// Independently verified canonical descriptor digest.
    pub descriptor_sha256: String,
}

/// Optional static-assets fetch capability exposed under one declared env name.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeAssetBinding {
    /// Tenant environment name.
    pub name: String,
}

/// Verified static-asset routing data consumed only by the trusted loader host.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeAssets {
    /// Canonical path-to-object manifest.
    pub manifest: AssetManifestV1,
    /// Canonical default-route and response configuration.
    pub routing: AssetRoutingConfigV1,
}

/// Secret-free Script identity and effective Workers Logs policy for the internal collector.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeObservabilityIdentity {
    /// Private protocol version.
    pub schema_version: u32,
    /// Owning account identity.
    pub account_id: String,
    /// Internal Worker identity used only for authority verification.
    pub worker_id: String,
    /// External Cloudflare Script name.
    pub script_name: String,
    /// External immutable Version identity.
    pub version_id: String,
    /// Active Deployment identity, when this Version is currently deployed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deployment_id: Option<String>,
    /// Worker routing generation frozen for this assembly.
    pub route_generation: u64,
    /// Script observability setting generation.
    pub observability_generation: u64,
    /// Master persistence switch.
    pub enabled: bool,
    /// Logs collection switch.
    pub logs_enabled: bool,
    /// Deterministic invocation head-sampling rate.
    pub head_sampling_rate: f64,
    /// Whether invocation summaries are persisted.
    pub invocation_logs: bool,
    /// Whether selected logs are persisted.
    pub persist: bool,
}

/// Fully verified immutable version assembly.
#[derive(Clone)]
pub struct RuntimeSnapshot {
    /// Canonical loader key.
    pub loader_key: String,
    /// Descriptor digest checked before loader get.
    pub worker_code_sha256: String,
    /// Current Worker route generation used to fence Durable Object dispatch.
    pub route_generation: u64,
    /// Internal collector identity; absent from validation and probe snapshots.
    pub observability: Option<RuntimeObservabilityIdentity>,
    /// Immutable compatibility date for this Version.
    pub compatibility_date: String,
    /// Immutable compatibility flags for this Version.
    pub compatibility_flags: Vec<String>,
    /// Executable or assets-only content discriminator.
    pub content_kind: VersionContentKind,
    /// Main module for executable Workers.
    pub main_module: Option<String>,
    /// Verified modules.
    pub modules: Vec<RuntimeModule>,
    /// Verified service-worker globals backed by module parts.
    pub module_bindings: Vec<RuntimeModuleBinding>,
    /// Canonical structured-clone-compatible vars.
    pub vars: BTreeMap<String, serde_json::Value>,
    /// Decrypted secret values. Empty in validation scope.
    pub secrets: BTreeMap<String, SecretString>,
    /// Verified runtime bindings. Empty in validation and probe scopes.
    pub bindings: Vec<RuntimeBinding>,
    /// Verified Queue producer bindings. Empty in validation and probe scopes.
    pub queue_bindings: Vec<RuntimeQueueBinding>,
    /// Verified Workflow caller bindings, carrying no execution or creation tokens.
    pub workflow_bindings: Vec<RuntimeWorkflowBinding>,
    /// Verified version Cron targets used by the generated system adapter.
    pub scheduled_targets: Vec<RuntimeScheduledTarget>,
    /// Verified lazy Service declarations.
    pub services: Vec<RuntimeServiceBinding>,
    /// Verified automatic response-cache policy.
    pub cache_policy: RuntimeCachePolicy,
    /// Optional Workers AI Markdown Conversion capability.
    pub ai_binding: Option<RuntimeAiBinding>,
    /// Optional local Images capability.
    pub images_binding: Option<RuntimeImagesBinding>,
    /// Optional immutable Version Metadata environment object.
    pub version_metadata_binding: Option<RuntimeVersionMetadataBinding>,
    /// Optional version-scoped static-assets fetch capability.
    pub asset_binding: Option<RuntimeAssetBinding>,
    /// Optional verified static assets used by the trusted default HTTP router.
    pub assets: Option<RuntimeAssets>,
}

impl std::fmt::Debug for RuntimeSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeSnapshot")
            .field("loader_key", &self.loader_key)
            .field("worker_code_sha256", &self.worker_code_sha256)
            .field("observability", &self.observability)
            .field("main_module", &self.main_module)
            .field("module_count", &self.modules.len())
            .field("module_binding_count", &self.module_bindings.len())
            .field("var_count", &self.vars.len())
            .field("secret_count", &self.secrets.len())
            .field("binding_count", &self.bindings.len())
            .field("queue_binding_count", &self.queue_bindings.len())
            .field("workflow_binding_count", &self.workflow_bindings.len())
            .field("scheduled_target_count", &self.scheduled_targets.len())
            .field("service_count", &self.services.len())
            .field("cache_enabled", &self.cache_policy.enabled)
            .field("ai_binding", &self.ai_binding.is_some())
            .field("images_binding", &self.images_binding.is_some())
            .field(
                "version_metadata_binding",
                &self.version_metadata_binding.is_some(),
            )
            .field("asset_binding", &self.asset_binding.is_some())
            .finish_non_exhaustive()
    }
}

/// Zeroizing internal JSON response. Debug never renders its body.
pub struct RuntimePayload {
    bytes: Vec<u8>,
}

impl RuntimePayload {
    /// Borrow bytes for the generation-authenticated loopback response.
    #[must_use]
    pub fn expose(&self) -> &[u8] {
        &self.bytes
    }
}

impl std::fmt::Debug for RuntimePayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimePayload")
            .field("size", &self.bytes.len())
            .finish_non_exhaustive()
    }
}

impl Drop for RuntimePayload {
    fn drop(&mut self) {
        self.bytes.zeroize();
    }
}

/// `RuntimeSource` authority over typed storage and `ArtifactStore`.
#[derive(Clone)]
pub struct RuntimeSource {
    storage: Arc<PlatformStorage>,
    artifacts: ArtifactStore,
    cache: Option<Arc<ArtifactCache>>,
    cache_fail_open: bool,
    limits: BundleLimits,
}

impl std::fmt::Debug for RuntimeSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeSource")
            .field("artifacts", &self.artifacts)
            .field("cache", &self.cache.is_some())
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl RuntimeSource {
    /// Bind immutable authorities. No raw database path or object key is exposed.
    #[must_use]
    pub fn new(
        storage: Arc<PlatformStorage>,
        artifacts: ArtifactStore,
        limits: BundleLimits,
    ) -> Self {
        Self {
            storage,
            artifacts,
            cache: None,
            cache_fail_open: true,
            limits,
        }
    }

    /// Resolve verified artifacts through the platform's bounded local cache.
    #[must_use]
    pub fn with_cache(mut self, cache: Arc<ArtifactCache>) -> Self {
        self.cache = Some(cache);
        self
    }

    /// Apply the operator-owned automatic-cache availability policy.
    #[must_use]
    pub const fn with_cache_fail_open(mut self, fail_open: bool) -> Self {
        self.cache_fail_open = fail_open;
        self
    }

    /// Resolve, verify, decrypt if allowed, and assemble one immutable version.
    pub async fn resolve(
        &self,
        key: &str,
        expected_worker_code_sha256: &str,
        scope: RuntimeScope,
    ) -> Result<RuntimeSnapshot, PlatformError> {
        let (account_id, worker_id, version_id) = parse_loader_key(key)?;
        if expected_worker_code_sha256.len() != 64
            || expected_worker_code_sha256
                .bytes()
                .any(|byte| !byte.is_ascii_hexdigit() || byte.is_ascii_uppercase())
        {
            return Err(invariant());
        }
        let repo = WorkerRepository::new(self.storage.db());
        let snapshot = repo.version_snapshot(
            account_id,
            worker_id,
            version_id,
            matches!(scope, RuntimeScope::Validation | RuntimeScope::Probe),
        )?;
        let observability = if scope == RuntimeScope::Runtime {
            let settings = repo.get_observability_settings(account_id, worker_id)?;
            Some(RuntimeObservabilityIdentity {
                schema_version: 1,
                account_id: account_id.to_string(),
                worker_id: worker_id.to_string(),
                script_name: snapshot.worker.name.clone(),
                version_id: version_id.to_string(),
                deployment_id: snapshot
                    .worker
                    .active_deployment_id
                    .filter(|_| snapshot.worker.active_version_id == Some(version_id))
                    .map(|value| value.to_string()),
                route_generation: snapshot.worker.route_generation,
                observability_generation: settings.generation,
                enabled: settings.enabled,
                logs_enabled: settings.logs_enabled,
                head_sampling_rate: settings.effective_head_sampling_rate(),
                invocation_logs: settings.invocation_logs,
                persist: settings.persist,
            })
        } else {
            None
        };
        match scope {
            RuntimeScope::Runtime if snapshot.version.state != VersionState::Ready => {
                return Err(not_ready());
            }
            RuntimeScope::Validation if snapshot.version.state != VersionState::Validating => {
                return Err(not_ready());
            }
            RuntimeScope::Probe
                if !matches!(
                    snapshot.version.state,
                    VersionState::Validating | VersionState::Ready
                ) =>
            {
                return Err(not_ready());
            }
            RuntimeScope::Runtime | RuntimeScope::Validation | RuntimeScope::Probe => {}
        }
        let assets = snapshot
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
            .transpose()?;
        let bundle = match snapshot.version.content_kind {
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
                let bytes = match &self.cache {
                    Some(cache) => {
                        let mut pinned = cache
                            .acquire(&self.artifacts, &artifact)
                            .await
                            .map_err(map_artifact_error)?;
                        pinned.read_all().map_err(map_artifact_error)?
                    }
                    None => self
                        .artifacts
                        .open(&artifact)
                        .await
                        .map_err(map_artifact_error)?
                        .to_vec(),
                };
                let bundle = CanonicalBundle::parse(bytes, self.limits)?;
                if bundle.sha256() != artifact_sha256
                    || bundle.manifest().main_module != main_module
                {
                    return Err(invariant());
                }
                Some(bundle)
            }
            VersionContentKind::AssetsOnly if scope == RuntimeScope::Runtime => {
                if assets.is_none() {
                    return Err(invariant());
                }
                None
            }
            VersionContentKind::AssetsOnly => return Err(not_ready()),
        };

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
        let mut secret_descriptors = Vec::with_capacity(snapshot.secrets.len());
        for secret in snapshot.secrets.values() {
            secret_descriptors.push(SecretDescriptor {
                name: secret.name.clone(),
                revision_id: secret.revision_id.clone(),
                ciphertext_sha256: ciphertext_sha256(
                    &secret.envelope.nonce,
                    &secret.envelope.ciphertext,
                ),
            });
        }
        let mut binding_descriptors = Vec::with_capacity(snapshot.bindings.len());
        let mut runtime_bindings = Vec::with_capacity(snapshot.bindings.len());
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
            binding_descriptors.push(descriptor.clone());
            runtime_bindings.push(RuntimeBinding {
                descriptor,
                descriptor_sha256: hex::encode(digest),
                durable_object_identity: if binding.kind == BindingKind::DoNamespace
                    && scope == RuntimeScope::Runtime
                {
                    let (prefix, key) = DurableObjectRepository::new(&self.storage)
                        .facade_identity(binding.resource_id)?;
                    Some(DurableObjectFacadeIdentity {
                        namespace_prefix: hex::encode(prefix),
                        namespace_name_key: SecretString::new(
                            base64::engine::general_purpose::STANDARD.encode(key),
                        ),
                    })
                } else {
                    None
                },
            });
        }
        let mut queue_binding_descriptors = Vec::with_capacity(snapshot.queue_bindings.len());
        let mut runtime_queue_bindings = Vec::with_capacity(snapshot.queue_bindings.len());
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
            queue_binding_descriptors.push(descriptor.clone());
            runtime_queue_bindings.push(RuntimeQueueBinding {
                descriptor,
                descriptor_sha256: hex::encode(digest),
            });
        }
        let mut workflow_binding_descriptors = Vec::with_capacity(snapshot.workflow_bindings.len());
        let mut runtime_workflow_bindings = Vec::with_capacity(snapshot.workflow_bindings.len());
        for binding in &snapshot.workflow_bindings {
            let digest = binding.descriptor.sha256()?;
            if digest != binding.descriptor_sha256 {
                return Err(invariant());
            }
            workflow_binding_descriptors.push(binding.descriptor.clone());
            runtime_workflow_bindings.push(RuntimeWorkflowBinding {
                descriptor: binding.descriptor.clone(),
                descriptor_sha256: hex::encode(digest),
            });
        }
        let scheduled_targets = if snapshot.version.content_kind == VersionContentKind::Worker {
            let cron = open_compute_storage::CronRepository::new(self.storage.db())
                .version_config(version_id)?;
            for declaration in &cron.declarations {
                for name in &declaration.workflow_bindings {
                    let Some(binding) = runtime_workflow_bindings
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
            for binding in &runtime_workflow_bindings {
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
            cron.declarations
                .into_iter()
                .map(|declaration| RuntimeScheduledTarget {
                    cron: declaration.expression,
                    scheduled_handler: declaration.scheduled_handler,
                    workflow_bindings: declaration.workflow_bindings,
                })
                .collect()
        } else {
            Vec::new()
        };
        let mut service_descriptors = Vec::with_capacity(snapshot.services.len());
        let mut runtime_services = Vec::with_capacity(snapshot.services.len());
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
            service_descriptors.push(descriptor.clone());
            runtime_services.push(RuntimeServiceBinding {
                descriptor,
                descriptor_sha256: hex::encode(digest),
            });
        }
        let mut cache_policy = CachePolicyDescriptorV1::default();
        for policy in &snapshot.cache_policies {
            match &policy.entrypoint {
                None => {
                    cache_policy.enabled = policy.enabled;
                    cache_policy.cross_version_cache = policy.cross_version_cache;
                }
                Some(name) => {
                    cache_policy.entrypoints.insert(
                        name.clone(),
                        CacheEntrypointPolicyV1 {
                            enabled: policy.enabled,
                            cross_version_cache: policy.cross_version_cache,
                        },
                    );
                }
            }
        }
        cache_policy.validate()?;
        let mut builtin_descriptors = Vec::with_capacity(snapshot.builtin_bindings.len());
        let mut ai_binding = None;
        let mut images_binding = None;
        let mut version_metadata_binding = None;
        let mut module_bindings = Vec::new();
        for binding in &snapshot.builtin_bindings {
            let kind = match binding.kind {
                BuiltinBindingKind::Ai => BuiltinBindingDescriptorKindV1::Ai,
                BuiltinBindingKind::Images => BuiltinBindingDescriptorKindV1::Images,
                BuiltinBindingKind::VersionMetadata => {
                    BuiltinBindingDescriptorKindV1::VersionMetadata
                }
                BuiltinBindingKind::WasmModule => BuiltinBindingDescriptorKindV1::WasmModule,
                BuiltinBindingKind::TextBlob => BuiltinBindingDescriptorKindV1::TextBlob,
                BuiltinBindingKind::DataBlob => BuiltinBindingDescriptorKindV1::DataBlob,
            };
            let descriptor =
                BuiltinBindingDescriptorV1::new(binding.name.clone(), kind, binding.tag.clone())?;
            let digest = descriptor.sha256()?;
            if digest != binding.descriptor_sha256 {
                return Err(invariant());
            }
            match binding.kind {
                BuiltinBindingKind::Ai => {
                    ai_binding = Some(RuntimeAiBinding {
                        name: binding.name.clone(),
                        descriptor_sha256: hex::encode(digest),
                    });
                }
                BuiltinBindingKind::Images => {
                    images_binding = Some(RuntimeImagesBinding {
                        name: binding.name.clone(),
                        descriptor_sha256: hex::encode(digest),
                    });
                }
                BuiltinBindingKind::VersionMetadata => {
                    version_metadata_binding = Some(RuntimeVersionMetadataBinding {
                        name: binding.name.clone(),
                        id: version_id.to_string(),
                        tag: binding.tag.clone(),
                        timestamp_ms: snapshot.version.created_at_ms,
                        descriptor_sha256: hex::encode(digest),
                    });
                }
                BuiltinBindingKind::WasmModule
                | BuiltinBindingKind::TextBlob
                | BuiltinBindingKind::DataBlob => {
                    let module_type = match binding.kind {
                        BuiltinBindingKind::WasmModule => ModuleType::Wasm,
                        BuiltinBindingKind::TextBlob => ModuleType::Text,
                        BuiltinBindingKind::DataBlob => ModuleType::Data,
                        _ => return Err(invariant()),
                    };
                    let module_name = binding.tag.as_deref().ok_or_else(invariant)?;
                    let bundle = bundle.as_ref().ok_or_else(invariant)?;
                    let module = bundle
                        .manifest()
                        .modules
                        .iter()
                        .find(|module| {
                            module.name == module_name && module.module_type == module_type
                        })
                        .ok_or_else(invariant)?;
                    module_bindings.push(RuntimeModuleBinding {
                        name: binding.name.clone(),
                        module_type,
                        bytes: bundle.module_bytes(module)?.to_vec(),
                    });
                }
            }
            builtin_descriptors.push(descriptor);
        }
        let descriptor = WorkerCodeDescriptorV1::new(
            account_id,
            worker_id,
            version_id,
            snapshot.version.created_at_ms,
            snapshot.version.compatibility_date.clone(),
            snapshot.version.compatibility_flags.clone(),
            bundle
                .as_ref()
                .map(|bundle| (bundle.sha256(), bundle.manifest())),
            assets
                .as_ref()
                .map(|(manifest, routing)| (manifest, routing)),
            vars.clone(),
            secret_descriptors,
            binding_descriptors,
            queue_binding_descriptors,
            workflow_binding_descriptors,
            service_descriptors,
            cache_policy.clone(),
            builtin_descriptors,
            snapshot.version.loader_schema_version,
        )?;
        let actual_descriptor = descriptor.sha256()?;
        if actual_descriptor != snapshot.version.worker_code_sha256
            || hex::encode(actual_descriptor) != expected_worker_code_sha256
        {
            return Err(invariant());
        }

        let mut modules = Vec::new();
        if let Some(bundle) = &bundle {
            modules.reserve(bundle.manifest().modules.len());
            for module in &bundle.manifest().modules {
                if module.module_type == ModuleType::SourceMap {
                    continue;
                }
                modules.push(RuntimeModule {
                    name: module.name.clone(),
                    module_type: module.module_type,
                    bytes: bundle.module_bytes(module)?.to_vec(),
                });
            }
        }
        let mut secrets = BTreeMap::new();
        if scope == RuntimeScope::Runtime {
            for secret in snapshot.secrets.values() {
                let plaintext = self.storage.crypto().decrypt(
                    &secret.envelope,
                    account_id,
                    worker_id,
                    version_id,
                    &secret.name,
                    &secret.revision_id,
                )?;
                let text = std::str::from_utf8(plaintext.expose()).map_err(|_| {
                    PlatformError::new(ErrorCode::SecretInvalid, "secret is not valid UTF-8")
                })?;
                secrets.insert(secret.name.clone(), SecretString::new(text));
            }
            crate::pipeline::validate_secret_set(&secrets, &vars).map_err(|_| invariant())?;
        }
        let asset_binding = assets
            .as_ref()
            .and_then(|(_, routing)| routing.binding.clone())
            .map(|name| RuntimeAssetBinding { name });
        Ok(RuntimeSnapshot {
            loader_key: key.to_owned(),
            worker_code_sha256: hex::encode(actual_descriptor),
            route_generation: snapshot.worker.route_generation,
            observability,
            compatibility_date: snapshot.version.compatibility_date,
            compatibility_flags: snapshot.version.compatibility_flags,
            content_kind: snapshot.version.content_kind,
            main_module: bundle
                .as_ref()
                .map(|bundle| bundle.manifest().main_module.clone()),
            modules,
            module_bindings,
            vars,
            secrets,
            bindings: runtime_bindings,
            queue_bindings: runtime_queue_bindings,
            workflow_bindings: runtime_workflow_bindings,
            scheduled_targets,
            services: runtime_services,
            cache_policy: RuntimeCachePolicy {
                enabled: cache_policy.enabled,
                cross_version_cache: cache_policy.cross_version_cache,
                fail_open: self.cache_fail_open,
                entrypoints: cache_policy.entrypoints,
            },
            ai_binding,
            images_binding,
            version_metadata_binding,
            asset_binding,
            assets: assets.map(|(manifest, routing)| RuntimeAssets { manifest, routing }),
        })
    }

    /// Encode the scoped snapshot for the authenticated loader-host bridge.
    pub fn internal_payload(snapshot: &RuntimeSnapshot) -> Result<RuntimePayload, PlatformError> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Module<'a> {
            name: &'a str,
            #[serde(rename = "type")]
            module_type: ModuleType,
            bytes_base64: String,
        }
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Payload<'a> {
            schema_version: u32,
            loader_key: &'a str,
            worker_code_sha256: &'a str,
            route_generation: u64,
            #[serde(skip_serializing_if = "Option::is_none")]
            observability: Option<&'a RuntimeObservabilityIdentity>,
            compatibility_date: &'a str,
            compatibility_flags: &'a [String],
            content_kind: VersionContentKind,
            #[serde(skip_serializing_if = "Option::is_none")]
            main_module: Option<&'a str>,
            modules: Vec<Module<'a>>,
            module_bindings: Vec<Module<'a>>,
            env: BTreeMap<&'a str, serde_json::Value>,
            bindings: Vec<BindingPayload<'a>>,
            scheduled_targets: &'a [RuntimeScheduledTarget],
            services: &'a [RuntimeServiceBinding],
            cache_policy: &'a RuntimeCachePolicy,
            #[serde(skip_serializing_if = "Option::is_none")]
            ai_binding: Option<&'a RuntimeAiBinding>,
            #[serde(skip_serializing_if = "Option::is_none")]
            images_binding: Option<&'a RuntimeImagesBinding>,
            #[serde(skip_serializing_if = "Option::is_none")]
            version_metadata_binding: Option<&'a RuntimeVersionMetadataBinding>,
            #[serde(skip_serializing_if = "Option::is_none")]
            asset_binding: Option<&'a RuntimeAssetBinding>,
            #[serde(skip_serializing_if = "Option::is_none")]
            assets: Option<&'a RuntimeAssets>,
        }
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct ResourceBindingPayload<'a> {
            #[serde(flatten)]
            descriptor: &'a BindingDescriptorV1,
            descriptor_sha256: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            namespace_prefix: Option<&'a str>,
            #[serde(skip_serializing_if = "Option::is_none")]
            namespace_name_key: Option<&'a str>,
        }
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct QueueBindingPayload<'a> {
            #[serde(flatten)]
            descriptor: &'a QueueProducerBindingDescriptorV1,
            descriptor_sha256: &'a str,
        }
        #[derive(Serialize)]
        #[serde(untagged)]
        enum BindingPayload<'a> {
            Resource(ResourceBindingPayload<'a>),
            Queue(QueueBindingPayload<'a>),
            Workflow(&'a RuntimeWorkflowBinding),
        }
        let modules = snapshot
            .modules
            .iter()
            .map(|module| Module {
                name: &module.name,
                module_type: module.module_type,
                bytes_base64: base64::engine::general_purpose::STANDARD.encode(&module.bytes),
            })
            .collect();
        let module_bindings = snapshot
            .module_bindings
            .iter()
            .map(|binding| Module {
                name: &binding.name,
                module_type: binding.module_type,
                bytes_base64: base64::engine::general_purpose::STANDARD.encode(&binding.bytes),
            })
            .collect();
        let mut env: BTreeMap<&str, serde_json::Value> = snapshot
            .vars
            .iter()
            .map(|(name, value)| (name.as_str(), value.clone()))
            .collect();
        for (name, value) in &snapshot.secrets {
            env.insert(
                name.as_str(),
                serde_json::Value::String(value.expose().to_owned()),
            );
        }
        let bindings = snapshot
            .bindings
            .iter()
            .map(|binding| {
                BindingPayload::Resource(ResourceBindingPayload {
                    descriptor: &binding.descriptor,
                    descriptor_sha256: &binding.descriptor_sha256,
                    namespace_prefix: binding
                        .durable_object_identity
                        .as_ref()
                        .map(|identity| identity.namespace_prefix.as_str()),
                    namespace_name_key: binding
                        .durable_object_identity
                        .as_ref()
                        .map(|identity| identity.namespace_name_key.expose()),
                })
            })
            .chain(snapshot.queue_bindings.iter().map(|binding| {
                BindingPayload::Queue(QueueBindingPayload {
                    descriptor: &binding.descriptor,
                    descriptor_sha256: &binding.descriptor_sha256,
                })
            }))
            .chain(
                snapshot
                    .workflow_bindings
                    .iter()
                    .map(BindingPayload::Workflow),
            )
            .collect();
        let bytes = serde_json::to_vec(&Payload {
            schema_version: 1,
            loader_key: &snapshot.loader_key,
            worker_code_sha256: &snapshot.worker_code_sha256,
            route_generation: snapshot.route_generation,
            observability: snapshot.observability.as_ref(),
            compatibility_date: &snapshot.compatibility_date,
            compatibility_flags: &snapshot.compatibility_flags,
            content_kind: snapshot.content_kind,
            main_module: snapshot.main_module.as_deref(),
            modules,
            module_bindings,
            env,
            bindings,
            scheduled_targets: &snapshot.scheduled_targets,
            services: &snapshot.services,
            cache_policy: &snapshot.cache_policy,
            ai_binding: snapshot.ai_binding.as_ref(),
            images_binding: snapshot.images_binding.as_ref(),
            version_metadata_binding: snapshot.version_metadata_binding.as_ref(),
            asset_binding: snapshot.asset_binding.as_ref(),
            assets: snapshot.assets.as_ref(),
        })
        .map_err(|_| invariant())?;
        Ok(RuntimePayload { bytes })
    }
}

#[allow(clippy::needless_pass_by_value)]
pub(crate) fn map_artifact_error(error: PlatformError) -> PlatformError {
    match error.code() {
        ErrorCode::ArtifactIntegrityError | ErrorCode::CacheEntryCorrupt => PlatformError::new(
            ErrorCode::ArtifactIntegrityError,
            "runtime artifact failed integrity verification",
        ),
        _ => PlatformError::new(
            ErrorCode::ArtifactUnavailable,
            "runtime artifact is unavailable",
        ),
    }
}

pub(crate) fn not_ready() -> PlatformError {
    PlatformError::new(
        ErrorCode::VersionNotReady,
        "version is not available in this RuntimeSource scope",
    )
}

pub(crate) fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::VersionInvariantViolation,
        "RuntimeSource descriptor invariant failed",
    )
}
