//! Immutable version creation pipeline.

#[path = "pipeline/bindings.rs"]
mod binding_preparation;
#[path = "pipeline/products.rs"]
mod products;
#[path = "pipeline/runtime_features.rs"]
mod runtime_features;
#[path = "pipeline/validation.rs"]
mod validation;
use binding_preparation::PreparedBindings;
use validation::{invariant, request_fingerprint};
pub(crate) use validation::{
    stable_validation_code, validate_binding_set, validate_idempotency_key,
    validate_injection_module_collisions, validate_secret_set, validate_service_set,
};

use products::{prepare_cron_config, validate_product_counts};
pub(crate) use runtime_features::idempotency_ref_id;
use runtime_features::{
    map_asset_store_error, prepare_runtime_features, validate_asset_content, validate_compatibility,
};

use crate::assets::{RunWorkerFirst, VersionAssets};
use crate::bundle::{
    BundleLimits, CanonicalBundle, StagedBundle, WORKER_BUNDLE_SCHEMA_VERSION, WorkerBundleManifest,
};
use crate::descriptor::{
    BindingDescriptorV1, BuiltinBindingDescriptorKindV1, BuiltinBindingDescriptorV1,
    CacheEntrypointPolicyV1, CachePolicyDescriptorV1, QueueProducerBindingDescriptorV1,
    SYSTEM_MODULE_PREFIX, SecretDescriptor, ServiceDescriptor, WorkerCodeDescriptorV1,
    ciphertext_sha256,
};
use crate::environment::{MAX_VARIABLE_BYTES, MAX_VARIABLES, canonicalize_vars, validate_env_name};
use crate::worker_loader::{version_has_worker_loader, worker_loader_generation_prefix};
use bytes::Bytes;
use futures::stream;
use open_compute_artifacts::ArtifactStore;
use open_compute_core::{
    BindingId, BindingKind, CanonicalBindingConfig, CanonicalPermissions, CronActivationId,
    CronSchedule, ErrorCode, InstanceId, PlatformError, QueueConsumerId, QueueId, RequestId,
    ResourceId, ResourceState, SecretBytes, SecretString, StartupId, VersionId, WorkerId,
};
use open_compute_storage::{
    BindingRepository, BuiltinBindingKind, CRON_PARSER_VERSION, DeploymentRecord, DeploymentSource,
    DurableObjectMigrationPlan, DurableObjectRepository, EffectiveResourceLimits,
    IdempotencyReservation, LOADER_SCHEMA_VERSION, NewCronConfig, NewCronDeclaration,
    NewQueueConsumerDeclaration, NewQueueProducerBinding, NewVersion, NewVersionAssets,
    NewVersionBinding, NewVersionObjectRef, NewVersionService, PlatformStorage, QueueAvailability,
    QueueConsumerConfig, QueueConsumerRepository, QueueRepository, QueueState, ResourceRepository,
    StoredVersionSecret, VersionBuiltinBindingRecord, VersionCachePolicyRecord, VersionContentKind,
    VersionObjectKind, VersionRecord, VersionState, WorkerObservabilityPatch, WorkerRepository,
    WorkflowDefinitionReservation, WorkflowRepository, WorkflowTarget,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use uuid::Uuid;
use zeroize::Zeroize;

const MAX_BINDINGS: usize = 64;
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

/// Control-plane declaration for one dynamic same-instance Service binding.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VersionServiceInput {
    /// Existing logical Worker or configured local-extension target.
    pub target: open_compute_storage::ServiceTarget,
    /// Optional named `WorkerEntrypoint` export.
    #[serde(default)]
    pub entrypoint: Option<String>,
    /// Optional deployer-authenticated JSON object delivered to the target as `ctx.props`.
    #[serde(default)]
    pub props: Option<serde_json::Value>,
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

/// Upload-time Standard resource limits declaration. Only the fixed Cloudflare upload schema
/// fields are accepted; values are validated against the Standard ceilings at materialization.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VersionResourceLimitsInput {
    /// Invocation CPU budget in milliseconds.
    #[serde(default)]
    pub cpu_ms: Option<u32>,
    /// Invocation subrequest budget.
    #[serde(default)]
    pub sub_requests: Option<u32>,
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
    /// Standard resource limits declared with the upload; omitted dimensions take the
    /// Standard defaults when the Version is materialized.
    #[serde(default)]
    pub limits: Option<VersionResourceLimitsInput>,
    /// Immutable closed Cloudflare Version annotations.
    #[serde(default)]
    pub annotations: BTreeMap<String, String>,
    /// Automatic response-cache policy.
    #[serde(default)]
    pub cache: VersionCacheInput,
    /// Native Dynamic Worker Loader binding names, frozen with this version.
    #[serde(default)]
    pub worker_loaders: Vec<String>,
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
            limits: None,
            annotations: BTreeMap::new(),
            cache: VersionCacheInput::default(),
            worker_loaders: Vec::new(),
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
    /// Optional ready dead-letter Queue in the same instance.
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
    /// Instance identity.
    pub instance_id: InstanceId,
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

    /// Revalidate a deployment candidate and return the exact running generation proof.
    fn validate_deployment(
        &self,
        candidate: ValidationCandidate,
    ) -> Pin<Box<dyn Future<Output = Result<StartupId, PlatformError>> + Send + '_>>;

    /// Current running generation, when the production validator is generation-aware.
    fn current_generation(&self) -> Option<StartupId>;

    /// Fence one Worker or route-generation prefix in the native Loader factory.
    fn revoke_worker_loader_prefix(
        &self,
        _prefix: String,
        _expected_generation: StartupId,
    ) -> Pin<Box<dyn Future<Output = Result<(), PlatformError>> + Send + '_>> {
        Box::pin(async {
            Err(PlatformError::new(
                ErrorCode::RuntimeUnavailable,
                "native Worker Loader revocation is unavailable",
            ))
        })
    }

    /// Restart the runtime after a failed commit leaves an irreversible native Loader fence.
    fn recover_worker_loader_revocation(
        &self,
        _generation: StartupId,
    ) -> Pin<Box<dyn Future<Output = Result<(), PlatformError>> + Send + '_>> {
        Box::pin(async {
            Err(PlatformError::new(
                ErrorCode::RuntimeUnavailable,
                "native Worker Loader recovery is unavailable",
            ))
        })
    }

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

    /// Prove that a frozen Workflow target exports a constructible Workflow class.
    fn validate_workflow(
        &self,
        _target: WorkflowTarget,
    ) -> Pin<Box<dyn Future<Output = Result<(), PlatformError>> + Send + '_>> {
        Box::pin(async {
            Err(PlatformError::new(
                ErrorCode::WorkflowRuntimeUnavailable,
                "runtime validator cannot prove the Workflow class",
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
#[derive(Clone, Debug, PartialEq)]
pub struct ProductPromotionRequest {
    /// Owning instance.
    pub instance_id: InstanceId,
    /// Worker whose active version changes.
    pub worker_id: WorkerId,
    /// Validated ready target version.
    pub version_id: VersionId,
    /// Exact v4 operation that creates the immutable Deployment.
    pub source: DeploymentSource,
    /// Closed Cloudflare deployment annotations persisted with the traffic assignment.
    pub annotations: BTreeMap<String, String>,
    /// Script-level observability fields committed with the active Deployment.
    pub observability: Option<WorkerObservabilityPatch>,
    /// Audit request identity.
    pub request_id: RequestId,
    /// Control-plane wall time.
    pub now_ms: i64,
}

#[cfg(any(test, feature = "test-support"))]
#[path = "pipeline/test_validator.rs"]
mod test_validator;

/// Secret-safe version request. Debug redacts secret values.
#[derive(Clone, Debug)]
pub struct CreateVersionRequest {
    /// Instance boundary.
    pub instance_id: InstanceId,
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
    /// Script-level observability fields committed only when this request deploys.
    pub observability: Option<WorkerObservabilityPatch>,
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
#[allow(
    clippy::large_enum_variant,
    reason = "the enum owns one bounded request without an extra allocation"
)]
pub enum CreateVersionOutcome {
    /// Pipeline ran and produced a new immutable version.
    Applied(CreateVersionResult),
    /// Same idempotency fingerprint already completed.
    Replay(Vec<u8>),
}

mod controller;

pub use controller::VersionController;

#[cfg(test)]
#[path = "pipeline_tests.rs"]
mod tests;
