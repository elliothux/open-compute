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
    ciphertext_sha256,
};
use crate::environment::{MAX_VARIABLE_BYTES, MAX_VARIABLES, canonicalize_vars, validate_env_name};
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

/// Control-plane declaration for one dynamic same-account Service binding.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VersionServiceInput {
    /// Existing logical target Worker identity; names are resolved by tooling before deploy.
    pub target_worker_id: WorkerId,
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

#[allow(
    clippy::type_complexity,
    reason = "the callable signature directly models the runtime protocol"
)]
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
    for name in &input.worker_loaders {
        descriptors.push(BuiltinBindingDescriptorV1::new(
            name.clone(),
            BuiltinBindingDescriptorKindV1::WorkerLoader,
            None,
        )?);
    }
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
                    BuiltinBindingDescriptorKindV1::WorkerLoader => {
                        BuiltinBindingKind::WorkerLoader
                    }
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
