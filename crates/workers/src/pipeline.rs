//! Immutable deployment creation pipeline.

#[path = "pipeline/bindings.rs"]
mod binding_preparation;
#[path = "pipeline/products.rs"]
mod products;
#[path = "pipeline/validation.rs"]
mod validation;
use binding_preparation::PreparedBindings;
use validation::{invariant, request_fingerprint};
pub(crate) use validation::{
    stable_validation_code, validate_binding_set, validate_idempotency_key,
    validate_injection_module_collisions, validate_secret_set, validate_service_set,
};

use products::{prepare_cron_config, validate_product_counts};

use crate::assets::{DeploymentAssets, RunWorkerFirst};
use crate::bundle::{
    BundleLimits, CanonicalBundle, StagedBundle, WORKER_BUNDLE_SCHEMA_VERSION, WorkerBundleManifest,
};
use crate::descriptor::{
    BindingDescriptorV1, BuiltinBindingDescriptorKindV1, BuiltinBindingDescriptorV1,
    CacheEntrypointPolicyV1, CachePolicyDescriptorV1, QueueProducerBindingDescriptorV1,
    SYSTEM_MODULE_PREFIX, SecretDescriptor, ServiceDescriptorV1, WorkerCodeDescriptorV1,
    canonicalize_vars, ciphertext_sha256, validate_env_name,
};
use bytes::Bytes;
use futures::stream;
use open_compute_artifacts::ArtifactStore;
use open_compute_core::{
    AccountId, BindingId, BindingKind, CanonicalBindingConfig, CanonicalPermissions,
    CronActivationId, CronSchedule, DeploymentId, ErrorCode, PlatformError, QueueConsumerId,
    QueueId, RequestId, ResourceId, ResourceState, SecretBytes, SecretString, WorkerId,
};
use open_compute_storage::{
    BindingRepository, BuiltinBindingKind, CRON_PARSER_VERSION, DeploymentBuiltinBindingRecord,
    DeploymentCachePolicyRecord, DeploymentContentKind, DeploymentObjectKind, DeploymentRecord,
    DeploymentState, DurableObjectRepository, IdempotencyReservation, LOADER_SCHEMA_VERSION,
    NewCronConfig, NewCronDeclaration, NewDeployment, NewDeploymentAssets, NewDeploymentBinding,
    NewDeploymentObjectRef, NewDeploymentService, NewQueueConsumerDeclaration,
    NewQueueProducerBinding, PlatformStorage, QueueAvailability, QueueConsumerConfig,
    QueueConsumerRepository, QueueRepository, QueueState, ResourceRepository,
    StoredDeploymentSecret, WorkerRepository,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
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
const DEFAULT_MAX_QUEUE_CONSUMER_CONCURRENCY: u32 = 32;
const MAX_QUEUE_CONSUMERS_PER_DEPLOYMENT: usize = 64;
const MAX_CRONS_PER_DEPLOYMENT: usize = 100;

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

/// Control-plane declaration for one dynamic same-account Service binding.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeploymentServiceInput {
    /// Existing logical target Worker identity; names are resolved by tooling before deploy.
    pub target_worker_id: WorkerId,
    /// Optional named `WorkerEntrypoint` export.
    #[serde(default)]
    pub entrypoint: Option<String>,
}

/// Automatic response-cache policy on the default or a named Worker entrypoint.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeploymentCachePolicyInput {
    /// Whether automatic response caching is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Whether automatic entries are shared across deployment versions.
    #[serde(default)]
    pub cross_version_cache: bool,
}

/// Deployment-wide automatic-cache configuration and named-entrypoint overrides.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeploymentCacheInput {
    /// Default export policy.
    #[serde(flatten)]
    pub default: DeploymentCachePolicyInput,
    /// Named Worker entrypoint policy overrides.
    #[serde(default)]
    pub entrypoints: BTreeMap<String, DeploymentCachePolicyInput>,
}

/// One platform-provided Images binding declaration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeploymentImagesInput {
    /// Tenant environment binding name.
    pub binding: String,
}

/// One standard Workers AI binding declaration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeploymentAiInput {
    /// Tenant environment binding name.
    pub binding: String,
}

/// One immutable deployment Version Metadata binding declaration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeploymentVersionMetadataInput {
    /// Tenant environment binding name.
    pub binding: String,
    /// Optional application-supplied immutable release tag.
    #[serde(default)]
    pub tag: Option<String>,
}

/// Platform-provided runtime capabilities frozen with one deployment.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeploymentRuntimeFeatures {
    /// Automatic response-cache policy.
    #[serde(default)]
    pub cache: DeploymentCacheInput,
    /// Optional Workers AI binding exposing the Markdown Conversion subset.
    #[serde(default)]
    pub ai: Option<DeploymentAiInput>,
    /// Optional local Images binding.
    #[serde(default)]
    pub images: Option<DeploymentImagesInput>,
    /// Optional frozen Version Metadata binding.
    #[serde(default)]
    pub version_metadata: Option<DeploymentVersionMetadataInput>,
}

/// Immutable Queue push-consumer declaration supplied with a deployment.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QueueConsumerInput {
    /// Existing ready source Queue identity.
    pub queue: QueueId,
    /// Optional named `WorkerEntrypoint` export.
    #[serde(default)]
    pub entrypoint: Option<String>,
    /// Delivery and retry policy.
    #[serde(flatten)]
    pub config: QueueConsumerConfig,
    /// Optional ready dead-letter Queue in the same account.
    #[serde(default)]
    pub dead_letter_queue: Option<QueueId>,
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

/// Cross-database product handoff invoked after validation and before active routing changes.
pub trait ProductPromotionCoordinator: Send + Sync + 'static {
    /// Stage, drain, promote, and activate Queue/Cron targets without overlapping generations.
    fn promote(
        &self,
        request: ProductPromotionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<(), PlatformError>> + Send + '_>>;
}

/// Immutable authority needed by the Queue/Cron promotion coordinator.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProductPromotionRequest {
    /// Owning account.
    pub account_id: AccountId,
    /// Worker whose active deployment changes.
    pub worker_id: WorkerId,
    /// Validated ready target deployment.
    pub deployment_id: DeploymentId,
    /// Audit request identity.
    pub request_id: RequestId,
    /// Control-plane wall time.
    pub now_ms: i64,
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
    /// Explicit Worker/Assets deployment content union.
    pub content: DeploymentContent,
    /// JSON-compatible vars.
    pub vars: BTreeMap<String, serde_json::Value>,
    /// Write-only UTF-8 secrets.
    pub secrets: BTreeMap<String, SecretString>,
    /// Immutable resource bindings keyed by tenant environment name.
    pub bindings: BTreeMap<String, DeploymentBindingInput>,
    /// Immutable Service declarations keyed by tenant environment name.
    pub services: BTreeMap<String, DeploymentServiceInput>,
    /// Platform-provided runtime capabilities.
    pub runtime_features: DeploymentRuntimeFeatures,
    /// Immutable Queue push-consumer declarations.
    pub queue_consumers: Vec<QueueConsumerInput>,
    /// Exact Cron set for the Worker's scheduled handler.
    pub crons: Vec<String>,
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

/// Authoritative deployment content; assets-only never fabricates a Worker bundle.
#[derive(Clone, Debug)]
pub enum DeploymentContent {
    /// Executable Worker with optional static assets.
    Worker {
        /// Canonical Worker bundle.
        bundle: DeploymentBundle,
        /// Optional static assets frozen with the code.
        assets: Option<DeploymentAssets>,
    },
    /// Static assets without executable tenant code.
    AssetsOnly {
        /// Required immutable static assets.
        assets: DeploymentAssets,
    },
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

#[derive(Clone, Debug)]
enum PreparedContent {
    Worker {
        bundle: PreparedBundle,
        assets: Option<DeploymentAssets>,
    },
    AssetsOnly {
        assets: DeploymentAssets,
    },
}

impl PreparedContent {
    fn prepare(input: &DeploymentContent, limits: BundleLimits) -> Result<Self, PlatformError> {
        match input {
            DeploymentContent::Worker { bundle, assets } => Ok(Self::Worker {
                bundle: PreparedBundle::prepare(bundle, limits)?,
                assets: assets.clone(),
            }),
            DeploymentContent::AssetsOnly { assets } => Ok(Self::AssetsOnly {
                assets: assets.clone(),
            }),
        }
    }

    const fn kind(&self) -> DeploymentContentKind {
        match self {
            Self::Worker { .. } => DeploymentContentKind::Worker,
            Self::AssetsOnly { .. } => DeploymentContentKind::AssetsOnly,
        }
    }

    const fn bundle(&self) -> Option<&PreparedBundle> {
        match self {
            Self::Worker { bundle, .. } => Some(bundle),
            Self::AssetsOnly { .. } => None,
        }
    }

    const fn assets(&self) -> Option<&DeploymentAssets> {
        match self {
            Self::Worker { assets, .. } => assets.as_ref(),
            Self::AssetsOnly { assets } => Some(assets),
        }
    }

    fn admission_bytes(&self) -> Result<u64, PlatformError> {
        let manifest_size = self
            .assets()
            .map(|assets| assets.manifest.canonical_bytes())
            .transpose()?
            .map_or(0, |bytes| bytes.len() as u64);
        self.bundle()
            .map(PreparedBundle::admission_bytes)
            .transpose()?
            .unwrap_or(64 * 1024)
            .checked_add(manifest_size)
            .ok_or_else(invariant)
    }
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
    max_queue_consumer_concurrency: u32,
    product_promoter: Option<Arc<dyn ProductPromotionCoordinator>>,
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
            max_queue_consumer_concurrency: DEFAULT_MAX_QUEUE_CONSUMER_CONCURRENCY,
            product_promoter: None,
        }
    }

    /// Apply the validated operator-local Queue consumer concurrency ceiling.
    #[must_use]
    pub fn with_queue_consumer_limit(mut self, maximum: u32) -> Self {
        self.max_queue_consumer_concurrency = maximum.max(1);
        self
    }

    /// Attach the single-process Queue/Cron cross-database promotion owner.
    #[must_use]
    pub fn with_product_promoter(mut self, promoter: Arc<dyn ProductPromotionCoordinator>) -> Self {
        self.product_promoter = Some(promoter);
        self
    }

    /// Execute upload, immutable DB transaction, runtime validation, and optional promotion.
    pub async fn create_deployment(
        &self,
        request: CreateDeploymentRequest,
    ) -> Result<CreateDeploymentOutcome, PlatformError> {
        self.create_deployment_with_id(request, None).await
    }

    /// Finalize a resumable upload using the deployment identity persisted before validation.
    pub async fn finalize_upload(
        &self,
        request: CreateDeploymentRequest,
        deployment_id: DeploymentId,
    ) -> Result<CreateDeploymentOutcome, PlatformError> {
        self.create_deployment_with_id(request, Some(deployment_id))
            .await
    }

    async fn create_deployment_with_id(
        &self,
        request: CreateDeploymentRequest,
        deployment_id: Option<DeploymentId>,
    ) -> Result<CreateDeploymentOutcome, PlatformError> {
        validate_idempotency_key(&request.idempotency_key)?;
        let content = PreparedContent::prepare(&request.content, self.bundle_limits)?;
        let (canonical_vars, stored_vars) =
            canonicalize_vars(request.vars.clone(), MAX_VARS, MAX_ENV_BYTES)?;
        validate_secret_set(&request.secrets, &canonical_vars)?;
        validate_binding_set(&request.bindings, &canonical_vars, &request.secrets)?;
        validate_service_set(
            &request.services,
            &canonical_vars,
            &request.secrets,
            &request.bindings,
        )?;
        if let Some(bundle) = content.bundle() {
            validate_injection_module_collisions(bundle.manifest())?;
        }
        validate_asset_content(&request, &content, &canonical_vars)?;
        validate_product_counts(&request)?;
        let repo = WorkerRepository::new(self.storage.db());
        // Authentication/account scoping happens before reserving a key, so a
        // nonexistent target cannot strand a running idempotency row.
        repo.get_worker(request.account_id, request.worker_id)?;
        let fingerprint_input =
            request_fingerprint(&request, &content, &canonical_vars, deployment_id)?;
        let fingerprint = self
            .storage
            .crypto()
            .fingerprint_request(&fingerprint_input);
        let reservation = repo.reserve_idempotency(
            request.account_id,
            "deployment.create",
            &request.idempotency_key,
            self.storage.crypto().fingerprint_key_id(),
            &fingerprint,
            request.now_ms,
            request.now_ms.saturating_add(IDEMPOTENCY_TTL_MS),
        )?;
        let recover_running = matches!(reservation, IdempotencyReservation::Running);
        match reservation {
            IdempotencyReservation::Complete(response) => {
                return Ok(CreateDeploymentOutcome::Replay(response));
            }
            IdempotencyReservation::Running if deployment_id.is_none() => {
                return Err(PlatformError::new(
                    ErrorCode::IdempotencyConflict,
                    "the same idempotent operation is still running",
                ));
            }
            IdempotencyReservation::Failed(response) => {
                let failed: FailedResponse =
                    serde_json::from_slice(&response).map_err(|_| invariant())?;
                return Err(PlatformError::new(
                    ErrorCode::from_stable_str(&failed.code).unwrap_or(ErrorCode::Internal),
                    "idempotent deployment operation previously failed",
                ));
            }
            IdempotencyReservation::Running | IdempotencyReservation::Reserved => {}
        }

        let fixed_deployment_id = deployment_id.unwrap_or_else(DeploymentId::generate);
        let operation = if recover_running {
            self.resume_reserved(
                &request,
                content,
                canonical_vars,
                stored_vars,
                fixed_deployment_id,
            )
            .await
        } else {
            self.create_reserved(
                &request,
                content,
                canonical_vars,
                stored_vars,
                fixed_deployment_id,
            )
            .await
        };
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
        content: PreparedContent,
        canonical_vars: BTreeMap<String, serde_json::Value>,
        stored_vars: BTreeMap<String, Vec<u8>>,
        deployment_id: DeploymentId,
    ) -> Result<CreateDeploymentResult, PlatformError> {
        let repo = WorkerRepository::new(self.storage.db());
        let _admission = self.storage.reserve_mutation(content.admission_bytes()?)?;
        let (stored_secrets, secret_descriptors) = self.encrypt_secrets(
            request.account_id,
            request.worker_id,
            deployment_id,
            &request.secrets,
        )?;
        let PreparedBindings {
            descriptors: binding_descriptors,
            rows: stored_bindings,
            queue_descriptors: queue_binding_descriptors,
            queue_rows: stored_queue_bindings,
            workflow_descriptors: workflow_binding_descriptors,
            workflow_rows: stored_workflow_bindings,
            durable_object_classes,
            service_descriptors,
            service_rows,
        } = self.prepare_bindings(request, deployment_id)?;
        let queue_consumers = self.prepare_queue_consumers(request)?;
        let cron = prepare_cron_config(request, &workflow_binding_descriptors)?;
        let (cache_policy, mut cache_rows, builtin_descriptors, builtin_rows) =
            prepare_runtime_features(&request.runtime_features)?;
        let descriptor = WorkerCodeDescriptorV1::new(
            request.account_id,
            request.worker_id,
            deployment_id,
            request.now_ms,
            content
                .bundle()
                .map(|bundle| (bundle.sha256(), bundle.manifest())),
            content
                .assets()
                .map(|assets| (&assets.manifest, &assets.routing)),
            canonical_vars,
            secret_descriptors,
            binding_descriptors,
            queue_binding_descriptors,
            workflow_binding_descriptors,
            service_descriptors,
            cache_policy,
            builtin_descriptors,
            u32::try_from(LOADER_SCHEMA_VERSION).map_err(|_| invariant())?,
        )?;
        if content.kind() == DeploymentContentKind::AssetsOnly {
            cache_rows.clear();
        }
        let descriptor_hash = descriptor.sha256()?;
        let artifact_reservation = self.artifacts.reserve_deployment_artifact().await;
        let bundle_identity = if let Some(bundle) = content.bundle() {
            let size = bundle.size()?;
            let artifact = bundle.store(&self.artifacts).await?;
            if artifact.sha256_bytes() != &bundle.sha256() || artifact.size() != size {
                return Err(PlatformError::new(
                    ErrorCode::ArtifactIntegrityError,
                    "ArtifactStore returned a different immutable artifact",
                ));
            }
            Some((bundle.sha256(), size, bundle.manifest().main_module.clone()))
        } else {
            None
        };
        let prepared_assets = self.prepare_assets(content.assets()).await?;
        let deployment = repo.insert_staging_deployment(
            &NewDeployment {
                id: deployment_id,
                account_id: request.account_id,
                worker_id: request.worker_id,
                content_kind: content.kind(),
                artifact_sha256: bundle_identity.as_ref().map(|value| value.0),
                artifact_size: bundle_identity.as_ref().map(|value| value.1),
                artifact_schema_version: bundle_identity
                    .as_ref()
                    .map(|_| WORKER_BUNDLE_SCHEMA_VERSION),
                main_module: bundle_identity.as_ref().map(|value| value.2.clone()),
                worker_code_sha256: descriptor_hash,
                vars: stored_vars,
                secrets: stored_secrets,
                request_id: request.request_id,
                now_ms: request.now_ms,
            },
            &open_compute_storage::NewDeploymentProducts {
                assets: prepared_assets.as_ref().map(|value| &value.0),
                asset_object_refs: prepared_assets
                    .as_ref()
                    .map_or(&[], |value| value.1.as_slice()),
                bindings: &stored_bindings,
                queue_bindings: &stored_queue_bindings,
                workflow_bindings: &stored_workflow_bindings,
                services: &service_rows,
                cache_policies: &cache_rows,
                builtin_bindings: &builtin_rows,
                queue_consumers: &queue_consumers,
                cron: (content.kind() == DeploymentContentKind::Worker).then_some(&cron),
            },
            self.storage.hardening().max_deployments_per_worker,
        )?;
        drop(artifact_reservation);
        let requires_product_promoter =
            !queue_consumers.is_empty() || !cron.declarations.is_empty();
        let queue_entrypoints: Vec<Option<String>> = queue_consumers
            .iter()
            .map(|consumer| consumer.entrypoint.clone())
            .collect();
        let cache_entrypoints = request
            .runtime_features
            .cache
            .entrypoints
            .iter()
            .filter(|(_, policy)| policy.enabled)
            .map(|(name, _)| Some(name.clone()));
        let queue_entrypoints = queue_entrypoints
            .into_iter()
            .chain(cache_entrypoints)
            .collect();
        self.finish_reserved(
            request,
            deployment,
            durable_object_classes,
            queue_entrypoints,
            requires_product_promoter,
        )
        .await
    }

    async fn resume_reserved(
        &self,
        request: &CreateDeploymentRequest,
        content: PreparedContent,
        canonical_vars: BTreeMap<String, serde_json::Value>,
        stored_vars: BTreeMap<String, Vec<u8>>,
        deployment_id: DeploymentId,
    ) -> Result<CreateDeploymentResult, PlatformError> {
        let repo = WorkerRepository::new(self.storage.db());
        let deployment =
            match repo.get_deployment(request.account_id, request.worker_id, deployment_id) {
                Ok(deployment) => deployment,
                Err(error) if error.code() == ErrorCode::DeploymentNotFound => {
                    return self
                        .create_reserved(
                            request,
                            content,
                            canonical_vars,
                            stored_vars,
                            deployment_id,
                        )
                        .await;
                }
                Err(error) => return Err(error),
            };
        if deployment.content_kind != content.kind() || deployment.deleted_at_ms.is_some() {
            return Err(invariant());
        }
        let mut durable_object_classes = Vec::new();
        for binding in
            BindingRepository::new(self.storage.db()).deployment_bindings(deployment_id)?
        {
            if binding.kind == BindingKind::DoNamespace {
                let namespace = DurableObjectRepository::new(self.storage)
                    .get_namespace(request.account_id, binding.resource_id)?;
                durable_object_classes.push(namespace.class_name);
            }
        }
        durable_object_classes.sort();
        durable_object_classes.dedup();
        let queue_declarations = QueueConsumerRepository::new(self.storage.db())
            .deployment_declarations(deployment_id)?;
        let cron_declarations = open_compute_storage::CronRepository::new(self.storage.db())
            .deployment_config(deployment_id)?
            .declarations;
        let requires_product_promoter =
            !queue_declarations.is_empty() || !cron_declarations.is_empty();
        let mut queue_entrypoints = queue_declarations
            .into_iter()
            .map(|consumer| consumer.entrypoint)
            .collect::<Vec<_>>();
        let (cache_policies, _) =
            open_compute_storage::deployment_runtime_features(self.storage.db(), deployment_id)?;
        queue_entrypoints.extend(
            cache_policies
                .into_iter()
                .filter(|policy| policy.enabled && policy.entrypoint.is_some())
                .map(|policy| policy.entrypoint),
        );
        self.finish_reserved(
            request,
            deployment,
            durable_object_classes,
            queue_entrypoints,
            requires_product_promoter,
        )
        .await
    }

    async fn finish_reserved(
        &self,
        request: &CreateDeploymentRequest,
        mut deployment: DeploymentRecord,
        durable_object_classes: Vec<String>,
        queue_entrypoints: Vec<Option<String>>,
        requires_product_promoter: bool,
    ) -> Result<CreateDeploymentResult, PlatformError> {
        let repo = WorkerRepository::new(self.storage.db());
        if deployment.state == DeploymentState::Rejected {
            let code = deployment
                .rejection_code
                .as_deref()
                .and_then(ErrorCode::from_stable_str)
                .unwrap_or(ErrorCode::BundleRuntimeInvalid);
            return Err(PlatformError::new(
                code,
                "deployment validation previously failed",
            ));
        }
        if deployment.state == DeploymentState::Staging {
            repo.begin_validation(deployment.id)?;
            deployment.state = DeploymentState::Validating;
        }
        let candidate = ValidationCandidate {
            account_id: request.account_id,
            worker_id: request.worker_id,
            deployment_id: deployment.id,
            worker_code_sha256: deployment.worker_code_sha256,
        };
        let validation = if deployment.state == DeploymentState::Validating
            && deployment.content_kind == DeploymentContentKind::Worker
        {
            self.validator.validate(candidate.clone()).await
        } else {
            Ok(())
        };
        if let Err(err) = validation {
            let code = stable_validation_code(&err);
            repo.mark_rejected(
                deployment.id,
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
            if deployment.state != DeploymentState::Validating {
                break;
            }
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
                    deployment.id,
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
        for entrypoint in &queue_entrypoints {
            if deployment.state == DeploymentState::Validating
                && let Some(entrypoint) = entrypoint
                && let Err(error) = self
                    .validator
                    .validate_entrypoint(candidate.clone(), entrypoint.clone())
                    .await
            {
                let code = stable_validation_code(&error);
                repo.mark_rejected(
                    deployment.id,
                    DeploymentState::Validating,
                    code,
                    request.now_ms,
                )?;
                return Err(PlatformError::new(
                    code,
                    "real workerd validation rejected a named entrypoint",
                ));
            }
        }
        if deployment.state == DeploymentState::Validating {
            repo.mark_ready(deployment.id, request.now_ms)?;
            deployment.state = DeploymentState::Ready;
            deployment.ready_at_ms = Some(request.now_ms);
        }
        if request.promote {
            let worker = repo.get_worker(request.account_id, request.worker_id)?;
            if worker.active_deployment_id == Some(deployment.id) {
                return Ok(CreateDeploymentResult {
                    deployment,
                    promoted: true,
                });
            }
            for route in repo.list_routes(request.account_id, request.worker_id)? {
                if let Some(entrypoint) = route.entrypoint {
                    self.validator
                        .validate_entrypoint(candidate.clone(), entrypoint)
                        .await?;
                }
            }
            if let Some(promoter) = &self.product_promoter {
                promoter
                    .promote(ProductPromotionRequest {
                        account_id: request.account_id,
                        worker_id: request.worker_id,
                        deployment_id: deployment.id,
                        request_id: request.request_id,
                        now_ms: request.now_ms,
                    })
                    .await?;
            } else if requires_product_promoter {
                return Err(PlatformError::new(
                    ErrorCode::QueueConsumerProjectionPending,
                    "Queue/Cron promotion coordinator is unavailable",
                ));
            } else {
                repo.promote_checked(
                    request.account_id,
                    request.worker_id,
                    deployment.id,
                    None,
                    Some(worker.route_generation),
                    request.request_id,
                    request.now_ms,
                )?;
            }
        }
        let result = CreateDeploymentResult {
            deployment,
            promoted: request.promote,
        };
        Ok(result)
    }

    async fn prepare_assets(
        &self,
        assets: Option<&DeploymentAssets>,
    ) -> Result<Option<(NewDeploymentAssets, Vec<NewDeploymentObjectRef>)>, PlatformError> {
        let Some(assets) = assets else {
            return Ok(None);
        };
        assets.manifest.validate()?;
        assets.routing.validate()?;
        let manifest_bytes = assets.manifest.canonical_bytes()?;
        let manifest_digest: [u8; 32] = Sha256::digest(&manifest_bytes).into();
        let manifest_ref = self
            .artifacts
            .put_verified(
                stream::once(async {
                    Ok::<Bytes, std::io::Error>(Bytes::from(manifest_bytes.clone()))
                }),
                &hex::encode(manifest_digest),
                manifest_bytes.len() as u64,
            )
            .await
            .map_err(|error| map_asset_store_error(&error))?;
        if manifest_ref.sha256_bytes() != &manifest_digest {
            return Err(PlatformError::new(
                ErrorCode::AssetIntegrityError,
                "asset manifest identity changed during upload",
            ));
        }
        let mut refs = vec![NewDeploymentObjectRef {
            kind: DeploymentObjectKind::AssetManifest,
            sha256: manifest_digest,
            size: manifest_bytes.len() as u64,
        }];
        let mut seen = BTreeMap::<[u8; 32], u64>::new();
        for entry in &assets.manifest.entries {
            let object = entry.artifact_ref()?;
            if let Some(size) = seen.insert(*object.sha256_bytes(), object.size()) {
                if size != object.size() {
                    return Err(PlatformError::new(
                        ErrorCode::AssetManifestInvalid,
                        "one asset digest declares conflicting lengths",
                    ));
                }
                continue;
            }
            self.artifacts
                .download_verified(&object, &mut std::io::sink())
                .await
                .map_err(|error| map_asset_store_error(&error))?;
            refs.push(NewDeploymentObjectRef {
                kind: DeploymentObjectKind::AssetBlob,
                sha256: *object.sha256_bytes(),
                size: object.size(),
            });
        }
        Ok(Some((
            NewDeploymentAssets {
                manifest_sha256: manifest_digest,
                manifest_json: manifest_bytes,
                routing_config_json: assets.routing.canonical_bytes()?,
                binding_name: assets.routing.binding.clone(),
                logical_file_count: u32::try_from(assets.manifest.entries.len())
                    .map_err(|_| invariant())?,
                logical_total_bytes: assets.manifest.total_bytes()?,
            },
            refs,
        )))
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
            let envelope = self.storage.crypto().encrypt(
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
}

#[allow(clippy::type_complexity)]
fn prepare_runtime_features(
    input: &DeploymentRuntimeFeatures,
) -> Result<
    (
        CachePolicyDescriptorV1,
        Vec<DeploymentCachePolicyRecord>,
        Vec<BuiltinBindingDescriptorV1>,
        Vec<DeploymentBuiltinBindingRecord>,
    ),
    PlatformError,
> {
    let cache_policy = CachePolicyDescriptorV1 {
        enabled: input.cache.default.enabled,
        cross_version_cache: input.cache.default.cross_version_cache,
        entrypoints: input
            .cache
            .entrypoints
            .iter()
            .map(|(name, policy)| {
                (
                    name.clone(),
                    CacheEntrypointPolicyV1 {
                        enabled: policy.enabled,
                        cross_version_cache: policy.cross_version_cache,
                    },
                )
            })
            .collect(),
    };
    cache_policy.validate()?;
    let mut cache_rows = vec![DeploymentCachePolicyRecord {
        entrypoint: None,
        enabled: cache_policy.enabled,
        cross_version_cache: cache_policy.cross_version_cache,
    }];
    cache_rows.extend(cache_policy.entrypoints.iter().map(|(name, policy)| {
        DeploymentCachePolicyRecord {
            entrypoint: Some(name.clone()),
            enabled: policy.enabled,
            cross_version_cache: policy.cross_version_cache,
        }
    }));
    let mut descriptors = Vec::new();
    if let Some(ai) = &input.ai {
        descriptors.push(BuiltinBindingDescriptorV1::new(
            ai.binding.clone(),
            BuiltinBindingDescriptorKindV1::Ai,
            None,
        )?);
    }
    if let Some(images) = &input.images {
        descriptors.push(BuiltinBindingDescriptorV1::new(
            images.binding.clone(),
            BuiltinBindingDescriptorKindV1::Images,
            None,
        )?);
    }
    if let Some(metadata) = &input.version_metadata {
        descriptors.push(BuiltinBindingDescriptorV1::new(
            metadata.binding.clone(),
            BuiltinBindingDescriptorKindV1::VersionMetadata,
            metadata.tag.clone(),
        )?);
    }
    descriptors.sort_by(|a, b| a.name.as_bytes().cmp(b.name.as_bytes()));
    let rows = descriptors
        .iter()
        .map(|descriptor| {
            Ok(DeploymentBuiltinBindingRecord {
                name: descriptor.name.clone(),
                kind: match descriptor.kind {
                    BuiltinBindingDescriptorKindV1::Ai => BuiltinBindingKind::Ai,
                    BuiltinBindingDescriptorKindV1::Images => BuiltinBindingKind::Images,
                    BuiltinBindingDescriptorKindV1::VersionMetadata => {
                        BuiltinBindingKind::VersionMetadata
                    }
                },
                tag: descriptor.tag.clone(),
                descriptor_sha256: descriptor.sha256()?,
            })
        })
        .collect::<Result<Vec<_>, PlatformError>>()?;
    Ok((cache_policy, cache_rows, descriptors, rows))
}

fn validate_asset_content(
    request: &CreateDeploymentRequest,
    content: &PreparedContent,
    vars: &BTreeMap<String, serde_json::Value>,
) -> Result<(), PlatformError> {
    let Some(assets) = content.assets() else {
        return Ok(());
    };
    assets.manifest.validate()?;
    assets.routing.validate()?;
    if let Some(binding) = assets.routing.binding.as_deref()
        && (vars.contains_key(binding)
            || request.secrets.contains_key(binding)
            || request.bindings.contains_key(binding))
    {
        return Err(PlatformError::new(
            ErrorCode::BindingTypeMismatch,
            "asset binding conflicts with another deployment env name",
        ));
    }
    if content.kind() == DeploymentContentKind::AssetsOnly
        && (!vars.is_empty()
            || !request.secrets.is_empty()
            || !request.bindings.is_empty()
            || !request.queue_consumers.is_empty()
            || !request.crons.is_empty()
            || matches!(
                assets.routing.run_worker_first,
                RunWorkerFirst::All(true) | RunWorkerFirst::Rules(_)
            ))
    {
        return Err(PlatformError::new(
            ErrorCode::AssetConfigUnsupported,
            "assets-only deployments cannot declare an execution environment",
        ));
    }
    Ok(())
}

fn map_asset_store_error(error: &PlatformError) -> PlatformError {
    match error.code() {
        ErrorCode::ArtifactIntegrityError | ErrorCode::CacheEntryCorrupt => PlatformError::new(
            ErrorCode::AssetIntegrityError,
            "static asset failed integrity verification",
        ),
        ErrorCode::LimitInvalid => PlatformError::new(
            ErrorCode::AssetLimitExceeded,
            "static asset exceeds the configured object limit",
        ),
        _ => PlatformError::new(
            ErrorCode::AssetStorageUnavailable,
            "static asset provider is unavailable",
        ),
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

#[cfg(test)]
#[path = "pipeline_tests.rs"]
mod tests;
