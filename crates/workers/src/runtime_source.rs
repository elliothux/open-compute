//! Scoped immutable `RuntimeSource` assembly from `SQLite` and verified artifacts.

use crate::bundle::{BundleLimits, CanonicalBundle, ModuleType};
use crate::descriptor::{
    BindingDescriptorV1, QueueProducerBindingDescriptorV1, SecretDescriptor,
    WorkerCodeDescriptorV1, ciphertext_sha256, parse_loader_key,
};
use base64::Engine as _;
use open_compute_artifacts::{ARTIFACT_KEY_VERSION, ArtifactCache, ArtifactRef, ArtifactStore};
use open_compute_core::{BindingKind, ErrorCode, PlatformError, SecretString};
use open_compute_storage::{
    DeploymentState, DurableObjectRepository, PlatformStorage, WorkerRepository,
};
use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::Arc;
use zeroize::Zeroize;

/// `RuntimeSource` authorization scope.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeScope {
    /// Only immutable ready deployments.
    Runtime,
    /// Only a currently validating deployment; secrets are omitted.
    Validation,
    /// Only a ready deployment used to prove a named export; secrets are omitted.
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

/// Fully verified immutable deployment assembly.
#[derive(Clone)]
pub struct RuntimeSnapshot {
    /// Canonical loader key.
    pub loader_key: String,
    /// Descriptor digest checked before loader get.
    pub worker_code_sha256: String,
    /// Current Worker route generation used to fence Durable Object dispatch.
    pub route_generation: u64,
    /// Main module.
    pub main_module: String,
    /// Exact tenant compatibility date.
    pub compatibility_date: String,
    /// Canonically sorted flags.
    pub compatibility_flags: Vec<String>,
    /// Verified modules.
    pub modules: Vec<RuntimeModule>,
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
    /// Immutable resource limits.
    pub limits: serde_json::Value,
}

impl std::fmt::Debug for RuntimeSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeSnapshot")
            .field("loader_key", &self.loader_key)
            .field("worker_code_sha256", &self.worker_code_sha256)
            .field("main_module", &self.main_module)
            .field("module_count", &self.modules.len())
            .field("var_count", &self.vars.len())
            .field("secret_count", &self.secrets.len())
            .field("binding_count", &self.bindings.len())
            .field("queue_binding_count", &self.queue_bindings.len())
            .field("workflow_binding_count", &self.workflow_bindings.len())
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
    /// Bind immutable authorities. No raw database path or S3 key is exposed.
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
            limits,
        }
    }

    /// Resolve verified artifacts through the platform's bounded local cache.
    #[must_use]
    pub fn with_cache(mut self, cache: Arc<ArtifactCache>) -> Self {
        self.cache = Some(cache);
        self
    }

    /// Resolve, verify, decrypt if allowed, and assemble one immutable deployment.
    pub async fn resolve(
        &self,
        key: &str,
        expected_worker_code_sha256: &str,
        scope: RuntimeScope,
    ) -> Result<RuntimeSnapshot, PlatformError> {
        let (account_id, worker_id, deployment_id) = parse_loader_key(key)?;
        if expected_worker_code_sha256.len() != 64
            || expected_worker_code_sha256
                .bytes()
                .any(|byte| !byte.is_ascii_hexdigit() || byte.is_ascii_uppercase())
        {
            return Err(invariant());
        }
        let repo = WorkerRepository::new(self.storage.db());
        let snapshot = repo.deployment_snapshot(
            account_id,
            worker_id,
            deployment_id,
            scope == RuntimeScope::Validation,
        )?;
        match scope {
            RuntimeScope::Runtime if snapshot.deployment.state != DeploymentState::Ready => {
                return Err(not_ready());
            }
            RuntimeScope::Validation
                if snapshot.deployment.state != DeploymentState::Validating =>
            {
                return Err(not_ready());
            }
            RuntimeScope::Probe if snapshot.deployment.state != DeploymentState::Ready => {
                return Err(not_ready());
            }
            RuntimeScope::Runtime | RuntimeScope::Validation | RuntimeScope::Probe => {}
        }
        let artifact = ArtifactRef::new(
            ARTIFACT_KEY_VERSION,
            &hex::encode(snapshot.deployment.artifact_sha256),
            snapshot.deployment.artifact_size,
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
        if bundle.sha256() != snapshot.deployment.artifact_sha256
            || bundle.manifest().main_module != snapshot.deployment.main_module
        {
            return Err(invariant());
        }

        let mut vars = BTreeMap::new();
        for (name, raw) in &snapshot.vars {
            let value = serde_json::from_slice(raw).map_err(|_| invariant())?;
            vars.insert(name.clone(), value);
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
                binding.config,
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
        let descriptor = WorkerCodeDescriptorV1::new(
            account_id,
            worker_id,
            deployment_id,
            bundle.sha256(),
            bundle.manifest(),
            snapshot.deployment.compatibility_date.clone(),
            snapshot.deployment.compatibility_flags.clone(),
            vars.clone(),
            secret_descriptors,
            binding_descriptors,
            queue_binding_descriptors,
            workflow_binding_descriptors,
            snapshot.deployment.limits.clone(),
            snapshot.deployment.loader_schema_version,
        )?;
        let actual_descriptor = descriptor.sha256()?;
        if actual_descriptor != snapshot.deployment.worker_code_sha256
            || hex::encode(actual_descriptor) != expected_worker_code_sha256
        {
            return Err(invariant());
        }

        let mut modules = Vec::with_capacity(bundle.manifest().modules.len());
        for module in &bundle.manifest().modules {
            modules.push(RuntimeModule {
                name: module.name.clone(),
                module_type: module.module_type,
                bytes: bundle.module_bytes(module)?.to_vec(),
            });
        }
        let mut secrets = BTreeMap::new();
        if scope == RuntimeScope::Runtime {
            for secret in snapshot.secrets.values() {
                let plaintext = self.storage.crypto().decrypt(
                    &secret.envelope,
                    account_id,
                    worker_id,
                    deployment_id,
                    &secret.name,
                    &secret.revision_id,
                )?;
                let text = std::str::from_utf8(plaintext.expose()).map_err(|_| {
                    PlatformError::new(ErrorCode::SecretInvalid, "secret is not valid UTF-8")
                })?;
                secrets.insert(secret.name.clone(), SecretString::new(text));
            }
        }
        Ok(RuntimeSnapshot {
            loader_key: key.to_owned(),
            worker_code_sha256: hex::encode(actual_descriptor),
            route_generation: snapshot.worker.route_generation,
            main_module: bundle.manifest().main_module.clone(),
            compatibility_date: snapshot.deployment.compatibility_date,
            compatibility_flags: snapshot.deployment.compatibility_flags,
            modules,
            vars,
            secrets,
            bindings: runtime_bindings,
            queue_bindings: runtime_queue_bindings,
            workflow_bindings: runtime_workflow_bindings,
            limits: snapshot.deployment.limits,
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
            main_module: &'a str,
            compatibility_date: &'a str,
            compatibility_flags: &'a [String],
            modules: Vec<Module<'a>>,
            env: BTreeMap<&'a str, serde_json::Value>,
            bindings: Vec<BindingPayload<'a>>,
            limits: &'a serde_json::Value,
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
            main_module: &snapshot.main_module,
            compatibility_date: &snapshot.compatibility_date,
            compatibility_flags: &snapshot.compatibility_flags,
            modules,
            env,
            bindings,
            limits: &snapshot.limits,
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
        ErrorCode::DeploymentNotReady,
        "deployment is not available in this RuntimeSource scope",
    )
}

pub(crate) fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::DeploymentInvariantViolation,
        "RuntimeSource descriptor invariant failed",
    )
}
