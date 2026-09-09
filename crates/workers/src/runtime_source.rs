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
use crate::worker_loader::{RuntimeWorkerLoaderBinding, worker_loader_namespace_key};
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

mod model;
mod resolution;

pub use model::*;

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
        resolution::validate_expected_digest(expected_worker_code_sha256)?;

        let repo = WorkerRepository::new(self.storage.db());
        let snapshot = repo.version_snapshot(
            account_id,
            worker_id,
            version_id,
            matches!(scope, RuntimeScope::Validation | RuntimeScope::Probe),
        )?;
        resolution::validate_scope(&snapshot, scope)?;

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

        let identity = resolution::ResolutionIdentity {
            account_id,
            worker_id,
            version_id,
            created_at_ms: snapshot.version.created_at_ms,
        };
        let assets = resolution::resolve_assets(&snapshot)?;
        let bundle = resolution::resolve_bundle(self, &snapshot, &assets, scope).await?;
        let environment = resolution::resolve_environment(&snapshot)?;
        let resource_bindings = resolution::resolve_resource_bindings(self, &snapshot, scope)?;
        let queue_bindings = resolution::resolve_queue_bindings(&snapshot)?;
        let workflow_bindings = resolution::resolve_workflow_bindings(&snapshot)?;
        let scheduled_targets = resolution::resolve_scheduled_targets(
            self,
            &snapshot,
            version_id,
            &workflow_bindings.runtime,
        )?;
        let services = resolution::resolve_services(&snapshot)?;
        let cache_policy = resolution::resolve_cache_policy(&snapshot)?;
        let builtins = resolution::resolve_builtin_bindings(&snapshot, bundle.as_ref(), identity)?;

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
            environment.vars.clone(),
            environment.secret_descriptors,
            resource_bindings.descriptors,
            queue_bindings.descriptors,
            workflow_bindings.descriptors,
            services.descriptors,
            cache_policy.clone(),
            builtins.descriptors,
            snapshot.version.loader_schema_version,
        )?;
        let actual_descriptor = descriptor.sha256()?;
        if actual_descriptor != snapshot.version.worker_code_sha256
            || hex::encode(actual_descriptor) != expected_worker_code_sha256
        {
            return Err(invariant());
        }

        let modules = resolution::runtime_modules(bundle.as_ref())?;
        let secrets =
            resolution::decrypt_secrets(self, &snapshot, scope, identity, &environment.vars)?;
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
            module_bindings: builtins.module_bindings,
            vars: environment.vars,
            secrets,
            bindings: resource_bindings.runtime,
            queue_bindings: queue_bindings.runtime,
            workflow_bindings: workflow_bindings.runtime,
            scheduled_targets,
            services: services.runtime,
            cache_policy: RuntimeCachePolicy {
                enabled: cache_policy.enabled,
                cross_version_cache: cache_policy.cross_version_cache,
                fail_open: self.cache_fail_open,
                entrypoints: cache_policy.entrypoints,
            },
            worker_loaders: builtins.worker_loaders,
            ai_binding: builtins.ai_binding,
            images_binding: builtins.images_binding,
            version_metadata_binding: builtins.version_metadata_binding,
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
            worker_loaders: &'a [RuntimeWorkerLoaderBinding],
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
            worker_loaders: &snapshot.worker_loaders,
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

#[allow(
    clippy::needless_pass_by_value,
    reason = "the callback contract transfers ownership of this value"
)]
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
