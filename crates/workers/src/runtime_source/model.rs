use super::*;

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
    /// Verified native Worker Loader bindings.
    pub worker_loaders: Vec<RuntimeWorkerLoaderBinding>,
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
            .field("worker_loader_count", &self.worker_loaders.len())
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
    pub(super) bytes: Vec<u8>,
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
