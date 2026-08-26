//! Immutable deployment creation pipeline.

use crate::bundle::{
    BundleLimits, CanonicalBundle, StagedBundle, WORKER_BUNDLE_SCHEMA_VERSION, WorkerBundleManifest,
};
use crate::descriptor::{
    BindingDescriptorV1, D1_FACADE_MODULE_NAME, DO_ALARM_SHIM_MODULE_NAME, DO_FACADE_MODULE_NAME,
    DO_ID_CODEC_MODULE_NAME, LOADED_ISOLATE_WRAPPER_MODULE_NAME, R2_FACADE_MODULE_NAME,
    SecretDescriptor, WorkerCodeDescriptorV1, canonicalize_vars, ciphertext_sha256,
    validate_env_name,
};
use bytes::Bytes;
use futures::stream;
use open_compute_artifacts::ArtifactStore;
use open_compute_core::{
    AccountId, BindingId, BindingKind, CanonicalBindingConfig, CanonicalPermissions, DeploymentId,
    ErrorCode, OperationClass, PlatformError, RequestId, ResourceId, ResourceState, SecretBytes,
    SecretString, WorkerId,
};
use open_compute_storage::{
    DeploymentRecord, DeploymentState, DurableObjectRepository, IdempotencyReservation,
    LOADER_SCHEMA_VERSION, NewDeployment, NewDeploymentBinding, PlatformStorage,
    ResourceRepository, StoredDeploymentSecret, WorkerRepository,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use uuid::Uuid;
use zeroize::Zeroize;

const MAX_VARS: usize = 64;
const MAX_ENV_BYTES: usize = 64 * 1024;
const MAX_SECRETS: usize = 64;
const MAX_SECRET_BYTES: usize = 16 * 1024;
const MAX_SECRET_TOTAL_BYTES: usize = 64 * 1024;
const IDEMPOTENCY_TTL_MS: i64 = 24 * 60 * 60 * 1000;

type PreparedBindings = (
    Vec<BindingDescriptorV1>,
    Vec<NewDeploymentBinding>,
    Vec<String>,
);

/// Control-plane request for one immutable deployment resource binding.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeploymentBindingInput {
    /// Static product kind expected by the adapter.
    #[serde(rename = "type")]
    pub kind: BindingKind,
    /// Existing ready resource identity. Display names are never accepted.
    pub id: ResourceId,
    /// Method capability set; defaults to read/write for product compatibility.
    #[serde(default)]
    pub permissions: CanonicalPermissions,
    /// Capability-version-one product configuration.
    #[serde(default)]
    pub config: CanonicalBindingConfig,
}

#[derive(Debug, Serialize, Deserialize)]
struct FailedResponse {
    code: String,
}

/// Candidate identity passed to the real runtime validator.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidationCandidate {
    /// Account identity.
    pub account_id: AccountId,
    /// Worker identity.
    pub worker_id: WorkerId,
    /// Immutable deployment identity.
    pub deployment_id: DeploymentId,
    /// Stored descriptor digest expected before loader get.
    pub worker_code_sha256: [u8; 32],
}

/// Runtime validation boundary implemented by the workerd transport.
pub trait RuntimeValidator: Send + Sync + 'static {
    /// Parse/link/initialize the candidate without invoking tenant fetch.
    fn validate(
        &self,
        candidate: ValidationCandidate,
    ) -> Pin<Box<dyn Future<Output = Result<(), PlatformError>> + Send + '_>>;

    /// Probe a named export without invoking the tenant handler.
    fn validate_entrypoint(
        &self,
        _candidate: ValidationCandidate,
        _entrypoint: String,
    ) -> Pin<Box<dyn Future<Output = Result<(), PlatformError>> + Send + '_>> {
        Box::pin(async {
            Err(PlatformError::new(
                ErrorCode::EntrypointNotFound,
                "runtime validator cannot prove the named entrypoint",
            ))
        })
    }

    /// Prove that a candidate exports a constructible Durable Object class.
    fn validate_durable_object_class(
        &self,
        _candidate: ValidationCandidate,
        _class_name: String,
    ) -> Pin<Box<dyn Future<Output = Result<(), PlatformError>> + Send + '_>> {
        Box::pin(async {
            Err(PlatformError::new(
                ErrorCode::DoClassNotFound,
                "runtime validator cannot prove the Durable Object class",
            ))
        })
    }
}

impl<F, Fut> RuntimeValidator for F
where
    F: Fn(ValidationCandidate) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<(), PlatformError>> + Send + 'static,
{
    fn validate(
        &self,
        candidate: ValidationCandidate,
    ) -> Pin<Box<dyn Future<Output = Result<(), PlatformError>> + Send + '_>> {
        Box::pin((self)(candidate))
    }
}

/// Secret-safe deployment request. Debug redacts secret values.
#[derive(Clone, Debug)]
pub struct CreateDeploymentRequest {
    /// Account boundary.
    pub account_id: AccountId,
    /// Parent Worker.
    pub worker_id: WorkerId,
    /// Required control idempotency key.
    pub idempotency_key: String,
    /// Canonical `WorkerBundleV1` input.
    pub bundle: DeploymentBundle,
    /// Exact tenant compatibility date.
    pub compatibility_date: String,
    /// Tenant compatibility flags.
    pub compatibility_flags: Vec<String>,
    /// JSON-compatible vars.
    pub vars: BTreeMap<String, serde_json::Value>,
    /// Write-only UTF-8 secrets.
    pub secrets: BTreeMap<String, SecretString>,
    /// Immutable resource bindings keyed by tenant environment name.
    pub bindings: BTreeMap<String, DeploymentBindingInput>,
    /// Immutable limits profile.
    pub limits: serde_json::Value,
    /// Promote only after runtime validation succeeds.
    pub promote: bool,
    /// Audit request identity.
    pub request_id: RequestId,
    /// Current wall-clock milliseconds.
    pub now_ms: i64,
}

/// Canonical deployment artifact supplied in memory or as a verified staging file.
#[derive(Clone, Debug)]
pub enum DeploymentBundle {
    /// Bounded convenience input used by library callers and small tests.
    Bytes(Vec<u8>),
    /// Incrementally verified private staging file used by the HTTP upload path.
    Staged(StagedBundle),
}

impl From<Vec<u8>> for DeploymentBundle {
    fn from(value: Vec<u8>) -> Self {
        Self::Bytes(value)
    }
}

#[derive(Clone, Debug)]
enum PreparedBundle {
    Memory(CanonicalBundle),
    Staged(StagedBundle),
}

impl PreparedBundle {
    fn prepare(input: &DeploymentBundle, limits: BundleLimits) -> Result<Self, PlatformError> {
        match input {
            DeploymentBundle::Bytes(bytes) => {
                CanonicalBundle::parse(bytes.clone(), limits).map(Self::Memory)
            }
            DeploymentBundle::Staged(bundle) => Ok(Self::Staged(bundle.clone())),
        }
    }

    fn admission_bytes(&self) -> Result<u64, PlatformError> {
        match self {
            Self::Memory(_) => self.size()?.checked_add(64 * 1024).ok_or_else(invariant),
            Self::Staged(_) => Ok(64 * 1024),
        }
    }

    fn manifest(&self) -> &WorkerBundleManifest {
        match self {
            Self::Memory(bundle) => bundle.manifest(),
            Self::Staged(bundle) => bundle.manifest(),
        }
    }

    fn sha256(&self) -> [u8; 32] {
        match self {
            Self::Memory(bundle) => bundle.sha256(),
            Self::Staged(bundle) => bundle.sha256(),
        }
    }

    fn size(&self) -> Result<u64, PlatformError> {
        match self {
            Self::Memory(bundle) => u64::try_from(bundle.bytes().len()).map_err(|_| {
                PlatformError::new(ErrorCode::BundleTooLarge, "bundle size exceeds u64")
            }),
            Self::Staged(bundle) => Ok(bundle.size()),
        }
    }

    async fn store(
        &self,
        artifacts: &ArtifactStore,
    ) -> Result<open_compute_artifacts::ArtifactRef, PlatformError> {
        let digest = hex::encode(self.sha256());
        let size = self.size()?;
        match self {
            Self::Memory(bundle) => {
                let body = Bytes::copy_from_slice(bundle.bytes());
                artifacts
                    .put_verified(
                        stream::once(async move { Ok::<Bytes, std::io::Error>(body) }),
                        &digest,
                        size,
                    )
                    .await
            }
            Self::Staged(bundle) => {
                artifacts
                    .put_verified_file(bundle.path(), &digest, size)
                    .await
            }
        }
    }
}

/// Successful creation response persisted for idempotent replay.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateDeploymentResult {
    /// Created deployment.
    pub deployment: DeploymentRecord,
    /// Whether the same operation promoted it.
    pub promoted: bool,
}

/// New result or exact persisted response bytes for replay.
#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum CreateDeploymentOutcome {
    /// Pipeline ran and produced a new immutable deployment.
    Applied(CreateDeploymentResult),
    /// Same idempotency fingerprint already completed.
    Replay(Vec<u8>),
}

/// P0.2 deployment orchestrator over typed P0.1 capabilities.
pub struct DeploymentController<'a> {
    storage: &'a PlatformStorage,
    artifacts: ArtifactStore,
    validator: Arc<dyn RuntimeValidator>,
    bundle_limits: BundleLimits,
}

impl std::fmt::Debug for DeploymentController<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeploymentController")
            .field("artifacts", &self.artifacts)
            .field("bundle_limits", &self.bundle_limits)
            .finish_non_exhaustive()
    }
}

impl<'a> DeploymentController<'a> {
    /// Bind storage, immutable artifacts, and a real runtime validator.
    #[must_use]
    pub fn new(
        storage: &'a PlatformStorage,
        artifacts: ArtifactStore,
        validator: Arc<dyn RuntimeValidator>,
        bundle_limits: BundleLimits,
    ) -> Self {
        Self {
            storage,
            artifacts,
            validator,
            bundle_limits,
        }
    }

    /// Execute upload, immutable DB transaction, runtime validation, and optional promotion.
    pub async fn create_deployment(
        &self,
        request: CreateDeploymentRequest,
    ) -> Result<CreateDeploymentOutcome, PlatformError> {
        validate_idempotency_key(&request.idempotency_key)?;
        let bundle = PreparedBundle::prepare(&request.bundle, self.bundle_limits)?;
        let (canonical_vars, stored_vars) =
            canonicalize_vars(request.vars.clone(), MAX_VARS, MAX_ENV_BYTES)?;
        validate_secret_set(&request.secrets, &canonical_vars)?;
        validate_binding_set(&request.bindings, &canonical_vars, &request.secrets)?;
        validate_injection_module_collisions(bundle.manifest(), &request.bindings)?;
        let repo = WorkerRepository::new(self.storage.db());
        // Authentication/account scoping happens before reserving a key, so a
        // nonexistent target cannot strand a running idempotency row.
        repo.get_worker(request.account_id, request.worker_id)?;
        let fingerprint_input = request_fingerprint(&request, &bundle, &canonical_vars)?;
        let fingerprint = self
            .storage
            .crypto()
            .fingerprint_request(&fingerprint_input);
        match repo.reserve_idempotency(
            request.account_id,
            "deployment.create",
            &request.idempotency_key,
            self.storage.crypto().fingerprint_key_id(),
            &fingerprint,
            request.now_ms,
            request.now_ms.saturating_add(IDEMPOTENCY_TTL_MS),
        )? {
            IdempotencyReservation::Complete(response) => {
                return Ok(CreateDeploymentOutcome::Replay(response));
            }
            IdempotencyReservation::Running => {
                return Err(PlatformError::new(
                    ErrorCode::IdempotencyConflict,
                    "the same idempotent operation is still running",
                ));
            }
            IdempotencyReservation::Failed(response) => {
                let failed: FailedResponse =
                    serde_json::from_slice(&response).map_err(|_| invariant())?;
                return Err(PlatformError::new(
                    parse_failure_code(&failed.code),
                    "idempotent deployment operation previously failed",
                ));
            }
            IdempotencyReservation::Reserved => {}
        }

        let operation = self
            .create_reserved(&request, bundle, canonical_vars, stored_vars)
            .await;
        match operation {
            Ok(result) => {
                let response = serde_json::to_vec(&result).map_err(|_| invariant())?;
                repo.complete_idempotency_with_deployment_ref(
                    request.account_id,
                    "deployment.create",
                    &request.idempotency_key,
                    &fingerprint,
                    &response,
                    result.deployment.id,
                    &idempotency_ref_id(
                        request.account_id,
                        "deployment.create",
                        &request.idempotency_key,
                    ),
                    request.now_ms,
                )?;
                Ok(CreateDeploymentOutcome::Applied(result))
            }
            Err(error) => {
                let response = serde_json::to_vec(&FailedResponse {
                    code: error.code().as_str().to_owned(),
                })
                .map_err(|_| invariant())?;
                repo.fail_idempotency(
                    request.account_id,
                    "deployment.create",
                    &request.idempotency_key,
                    &fingerprint,
                    &response,
                )?;
                Err(error)
            }
        }
    }

    async fn create_reserved(
        &self,
        request: &CreateDeploymentRequest,
        bundle: PreparedBundle,
        canonical_vars: BTreeMap<String, serde_json::Value>,
        stored_vars: BTreeMap<String, Vec<u8>>,
    ) -> Result<CreateDeploymentResult, PlatformError> {
        let repo = WorkerRepository::new(self.storage.db());
        let _admission = self
            .storage
            .reserve_mutation(OperationClass::Workers, bundle.admission_bytes()?)?;
        let deployment_id = DeploymentId::generate();
        let (stored_secrets, secret_descriptors) = self.encrypt_secrets(
            request.account_id,
            request.worker_id,
            deployment_id,
            &request.secrets,
        )?;
        let (binding_descriptors, stored_bindings, durable_object_classes) =
            self.prepare_bindings(request)?;
        let descriptor = WorkerCodeDescriptorV1::new(
            request.account_id,
            request.worker_id,
            deployment_id,
            bundle.sha256(),
            bundle.manifest(),
            request.compatibility_date.clone(),
            request.compatibility_flags.clone(),
            canonical_vars,
            secret_descriptors,
            binding_descriptors,
            request.limits.clone(),
            u32::try_from(LOADER_SCHEMA_VERSION).map_err(|_| invariant())?,
        )?;
        let descriptor_hash = descriptor.sha256()?;
        let size = bundle.size()?;
        let artifact = bundle.store(&self.artifacts).await?;
        if artifact.sha256_bytes() != &bundle.sha256() || artifact.size() != size {
            return Err(PlatformError::new(
                ErrorCode::ArtifactIntegrityError,
                "ArtifactStore returned a different immutable artifact",
            ));
        }
        let mut deployment = repo.insert_staging_deployment_with_bindings_and_limit(
            &NewDeployment {
                id: deployment_id,
                account_id: request.account_id,
                worker_id: request.worker_id,
                artifact_sha256: bundle.sha256(),
                artifact_size: size,
                artifact_schema_version: WORKER_BUNDLE_SCHEMA_VERSION,
                main_module: bundle.manifest().main_module.clone(),
                compatibility_date: request.compatibility_date.clone(),
                compatibility_flags: descriptor.compatibility_flags.clone(),
                limits: request.limits.clone(),
                worker_code_sha256: descriptor_hash,
                vars: stored_vars,
                secrets: stored_secrets,
                request_id: request.request_id,
                now_ms: request.now_ms,
            },
            &stored_bindings,
            self.storage.hardening().max_deployments_per_worker,
        )?;
        repo.begin_validation(deployment_id)?;
        let candidate = ValidationCandidate {
            account_id: request.account_id,
            worker_id: request.worker_id,
            deployment_id,
            worker_code_sha256: descriptor_hash,
        };
        let validation = self.validator.validate(candidate.clone()).await;
        if let Err(err) = validation {
            let code = stable_validation_code(&err);
            repo.mark_rejected(
                deployment_id,
                DeploymentState::Validating,
                code,
                request.now_ms,
            )?;
            return Err(PlatformError::new(
                code,
                "real workerd validation rejected the deployment",
            ));
        }
        for class_name in durable_object_classes {
            if let Err(error) = self
                .validator
                .validate_durable_object_class(candidate.clone(), class_name)
                .await
            {
                let code = if error.code() == ErrorCode::DoClassNotFound {
                    ErrorCode::DoClassNotFound
                } else {
                    stable_validation_code(&error)
                };
                repo.mark_rejected(
                    deployment_id,
                    DeploymentState::Validating,
                    code,
                    request.now_ms,
                )?;
                return Err(PlatformError::new(
                    code,
                    "real workerd validation rejected a Durable Object class",
                ));
            }
        }
        repo.mark_ready(deployment_id, request.now_ms)?;
        deployment.state = DeploymentState::Ready;
        deployment.ready_at_ms = Some(request.now_ms);
        if request.promote {
            let worker = repo.get_worker(request.account_id, request.worker_id)?;
            for route in repo.list_routes(request.account_id, request.worker_id)? {
                if let Some(entrypoint) = route.entrypoint {
                    self.validator
                        .validate_entrypoint(candidate.clone(), entrypoint)
                        .await?;
                }
            }
            repo.promote_checked(
                request.account_id,
                request.worker_id,
                deployment_id,
                None,
                Some(worker.route_generation),
                request.request_id,
                request.now_ms,
            )?;
        }
        let result = CreateDeploymentResult {
            deployment,
            promoted: request.promote,
        };
        Ok(result)
    }

    fn encrypt_secrets(
        &self,
        account_id: AccountId,
        worker_id: WorkerId,
        deployment_id: DeploymentId,
        secrets: &BTreeMap<String, SecretString>,
    ) -> Result<
        (
            BTreeMap<String, StoredDeploymentSecret>,
            Vec<SecretDescriptor>,
        ),
        PlatformError,
    > {
        let mut stored = BTreeMap::new();
        let mut descriptors = Vec::with_capacity(secrets.len());
        for (name, value) in secrets {
            let revision_id = Uuid::now_v7().to_string();
            let plaintext = SecretBytes::new(value.expose().as_bytes().to_vec());
            let envelope = self.storage.crypto().encrypt_revision(
                &plaintext,
                account_id,
                worker_id,
                deployment_id,
                name,
                &revision_id,
            )?;
            descriptors.push(SecretDescriptor {
                name: name.clone(),
                revision_id: revision_id.clone(),
                ciphertext_sha256: ciphertext_sha256(&envelope.nonce, &envelope.ciphertext),
            });
            stored.insert(
                name.clone(),
                StoredDeploymentSecret {
                    name: name.clone(),
                    revision_id,
                    envelope,
                },
            );
        }
        Ok((stored, descriptors))
    }

    fn prepare_bindings(
        &self,
        request: &CreateDeploymentRequest,
    ) -> Result<PreparedBindings, PlatformError> {
        let repository = ResourceRepository::new(self.storage.db());
        let mut descriptors = Vec::with_capacity(request.bindings.len());
        let mut rows = Vec::with_capacity(request.bindings.len());
        let mut durable_object_classes = Vec::new();
        for (name, input) in &request.bindings {
            let resource = repository.get(request.account_id, input.id)?;
            if resource.state != ResourceState::Ready {
                return Err(PlatformError::new(
                    ErrorCode::ResourceNotReady,
                    "deployment binding resource is not ready",
                ));
            }
            if resource.kind != input.kind {
                return Err(PlatformError::new(
                    ErrorCode::ResourceNotFound,
                    "resource was not found in the requested scope",
                ));
            }
            if input.kind == BindingKind::DoNamespace {
                let namespace = DurableObjectRepository::new(self.storage)
                    .get_namespace(request.account_id, input.id)?;
                if namespace.owner_worker_id != request.worker_id {
                    return Err(PlatformError::new(
                        ErrorCode::DoNamespaceNotFound,
                        "Durable Object namespace is not owned by this Worker",
                    ));
                }
                durable_object_classes.push(namespace.class_name);
            }
            let descriptor = BindingDescriptorV1::new(
                BindingId::generate(),
                name.clone(),
                input.kind,
                input.id,
                resource.spec_generation,
                1,
                input.permissions,
                input.config,
            )?;
            let permissions_json =
                serde_json::to_vec(&descriptor.permissions).map_err(|_| invariant())?;
            let config_json = serde_json::to_vec(&descriptor.config).map_err(|_| invariant())?;
            rows.push(NewDeploymentBinding {
                id: descriptor.binding_id,
                name: descriptor.name.clone(),
                kind: descriptor.kind,
                resource_id: descriptor.resource_id,
                resource_spec_generation: descriptor.resource_spec_generation,
                capability_version: descriptor.capability_version,
                permissions_json,
                config_json,
                descriptor_sha256: descriptor.sha256()?,
            });
            descriptors.push(descriptor);
        }
        durable_object_classes.sort();
        durable_object_classes.dedup();
        Ok((descriptors, rows, durable_object_classes))
    }
}

pub(crate) fn idempotency_ref_id(account_id: AccountId, scope: &str, key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"open-compute/deployment-referrer/v1\0");
    hasher.update(account_id.to_string().as_bytes());
    hasher.update([0]);
    hasher.update(scope.as_bytes());
    hasher.update([0]);
    hasher.update(key.as_bytes());
    hex::encode(hasher.finalize())
}

pub(crate) fn validate_secret_set(
    secrets: &BTreeMap<String, SecretString>,
    vars: &BTreeMap<String, serde_json::Value>,
) -> Result<(), PlatformError> {
    if secrets.len() > MAX_SECRETS {
        return Err(secret_invalid("deployment contains too many secrets"));
    }
    let mut total = 0_usize;
    for (name, value) in secrets {
        validate_env_name(name)?;
        if vars.contains_key(name) {
            return Err(secret_invalid("var and secret env names conflict"));
        }
        let size = value.expose().len();
        if size == 0 || size > MAX_SECRET_BYTES {
            return Err(secret_invalid("secret value exceeds its configured size"));
        }
        total = total.checked_add(size).ok_or_else(|| {
            secret_invalid("deployment secrets exceed their configured total size")
        })?;
        if total > MAX_SECRET_TOTAL_BYTES {
            return Err(secret_invalid(
                "deployment secrets exceed their configured total size",
            ));
        }
    }
    Ok(())
}

pub(crate) fn validate_binding_set(
    bindings: &BTreeMap<String, DeploymentBindingInput>,
    vars: &BTreeMap<String, serde_json::Value>,
    secrets: &BTreeMap<String, SecretString>,
) -> Result<(), PlatformError> {
    if bindings.len() > MAX_VARS {
        return Err(PlatformError::new(
            ErrorCode::ResourceLimitExceeded,
            "deployment contains too many bindings",
        ));
    }
    for name in bindings.keys() {
        validate_env_name(name)?;
        if name.len() > 64 || vars.contains_key(name) || secrets.contains_key(name) {
            return Err(PlatformError::new(
                ErrorCode::BindingTypeMismatch,
                "binding env name is invalid or conflicts with var or secret",
            ));
        }
    }
    Ok(())
}

pub(crate) fn validate_injection_module_collisions(
    manifest: &WorkerBundleManifest,
    bindings: &BTreeMap<String, DeploymentBindingInput>,
) -> Result<(), PlatformError> {
    if !bindings.values().any(|binding| {
        matches!(
            binding.kind,
            BindingKind::R2Bucket | BindingKind::D1Database | BindingKind::DoNamespace
        )
    }) {
        return Ok(());
    }
    if manifest.modules.iter().any(|module| {
        matches!(
            module.name.as_str(),
            R2_FACADE_MODULE_NAME
                | D1_FACADE_MODULE_NAME
                | DO_FACADE_MODULE_NAME
                | DO_ID_CODEC_MODULE_NAME
                | DO_ALARM_SHIM_MODULE_NAME
                | LOADED_ISOLATE_WRAPPER_MODULE_NAME
        )
    }) {
        return Err(PlatformError::new(
            ErrorCode::BundleInvalid,
            "tenant bundle collides with a reserved loaded-isolate module",
        ));
    }
    Ok(())
}

fn request_fingerprint(
    request: &CreateDeploymentRequest,
    bundle: &PreparedBundle,
    vars: &BTreeMap<String, serde_json::Value>,
) -> Result<[u8; 32], PlatformError> {
    let mut canonical = Vec::new();
    frame(&mut canonical, request.account_id.to_string().as_bytes())?;
    frame(&mut canonical, request.worker_id.to_string().as_bytes())?;
    frame(&mut canonical, &bundle.sha256())?;
    frame(&mut canonical, request.compatibility_date.as_bytes())?;
    let mut flags = request.compatibility_flags.clone();
    flags.sort();
    flags.dedup();
    frame(
        &mut canonical,
        &serde_json::to_vec(&flags).map_err(|_| invariant())?,
    )?;
    frame(
        &mut canonical,
        &serde_json::to_vec(vars).map_err(|_| invariant())?,
    )?;
    for (name, value) in &request.secrets {
        frame(&mut canonical, name.as_bytes())?;
        frame(&mut canonical, value.expose().as_bytes())?;
    }
    frame(
        &mut canonical,
        &serde_json::to_vec(&request.bindings).map_err(|_| invariant())?,
    )?;
    frame(
        &mut canonical,
        &serde_json::to_vec(&request.limits).map_err(|_| invariant())?,
    )?;
    canonical.push(u8::from(request.promote));
    let mut domain = Sha256::new();
    domain.update(b"open-compute/deployment-request/v1");
    domain.update(request.account_id.as_uuid().as_bytes());
    domain.update(&canonical);
    let digest: [u8; 32] = domain.finalize().into();
    // This unkeyed digest is only an input to the master-key-derived HMAC.
    canonical.zeroize();
    Ok(digest)
}

fn frame(out: &mut Vec<u8>, value: &[u8]) -> Result<(), PlatformError> {
    let len = u64::try_from(value.len()).map_err(|_| invariant())?;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(value);
    Ok(())
}

pub(crate) fn validate_idempotency_key(key: &str) -> Result<(), PlatformError> {
    if key.is_empty()
        || key.len() > 128
        || key
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte == b' ')
    {
        return Err(PlatformError::new(
            ErrorCode::IdempotencyConflict,
            "idempotency key is invalid",
        ));
    }
    Ok(())
}

pub(crate) fn stable_validation_code(error: &PlatformError) -> ErrorCode {
    match error.code() {
        ErrorCode::RuntimeUnavailable | ErrorCode::RuntimeResultUnknown => error.code(),
        ErrorCode::ResourceLimitExceeded => ErrorCode::ResourceLimitExceeded,
        _ => ErrorCode::BundleRuntimeInvalid,
    }
}

pub(crate) fn parse_failure_code(code: &str) -> ErrorCode {
    match code {
        "ACCOUNT_NOT_FOUND" => ErrorCode::AccountNotFound,
        "WORKER_NOT_FOUND" => ErrorCode::WorkerNotFound,
        "WORKER_DELETED" => ErrorCode::WorkerDeleted,
        "DEPLOYMENT_NOT_FOUND" => ErrorCode::DeploymentNotFound,
        "DEPLOYMENT_NOT_READY" => ErrorCode::DeploymentNotReady,
        "DEPLOYMENT_INVARIANT_VIOLATION" => ErrorCode::DeploymentInvariantViolation,
        "BUNDLE_INVALID" => ErrorCode::BundleInvalid,
        "BUNDLE_TOO_LARGE" => ErrorCode::BundleTooLarge,
        "BUNDLE_RUNTIME_INVALID" => ErrorCode::BundleRuntimeInvalid,
        "COMPATIBILITY_UNSUPPORTED" => ErrorCode::CompatibilityUnsupported,
        "ARTIFACT_UNAVAILABLE" => ErrorCode::ArtifactUnavailable,
        "ARTIFACT_INTEGRITY_ERROR" => ErrorCode::ArtifactIntegrityError,
        "SECRET_INVALID" => ErrorCode::SecretInvalid,
        "RESOURCE_LIMIT_EXCEEDED" => ErrorCode::ResourceLimitExceeded,
        "RESOURCE_NOT_FOUND" => ErrorCode::ResourceNotFound,
        "RESOURCE_NAME_CONFLICT" => ErrorCode::ResourceNameConflict,
        "RESOURCE_NOT_READY" => ErrorCode::ResourceNotReady,
        "RESOURCE_REFERENCED" => ErrorCode::ResourceReferenced,
        "RESOURCE_UNAVAILABLE" => ErrorCode::ResourceUnavailable,
        "RESOURCE_INVARIANT_VIOLATION" => ErrorCode::ResourceInvariantViolation,
        "BINDING_NOT_FOUND" => ErrorCode::BindingNotFound,
        "BINDING_TYPE_MISMATCH" => ErrorCode::BindingTypeMismatch,
        "BINDING_PERMISSION_DENIED" => ErrorCode::BindingPermissionDenied,
        "BINDING_CAPABILITY_UNSUPPORTED" => ErrorCode::BindingCapabilityUnsupported,
        "BINDING_PROTOCOL_ERROR" => ErrorCode::BindingProtocolError,
        "BINDING_LIMIT_EXCEEDED" => ErrorCode::BindingLimitExceeded,
        "BINDING_RESULT_UNKNOWN" => ErrorCode::BindingResultUnknown,
        "RUNTIME_UNAVAILABLE" => ErrorCode::RuntimeUnavailable,
        "RUNTIME_RESULT_UNKNOWN" => ErrorCode::RuntimeResultUnknown,
        _ => ErrorCode::Internal,
    }
}

fn secret_invalid(message: &'static str) -> PlatformError {
    PlatformError::new(ErrorCode::SecretInvalid, message)
}

fn invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::DeploymentInvariantViolation,
        "deployment descriptor invariant failed",
    )
}

#[cfg(test)]
#[path = "pipeline_tests.rs"]
mod tests;
