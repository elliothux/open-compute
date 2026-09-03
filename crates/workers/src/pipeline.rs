//! Immutable version creation pipeline.

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

use crate::assets::{RunWorkerFirst, VersionAssets};
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
    CronActivationId, CronSchedule, ErrorCode, PlatformError, QueueConsumerId, QueueId, RequestId,
    ResourceId, ResourceState, SecretBytes, SecretString, VersionId, WorkerId,
};
use open_compute_storage::{
    BindingRepository, BuiltinBindingKind, CRON_PARSER_VERSION, DeploymentRecord, DeploymentSource,
    DurableObjectMigrationPlan, DurableObjectRepository, IdempotencyReservation,
    LOADER_SCHEMA_VERSION, NewCronConfig, NewCronDeclaration, NewQueueConsumerDeclaration,
    NewQueueProducerBinding, NewVersion, NewVersionAssets, NewVersionBinding, NewVersionObjectRef,
    NewVersionService, PlatformStorage, QueueAvailability, QueueConsumerConfig,
    QueueConsumerRepository, QueueRepository, QueueState, ResourceRepository, StoredVersionSecret,
    VersionBuiltinBindingRecord, VersionCachePolicyRecord, VersionContentKind, VersionObjectKind,
    VersionRecord, VersionState, WorkerRepository,
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
const MAX_QUEUE_CONSUMERS_PER_VERSION: usize = 64;
const MAX_CRONS_PER_VERSION: usize = 100;

/// Control-plane request for one immutable version resource binding.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VersionBindingInput {
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
pub struct VersionServiceInput {
    /// Existing logical target Worker identity; names are resolved by tooling before deploy.
    pub target_worker_id: WorkerId,
    /// Optional named `WorkerEntrypoint` export.
    #[serde(default)]
    pub entrypoint: Option<String>,
}

/// Automatic response-cache policy on the default or a named Worker entrypoint.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VersionCachePolicyInput {
    /// Whether automatic response caching is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Whether automatic entries are shared across version versions.
    #[serde(default)]
    pub cross_version_cache: bool,
}

/// Version-wide automatic-cache configuration and named-entrypoint overrides.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VersionCacheInput {
    /// Default export policy.
    #[serde(flatten)]
    pub default: VersionCachePolicyInput,
    /// Named Worker entrypoint policy overrides.
    #[serde(default)]
    pub entrypoints: BTreeMap<String, VersionCachePolicyInput>,
}

/// One platform-provided Images binding declaration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VersionImagesInput {
    /// Tenant environment binding name.
    pub binding: String,
}

/// One standard Workers AI binding declaration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VersionAiInput {
    /// Tenant environment binding name.
    pub binding: String,
}

/// One immutable version Version Metadata binding declaration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VersionVersionMetadataInput {
    /// Tenant environment binding name.
    pub binding: String,
    /// Optional application-supplied immutable release tag.
    #[serde(default)]
    pub tag: Option<String>,
}

/// Service-worker global backed by one immutable multipart module part.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VersionModuleBindingInput {
    /// Exact canonical bundle module name.
    pub module: String,
    /// Exact global representation emitted by fixed Wrangler.
    pub kind: ModuleBindingKind,
}

/// Supported service-worker multipart global representations.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModuleBindingKind {
    /// Compile bytes as a `WebAssembly.Module`.
    WasmModule,
    /// Decode bytes as UTF-8 text.
    TextBlob,
    /// Expose bytes as an `ArrayBuffer`.
    DataBlob,
}

/// Platform-provided runtime capabilities frozen with one version.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VersionRuntimeFeatures {
    /// Immutable compatibility date passed to the tenant isolate.
    #[serde(default = "default_compatibility_date")]
    pub compatibility_date: String,
    /// Immutable compatibility flags passed to the tenant isolate.
    #[serde(default)]
    pub compatibility_flags: Vec<String>,
    /// Immutable closed Cloudflare Version annotations.
    #[serde(default)]
    pub annotations: BTreeMap<String, String>,
    /// Automatic response-cache policy.
    #[serde(default)]
    pub cache: VersionCacheInput,
    /// Optional Workers AI binding exposing the Markdown Conversion subset.
    #[serde(default)]
    pub ai: Option<VersionAiInput>,
    /// Optional local Images binding.
    #[serde(default)]
    pub images: Option<VersionImagesInput>,
    /// Optional frozen Version Metadata binding.
    #[serde(default)]
    pub version_metadata: Option<VersionVersionMetadataInput>,
    /// Service-worker module globals keyed by tenant binding name.
    #[serde(default)]
    pub module_bindings: BTreeMap<String, VersionModuleBindingInput>,
}

impl Default for VersionRuntimeFeatures {
    fn default() -> Self {
        Self {
            compatibility_date: default_compatibility_date(),
            compatibility_flags: Vec::new(),
            annotations: BTreeMap::new(),
            cache: VersionCacheInput::default(),
            ai: None,
            images: None,
            version_metadata: None,
            module_bindings: BTreeMap::new(),
        }
    }
}

fn default_compatibility_date() -> String {
    crate::WORKER_COMPATIBILITY_DATE.to_owned()
}

/// Immutable Queue push-consumer declaration supplied with a version.
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
    /// Immutable version identity.
    pub version_id: VersionId,
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
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProductPromotionRequest {
    /// Owning account.
    pub account_id: AccountId,
    /// Worker whose active version changes.
    pub worker_id: WorkerId,
    /// Validated ready target version.
    pub version_id: VersionId,
    /// Exact v4 operation that creates the immutable Deployment.
    pub source: DeploymentSource,
    /// Closed Cloudflare deployment annotations persisted with the traffic assignment.
    pub annotations: BTreeMap<String, String>,
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

/// Secret-safe version request. Debug redacts secret values.
#[derive(Clone, Debug)]
pub struct CreateVersionRequest {
    /// Account boundary.
    pub account_id: AccountId,
    /// Parent Worker.
    pub worker_id: WorkerId,
    /// Required control idempotency key.
    pub idempotency_key: String,
    /// Explicit Worker/Assets version content union.
    pub content: VersionContent,
    /// JSON-compatible vars.
    pub vars: BTreeMap<String, serde_json::Value>,
    /// Write-only UTF-8 secrets.
    pub secrets: BTreeMap<String, SecretString>,
    /// Immutable resource bindings keyed by tenant environment name.
    pub bindings: BTreeMap<String, VersionBindingInput>,
    /// Immutable Service declarations keyed by tenant environment name.
    pub services: BTreeMap<String, VersionServiceInput>,
    /// Platform-provided runtime capabilities.
    pub runtime_features: VersionRuntimeFeatures,
    /// Immutable Queue push-consumer declarations.
    pub queue_consumers: Vec<QueueConsumerInput>,
    /// Exact Cron set for the Worker's scheduled handler.
    pub crons: Vec<String>,
    /// Create a 100-percent Deployment only after runtime validation succeeds.
    pub deployment_source: Option<DeploymentSource>,
    /// Audit request identity.
    pub request_id: RequestId,
    /// Current wall-clock milliseconds.
    pub now_ms: i64,
}

/// Canonical version artifact supplied in memory or as a verified staging file.
#[derive(Clone, Debug)]
pub enum VersionBundle {
    /// Bounded convenience input used by library callers and small tests.
    Bytes(Vec<u8>),
    /// Incrementally verified private staging file used by the HTTP upload path.
    Staged(StagedBundle),
}

/// Authoritative version content; assets-only never fabricates a Worker bundle.
#[derive(Clone, Debug)]
pub enum VersionContent {
    /// Executable Worker with optional static assets.
    Worker {
        /// Canonical Worker bundle.
        bundle: VersionBundle,
        /// Optional static assets frozen with the code.
        assets: Option<VersionAssets>,
    },
    /// Static assets without executable tenant code.
    AssetsOnly {
        /// Required immutable static assets.
        assets: VersionAssets,
    },
}

impl From<Vec<u8>> for VersionBundle {
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
        assets: Option<VersionAssets>,
    },
    AssetsOnly {
        assets: VersionAssets,
    },
}

impl PreparedContent {
    fn prepare(input: &VersionContent, limits: BundleLimits) -> Result<Self, PlatformError> {
        match input {
            VersionContent::Worker { bundle, assets } => Ok(Self::Worker {
                bundle: PreparedBundle::prepare(bundle, limits)?,
                assets: assets.clone(),
            }),
            VersionContent::AssetsOnly { assets } => Ok(Self::AssetsOnly {
                assets: assets.clone(),
            }),
        }
    }

    const fn kind(&self) -> VersionContentKind {
        match self {
            Self::Worker { .. } => VersionContentKind::Worker,
            Self::AssetsOnly { .. } => VersionContentKind::AssetsOnly,
        }
    }

    const fn bundle(&self) -> Option<&PreparedBundle> {
        match self {
            Self::Worker { bundle, .. } => Some(bundle),
            Self::AssetsOnly { .. } => None,
        }
    }

    const fn assets(&self) -> Option<&VersionAssets> {
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
    fn prepare(input: &VersionBundle, limits: BundleLimits) -> Result<Self, PlatformError> {
        match input {
            VersionBundle::Bytes(bytes) => {
                CanonicalBundle::parse(bytes.clone(), limits).map(Self::Memory)
            }
            VersionBundle::Staged(bundle) => Ok(Self::Staged(bundle.clone())),
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
pub struct CreateVersionResult {
    /// Created version.
    pub version: VersionRecord,
    /// Deployment created by the same operation, if requested.
    pub deployment: Option<DeploymentRecord>,
}

/// New result or exact persisted response bytes for replay.
#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum CreateVersionOutcome {
    /// Pipeline ran and produced a new immutable version.
    Applied(CreateVersionResult),
    /// Same idempotency fingerprint already completed.
    Replay(Vec<u8>),
}

/// P0.2 version orchestrator over typed P0.1 capabilities.
pub struct VersionController<'a> {
    storage: &'a PlatformStorage,
    artifacts: ArtifactStore,
    validator: Arc<dyn RuntimeValidator>,
    bundle_limits: BundleLimits,
    max_queue_consumer_concurrency: u32,
    product_promoter: Option<Arc<dyn ProductPromotionCoordinator>>,
    durable_object_migration: Option<DurableObjectMigrationPlan>,
}

impl std::fmt::Debug for VersionController<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VersionController")
            .field("artifacts", &self.artifacts)
            .field("bundle_limits", &self.bundle_limits)
            .finish_non_exhaustive()
    }
}

impl<'a> VersionController<'a> {
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
            durable_object_migration: None,
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

    /// Attach the prepared Durable Object plan published by this Version's ready transition.
    #[must_use]
    pub fn with_durable_object_migration(mut self, plan: DurableObjectMigrationPlan) -> Self {
        self.durable_object_migration = Some(plan);
        self
    }

    /// Execute upload, immutable DB transaction, runtime validation, and optional promotion.
    pub async fn create_version(
        &self,
        request: CreateVersionRequest,
    ) -> Result<CreateVersionOutcome, PlatformError> {
        self.create_version_with_id(request, None).await
    }

    /// Finalize a resumable upload using the version identity persisted before validation.
    pub async fn finalize_upload(
        &self,
        request: CreateVersionRequest,
        version_id: VersionId,
    ) -> Result<CreateVersionOutcome, PlatformError> {
        self.create_version_with_id(request, Some(version_id)).await
    }

    async fn create_version_with_id(
        &self,
        request: CreateVersionRequest,
        version_id: Option<VersionId>,
    ) -> Result<CreateVersionOutcome, PlatformError> {
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
        let fingerprint_input = request_fingerprint(
            &request,
            &content,
            &canonical_vars,
            version_id,
            self.durable_object_migration.as_ref(),
        )?;
        let fingerprint = self
            .storage
            .crypto()
            .fingerprint_request(&fingerprint_input);
        let reservation = repo.reserve_idempotency(
            request.account_id,
            "version.create",
            &request.idempotency_key,
            self.storage.crypto().fingerprint_key_id(),
            &fingerprint,
            request.now_ms,
            request.now_ms.saturating_add(IDEMPOTENCY_TTL_MS),
        )?;
        let recover_running = matches!(reservation, IdempotencyReservation::Running);
        match reservation {
            IdempotencyReservation::Complete(response) => {
                return Ok(CreateVersionOutcome::Replay(response));
            }
            IdempotencyReservation::Running if version_id.is_none() => {
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
                    "idempotent version operation previously failed",
                ));
            }
            IdempotencyReservation::Running | IdempotencyReservation::Reserved => {}
        }

        let fixed_version_id = version_id.unwrap_or_else(VersionId::generate);
        let migration_preflight = self
            .durable_object_migration
            .as_ref()
            .map(|plan| {
                DurableObjectRepository::new(self.storage).validate_worker_migration_version(
                    request.worker_id,
                    fixed_version_id,
                    plan,
                )
            })
            .transpose();
        let operation = if let Err(error) = migration_preflight {
            Err(error)
        } else if recover_running {
            self.resume_reserved(
                &request,
                content,
                canonical_vars,
                stored_vars,
                fixed_version_id,
            )
            .await
        } else {
            self.create_reserved(
                &request,
                content,
                canonical_vars,
                stored_vars,
                fixed_version_id,
            )
            .await
        };
        match operation {
            Ok(result) => {
                let response = serde_json::to_vec(&serde_json::json!({
                    "version": result.version.to_api_json(),
                    "deployment": result.deployment,
                }))
                .map_err(|_| invariant())?;
                repo.complete_idempotency_with_version_ref(
                    request.account_id,
                    "version.create",
                    &request.idempotency_key,
                    &fingerprint,
                    &response,
                    result.version.id,
                    &idempotency_ref_id(
                        request.account_id,
                        "version.create",
                        &request.idempotency_key,
                    ),
                    request.now_ms,
                )?;
                Ok(CreateVersionOutcome::Applied(result))
            }
            Err(error) => {
                let response = serde_json::to_vec(&FailedResponse {
                    code: error.code().as_str().to_owned(),
                })
                .map_err(|_| invariant())?;
                repo.fail_idempotency(
                    request.account_id,
                    "version.create",
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
        request: &CreateVersionRequest,
        content: PreparedContent,
        canonical_vars: BTreeMap<String, serde_json::Value>,
        stored_vars: BTreeMap<String, Vec<u8>>,
        version_id: VersionId,
    ) -> Result<CreateVersionResult, PlatformError> {
        let repo = WorkerRepository::new(self.storage.db());
        let compatibility_flags = validate_compatibility(&request.runtime_features)?;
        let _admission = self.storage.reserve_mutation(content.admission_bytes()?)?;
        let (stored_secrets, secret_descriptors) = self.encrypt_secrets(
            request.account_id,
            request.worker_id,
            version_id,
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
        } = self.prepare_bindings(request, version_id)?;
        let queue_consumers = self.prepare_queue_consumers(request)?;
        let cron = prepare_cron_config(request, &workflow_binding_descriptors)?;
        let (cache_policy, mut cache_rows, builtin_descriptors, builtin_rows) =
            prepare_runtime_features(&request.runtime_features)?;
        let mut builtin_names = HashSet::new();
        for descriptor in &builtin_descriptors {
            if !builtin_names.insert(descriptor.name.as_str())
                || canonical_vars.contains_key(&descriptor.name)
                || request.secrets.contains_key(&descriptor.name)
                || request.bindings.contains_key(&descriptor.name)
                || request.services.contains_key(&descriptor.name)
                || content
                    .assets()
                    .and_then(|assets| assets.routing.binding.as_deref())
                    == Some(descriptor.name.as_str())
            {
                return Err(PlatformError::new(
                    ErrorCode::BindingTypeMismatch,
                    "runtime binding names must be unique",
                ));
            }
            let expected_type = match descriptor.kind {
                BuiltinBindingDescriptorKindV1::WasmModule => Some(crate::ModuleType::Wasm),
                BuiltinBindingDescriptorKindV1::TextBlob => Some(crate::ModuleType::Text),
                BuiltinBindingDescriptorKindV1::DataBlob => Some(crate::ModuleType::Data),
                _ => None,
            };
            if let Some(expected_type) = expected_type {
                let module_name = descriptor.tag.as_deref().ok_or_else(invariant)?;
                if !content.bundle().is_some_and(|bundle| {
                    bundle.manifest().modules.iter().any(|module| {
                        module.name == module_name && module.module_type == expected_type
                    })
                }) {
                    return Err(PlatformError::new(
                        ErrorCode::BundleInvalid,
                        "service-worker module binding does not match a canonical bundle part",
                    ));
                }
            }
        }
        let descriptor = WorkerCodeDescriptorV1::new(
            request.account_id,
            request.worker_id,
            version_id,
            request.now_ms,
            request.runtime_features.compatibility_date.clone(),
            compatibility_flags.clone(),
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
        if content.kind() == VersionContentKind::AssetsOnly {
            cache_rows.clear();
        }
        let descriptor_hash = descriptor.sha256()?;
        let artifact_reservation = self.artifacts.reserve_version_artifact().await;
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
        let version = repo.insert_staging_version(
            &NewVersion {
                id: version_id,
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
                compatibility_date: request.runtime_features.compatibility_date.clone(),
                compatibility_flags,
                vars: stored_vars,
                secrets: stored_secrets,
                request_id: request.request_id,
                now_ms: request.now_ms,
            },
            &open_compute_storage::NewVersionProducts {
                annotations: Some(&request.runtime_features.annotations),
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
                cron: (content.kind() == VersionContentKind::Worker).then_some(&cron),
            },
            self.storage.hardening().max_versions_per_worker,
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
            version,
            durable_object_classes,
            queue_entrypoints,
            requires_product_promoter,
        )
        .await
    }

    async fn resume_reserved(
        &self,
        request: &CreateVersionRequest,
        content: PreparedContent,
        canonical_vars: BTreeMap<String, serde_json::Value>,
        stored_vars: BTreeMap<String, Vec<u8>>,
        version_id: VersionId,
    ) -> Result<CreateVersionResult, PlatformError> {
        let repo = WorkerRepository::new(self.storage.db());
        let version =
            match repo.get_worker_version(request.account_id, request.worker_id, version_id) {
                Ok(version) => version,
                Err(error) if error.code() == ErrorCode::VersionNotFound => {
                    return self
                        .create_reserved(request, content, canonical_vars, stored_vars, version_id)
                        .await;
                }
                Err(error) => return Err(error),
            };
        if version.content_kind != content.kind() || version.deleted_at_ms.is_some() {
            return Err(invariant());
        }
        let mut durable_object_classes = Vec::new();
        for binding in BindingRepository::new(self.storage.db()).version_bindings(version_id)? {
            if binding.kind == BindingKind::DoNamespace {
                let namespace = DurableObjectRepository::new(self.storage)
                    .get_namespace(request.account_id, binding.resource_id)?;
                durable_object_classes.push(namespace.class_name);
            }
        }
        durable_object_classes.sort();
        durable_object_classes.dedup();
        let queue_declarations =
            QueueConsumerRepository::new(self.storage.db()).version_declarations(version_id)?;
        let cron_declarations = open_compute_storage::CronRepository::new(self.storage.db())
            .version_config(version_id)?
            .declarations;
        let requires_product_promoter =
            !queue_declarations.is_empty() || !cron_declarations.is_empty();
        let mut queue_entrypoints = queue_declarations
            .into_iter()
            .map(|consumer| consumer.entrypoint)
            .collect::<Vec<_>>();
        let (cache_policies, _) =
            open_compute_storage::version_runtime_features(self.storage.db(), version_id)?;
        queue_entrypoints.extend(
            cache_policies
                .into_iter()
                .filter(|policy| policy.enabled && policy.entrypoint.is_some())
                .map(|policy| policy.entrypoint),
        );
        self.finish_reserved(
            request,
            version,
            durable_object_classes,
            queue_entrypoints,
            requires_product_promoter,
        )
        .await
    }

    async fn finish_reserved(
        &self,
        request: &CreateVersionRequest,
        mut version: VersionRecord,
        durable_object_classes: Vec<String>,
        queue_entrypoints: Vec<Option<String>>,
        requires_product_promoter: bool,
    ) -> Result<CreateVersionResult, PlatformError> {
        let repo = WorkerRepository::new(self.storage.db());
        if version.state == VersionState::Rejected {
            let code = version
                .rejection_code
                .as_deref()
                .and_then(ErrorCode::from_stable_str)
                .unwrap_or(ErrorCode::BundleRuntimeInvalid);
            return Err(PlatformError::new(
                code,
                "version validation previously failed",
            ));
        }
        if version.state == VersionState::Staging {
            repo.begin_validation(version.id)?;
            version.state = VersionState::Validating;
        }
        let candidate = ValidationCandidate {
            account_id: request.account_id,
            worker_id: request.worker_id,
            version_id: version.id,
            worker_code_sha256: version.worker_code_sha256,
        };
        let validation = if version.state == VersionState::Validating
            && version.content_kind == VersionContentKind::Worker
        {
            self.validator.validate(candidate.clone()).await
        } else {
            Ok(())
        };
        if let Err(err) = validation {
            let code = stable_validation_code(&err);
            repo.mark_rejected(version.id, VersionState::Validating, code, request.now_ms)?;
            return Err(PlatformError::new(
                code,
                "real workerd validation rejected the version",
            ));
        }
        for class_name in durable_object_classes {
            if version.state != VersionState::Validating {
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
                repo.mark_rejected(version.id, VersionState::Validating, code, request.now_ms)?;
                return Err(PlatformError::new(
                    code,
                    "real workerd validation rejected a Durable Object class",
                ));
            }
        }
        for entrypoint in &queue_entrypoints {
            if version.state == VersionState::Validating
                && let Some(entrypoint) = entrypoint
                && let Err(error) = self
                    .validator
                    .validate_entrypoint(candidate.clone(), entrypoint.clone())
                    .await
            {
                let code = stable_validation_code(&error);
                repo.mark_rejected(version.id, VersionState::Validating, code, request.now_ms)?;
                return Err(PlatformError::new(
                    code,
                    "real workerd validation rejected a named entrypoint",
                ));
            }
        }
        if version.state == VersionState::Validating {
            if let Some(plan) = &self.durable_object_migration {
                repo.mark_ready_with_durable_object_migration(
                    version.id,
                    request.worker_id,
                    plan,
                    request.now_ms,
                )?;
            } else {
                repo.mark_ready(version.id, request.now_ms)?;
            }
            version.state = VersionState::Ready;
            version.ready_at_ms = Some(request.now_ms);
        }
        let deployment = if let Some(source) = request.deployment_source {
            let worker = repo.get_worker(request.account_id, request.worker_id)?;
            if worker.active_version_id == Some(version.id) {
                let deployment_id = worker.active_deployment_id.ok_or_else(invariant)?;
                return Ok(CreateVersionResult {
                    version,
                    deployment: Some(repo.get_deployment(
                        request.account_id,
                        request.worker_id,
                        deployment_id,
                    )?),
                });
            }
            for route in repo.list_worker_routes(request.account_id, request.worker_id)? {
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
                        version_id: version.id,
                        source,
                        annotations: BTreeMap::new(),
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
                repo.create_deployment_checked(
                    request.account_id,
                    request.worker_id,
                    version.id,
                    None,
                    Some(worker.route_generation),
                    source,
                    &BTreeMap::new(),
                    request.request_id,
                    request.now_ms,
                )?;
            }
            let worker = repo.get_worker(request.account_id, request.worker_id)?;
            let deployment_id = worker.active_deployment_id.ok_or_else(invariant)?;
            Some(repo.get_deployment(request.account_id, request.worker_id, deployment_id)?)
        } else {
            None
        };
        let result = CreateVersionResult {
            version,
            deployment,
        };
        Ok(result)
    }

    async fn prepare_assets(
        &self,
        assets: Option<&VersionAssets>,
    ) -> Result<Option<(NewVersionAssets, Vec<NewVersionObjectRef>)>, PlatformError> {
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
        let mut refs = vec![NewVersionObjectRef {
            kind: VersionObjectKind::AssetManifest,
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
            refs.push(NewVersionObjectRef {
                kind: VersionObjectKind::AssetBlob,
                sha256: *object.sha256_bytes(),
                size: object.size(),
            });
        }
        Ok(Some((
            NewVersionAssets {
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
        version_id: VersionId,
        secrets: &BTreeMap<String, SecretString>,
    ) -> Result<(BTreeMap<String, StoredVersionSecret>, Vec<SecretDescriptor>), PlatformError> {
        let mut stored = BTreeMap::new();
        let mut descriptors = Vec::with_capacity(secrets.len());
        for (name, value) in secrets {
            let revision_id = Uuid::now_v7().to_string();
            let plaintext = SecretBytes::new(value.expose().as_bytes().to_vec());
            let envelope = self.storage.crypto().encrypt(
                &plaintext,
                account_id,
                worker_id,
                version_id,
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
                StoredVersionSecret {
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
    input: &VersionRuntimeFeatures,
) -> Result<
    (
        CachePolicyDescriptorV1,
        Vec<VersionCachePolicyRecord>,
        Vec<BuiltinBindingDescriptorV1>,
        Vec<VersionBuiltinBindingRecord>,
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
    let mut cache_rows = vec![VersionCachePolicyRecord {
        entrypoint: None,
        enabled: cache_policy.enabled,
        cross_version_cache: cache_policy.cross_version_cache,
    }];
    cache_rows.extend(cache_policy.entrypoints.iter().map(|(name, policy)| {
        VersionCachePolicyRecord {
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
    for (name, binding) in &input.module_bindings {
        let kind = match binding.kind {
            ModuleBindingKind::WasmModule => BuiltinBindingDescriptorKindV1::WasmModule,
            ModuleBindingKind::TextBlob => BuiltinBindingDescriptorKindV1::TextBlob,
            ModuleBindingKind::DataBlob => BuiltinBindingDescriptorKindV1::DataBlob,
        };
        descriptors.push(BuiltinBindingDescriptorV1::new(
            name.clone(),
            kind,
            Some(binding.module.clone()),
        )?);
    }
    descriptors.sort_by(|a, b| a.name.as_bytes().cmp(b.name.as_bytes()));
    let rows = descriptors
        .iter()
        .map(|descriptor| {
            Ok(VersionBuiltinBindingRecord {
                name: descriptor.name.clone(),
                kind: match descriptor.kind {
                    BuiltinBindingDescriptorKindV1::Ai => BuiltinBindingKind::Ai,
                    BuiltinBindingDescriptorKindV1::Images => BuiltinBindingKind::Images,
                    BuiltinBindingDescriptorKindV1::VersionMetadata => {
                        BuiltinBindingKind::VersionMetadata
                    }
                    BuiltinBindingDescriptorKindV1::WasmModule => BuiltinBindingKind::WasmModule,
                    BuiltinBindingDescriptorKindV1::TextBlob => BuiltinBindingKind::TextBlob,
                    BuiltinBindingDescriptorKindV1::DataBlob => BuiltinBindingKind::DataBlob,
                },
                tag: descriptor.tag.clone(),
                descriptor_sha256: descriptor.sha256()?,
            })
        })
        .collect::<Result<Vec<_>, PlatformError>>()?;
    Ok((cache_policy, cache_rows, descriptors, rows))
}

fn validate_compatibility(input: &VersionRuntimeFeatures) -> Result<Vec<String>, PlatformError> {
    // P6 intentionally certifies only the formal pin's latest date. Supporting an older date
    // requires separate stock-workerd evidence and an explicit capability-range update.
    if input.compatibility_date != crate::WORKER_COMPATIBILITY_DATE {
        return Err(PlatformError::new(
            ErrorCode::CompatibilityUnsupported,
            "compatibility date is outside the certified pinned-workerd range",
        ));
    }
    if !crate::supports_worker_compatibility(&input.compatibility_date, &input.compatibility_flags)
    {
        return Err(PlatformError::new(
            ErrorCode::CompatibilityUnsupported,
            "compatibility flags are outside the fixed pinned-runtime contract",
        ));
    }
    Ok(input.compatibility_flags.clone())
}

fn validate_asset_content(
    request: &CreateVersionRequest,
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
            "asset binding conflicts with another version env name",
        ));
    }
    if content.kind() == VersionContentKind::AssetsOnly
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
            "assets-only versions cannot declare an execution environment",
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
    hasher.update(b"open-compute/version-referrer/v1\0");
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
