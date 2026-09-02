//! Immutable `WorkerCode` descriptor and loader key grammar.

use crate::assets::{AssetManifestV1, AssetRoutingConfigV1};
use crate::bundle::{ModuleManifest, WorkerBundleManifest};
use open_compute_core::{
    AccountId, BindingId, BindingKind, CanonicalBindingConfig, CanonicalPermissions, ErrorCode,
    PlatformError, ResourceId, VersionId, WorkerId,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

/// Namespace reserved for all platform-owned loaded-isolate modules.
pub const SYSTEM_MODULE_PREFIX: &str = "__open_compute__/";
const SYSTEM_WORKER_MANIFEST: &[u8] =
    include_bytes!("../../../packages/runtime/dist/manifest.json");

/// Immutable asset identity and routing included in the version descriptor.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssetDescriptorV1 {
    /// Canonical manifest artifact digest.
    pub manifest_sha256: String,
    /// Canonical manifest byte length.
    pub manifest_size: u64,
    /// Frozen route and optional binding configuration.
    pub routing: AssetRoutingConfigV1,
}

#[cfg(test)]
mod source_tests;

/// Secret identity included without plaintext.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SecretDescriptor {
    /// Env name.
    pub name: String,
    /// Immutable random revision.
    pub revision_id: String,
    /// Digest of nonce plus ciphertext.
    pub ciphertext_sha256: String,
}

/// Canonical immutable descriptor for one version resource binding.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BindingDescriptorV1 {
    /// Descriptor schema version. P0.3 supports exactly one.
    pub schema_version: u32,
    /// Immutable binding identity.
    pub binding_id: BindingId,
    /// Tenant environment name.
    pub name: String,
    /// Static adapter and resource kind.
    pub kind: BindingKind,
    /// Frozen logical resource identity.
    pub resource_id: ResourceId,
    /// Frozen binding-breaking resource generation.
    pub resource_spec_generation: u64,
    /// Static adapter capability version.
    pub capability_version: u32,
    /// Canonical method permissions.
    pub permissions: CanonicalPermissions,
    /// Canonical product configuration.
    pub config: CanonicalBindingConfig,
}

/// Canonical immutable declaration for one dynamic cross-Worker Service binding.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServiceDescriptorV1 {
    /// Descriptor schema version.
    pub schema_version: u32,
    /// Tenant environment binding name.
    pub name: String,
    /// Frozen logical target Worker identity.
    pub target_worker_id: WorkerId,
    /// Optional named `WorkerEntrypoint` export.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entrypoint: Option<String>,
    /// Invocation policy schema enforced by the trusted controller.
    pub policy_version: u32,
}

/// Immutable automatic response-cache policy for one version.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CachePolicyDescriptorV1 {
    /// Default export policy.
    pub enabled: bool,
    /// Share automatic entries across version versions.
    pub cross_version_cache: bool,
    /// Named entrypoint overrides, sorted by entrypoint name.
    #[serde(default)]
    pub entrypoints: BTreeMap<String, CacheEntrypointPolicyV1>,
}

/// Immutable automatic-cache override for one named Worker entrypoint.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CacheEntrypointPolicyV1 {
    /// Whether automatic response caching is enabled for the entrypoint.
    pub enabled: bool,
    /// Share automatic entries across version versions.
    pub cross_version_cache: bool,
}

impl CachePolicyDescriptorV1 {
    /// Validate the complete version cache policy.
    pub fn validate(&self) -> Result<(), PlatformError> {
        for name in self.entrypoints.keys() {
            if name == "default"
                || name.len() > 128
                || !name
                    .bytes()
                    .next()
                    .is_some_and(|byte| byte.is_ascii_alphabetic() || matches!(byte, b'_' | b'$'))
                || name
                    .bytes()
                    .any(|byte| !byte.is_ascii_alphanumeric() && !matches!(byte, b'_' | b'$'))
            {
                return Err(PlatformError::new(
                    ErrorCode::EntrypointNotFound,
                    "cache policy entrypoint is invalid",
                ));
            }
        }
        Ok(())
    }
}

/// Kind of immutable platform-provided version binding.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuiltinBindingDescriptorKindV1 {
    /// Workers AI binding limited to Markdown Conversion.
    Ai,
    /// Local Images transformation capability.
    Images,
    /// Frozen version Version Metadata object.
    VersionMetadata,
}

/// Canonical descriptor for an Images or Version Metadata binding.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BuiltinBindingDescriptorV1 {
    /// Descriptor schema version.
    pub schema_version: u32,
    /// Tenant environment name.
    pub name: String,
    /// Platform-provided binding kind.
    pub kind: BuiltinBindingDescriptorKindV1,
    /// Optional immutable version tag, only for Version Metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
}

impl BuiltinBindingDescriptorV1 {
    /// Validate and construct one immutable platform binding descriptor.
    pub fn new(
        name: String,
        kind: BuiltinBindingDescriptorKindV1,
        tag: Option<String>,
    ) -> Result<Self, PlatformError> {
        validate_env_name(&name)?;
        if name.len() > 64
            || !matches!(kind, BuiltinBindingDescriptorKindV1::VersionMetadata) && tag.is_some()
            || tag.as_deref().is_some_and(|value| {
                value.is_empty()
                    || value.len() > 128
                    || value.bytes().any(|byte| byte.is_ascii_control())
            })
        {
            return Err(binding_invariant());
        }
        Ok(Self {
            schema_version: 1,
            name,
            kind,
            tag,
        })
    }

    /// Canonical typed JSON bytes persisted and hashed at staging.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, PlatformError> {
        if self.schema_version != 1 {
            return Err(binding_invariant());
        }
        serde_json::to_vec(self).map_err(|_| binding_invariant())
    }

    /// SHA-256 of canonical descriptor bytes.
    pub fn sha256(&self) -> Result<[u8; 32], PlatformError> {
        Ok(Sha256::digest(self.canonical_bytes()?).into())
    }
}

impl ServiceDescriptorV1 {
    /// Validate and build the first Service invocation policy.
    pub fn new(
        name: String,
        target_worker_id: WorkerId,
        entrypoint: Option<String>,
    ) -> Result<Self, PlatformError> {
        validate_env_name(&name)?;
        if name.len() > 64 {
            return Err(binding_invariant());
        }
        if entrypoint.as_deref().is_some_and(|value| {
            value.is_empty()
                || value.len() > 128
                || !value
                    .bytes()
                    .next()
                    .is_some_and(|byte| byte.is_ascii_alphabetic() || matches!(byte, b'_' | b'$'))
                || value
                    .bytes()
                    .any(|byte| !byte.is_ascii_alphanumeric() && !matches!(byte, b'_' | b'$'))
        }) {
            return Err(PlatformError::new(
                ErrorCode::ServiceEntrypointNotFound,
                "Service entrypoint name is invalid",
            ));
        }
        Ok(Self {
            schema_version: 1,
            name,
            target_worker_id,
            entrypoint,
            policy_version: 1,
        })
    }

    /// Canonical typed JSON bytes persisted and hashed at staging.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, PlatformError> {
        if self.schema_version != 1 || self.policy_version != 1 {
            return Err(binding_invariant());
        }
        serde_json::to_vec(self).map_err(|_| binding_invariant())
    }

    /// SHA-256 of canonical descriptor bytes.
    pub fn sha256(&self) -> Result<[u8; 32], PlatformError> {
        Ok(Sha256::digest(self.canonical_bytes()?).into())
    }
}

impl BindingDescriptorV1 {
    /// Validate and build the P0.3 capability version implemented by the static registry.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        binding_id: BindingId,
        name: String,
        kind: BindingKind,
        resource_id: ResourceId,
        resource_spec_generation: u64,
        capability_version: u32,
        permissions: CanonicalPermissions,
        config: CanonicalBindingConfig,
    ) -> Result<Self, PlatformError> {
        validate_env_name(&name)?;
        if name.len() > 64
            || resource_spec_generation == 0
            || matches!(kind, BindingKind::QueueProducer | BindingKind::Workflow)
        {
            return Err(binding_invariant());
        }
        if capability_version != 1 {
            return Err(PlatformError::new(
                ErrorCode::BindingCapabilityUnsupported,
                "binding capability version is not supported",
            ));
        }
        Ok(Self {
            schema_version: 1,
            binding_id,
            name,
            kind,
            resource_id,
            resource_spec_generation,
            capability_version,
            permissions,
            config,
        })
    }

    /// Canonical typed JSON bytes persisted and hashed at staging.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, PlatformError> {
        if self.schema_version != 1 || self.capability_version != 1 {
            return Err(binding_invariant());
        }
        serde_json::to_vec(self).map_err(|_| binding_invariant())
    }

    /// SHA-256 of canonical descriptor bytes.
    pub fn sha256(&self) -> Result<[u8; 32], PlatformError> {
        Ok(Sha256::digest(self.canonical_bytes()?).into())
    }
}

/// Canonical immutable descriptor for one Queue producer binding.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QueueProducerBindingDescriptorV1 {
    /// Descriptor schema version.
    pub schema_version: u32,
    /// Immutable binding identity.
    pub binding_id: BindingId,
    /// Tenant environment name.
    pub name: String,
    /// Fixed runtime binding kind.
    pub kind: BindingKind,
    /// Frozen Queue identity.
    pub queue_id: open_compute_core::QueueId,
    /// Frozen Queue lifecycle generation.
    pub queue_lifecycle_generation: u64,
    /// Static producer capability version.
    pub capability_version: u32,
}

impl QueueProducerBindingDescriptorV1 {
    /// Validate and build the P2.2 Queue producer descriptor.
    pub fn new(
        binding_id: BindingId,
        name: String,
        queue_id: open_compute_core::QueueId,
        queue_lifecycle_generation: u64,
        capability_version: u32,
    ) -> Result<Self, PlatformError> {
        validate_env_name(&name)?;
        if name.len() > 64 || queue_lifecycle_generation == 0 {
            return Err(binding_invariant());
        }
        if capability_version != 1 {
            return Err(PlatformError::new(
                ErrorCode::BindingCapabilityUnsupported,
                "Queue binding capability version is not supported",
            ));
        }
        Ok(Self {
            schema_version: 1,
            binding_id,
            name,
            kind: BindingKind::QueueProducer,
            queue_id,
            queue_lifecycle_generation,
            capability_version,
        })
    }

    /// Canonical typed JSON bytes persisted and hashed at staging.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, PlatformError> {
        if self.schema_version != 1
            || self.kind != BindingKind::QueueProducer
            || self.capability_version != 1
            || self.queue_lifecycle_generation == 0
        {
            return Err(binding_invariant());
        }
        serde_json::to_vec(self).map_err(|_| binding_invariant())
    }

    /// SHA-256 of canonical descriptor bytes.
    pub fn sha256(&self) -> Result<[u8; 32], PlatformError> {
        Ok(Sha256::digest(self.canonical_bytes()?).into())
    }
}

/// Canonical hash input for every runtime-effective version field.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkerCodeDescriptorV1 {
    /// Descriptor schema.
    pub schema_version: u32,
    /// Canonical three-segment loader key.
    pub loader_key: String,
    /// Immutable version creation timestamp used by Version Metadata.
    pub created_at_ms: i64,
    /// Immutable compatibility date used to compile the tenant isolate.
    pub compatibility_date: String,
    /// Immutable sorted compatibility flags used to compile the tenant isolate.
    pub compatibility_flags: Vec<String>,
    /// Explicit version content union discriminator.
    pub content_kind: open_compute_storage::VersionContentKind,
    /// Canonical artifact digest.
    pub artifact_sha256: Option<String>,
    /// Artifact schema.
    pub artifact_schema_version: Option<u32>,
    /// Main module.
    pub main_module: Option<String>,
    /// Ordered module descriptors.
    pub ordered_modules: Vec<ModuleManifest>,
    /// Optional immutable static-asset descriptor.
    pub assets: Option<AssetDescriptorV1>,
    /// Canonical JSON vars.
    pub canonical_vars: BTreeMap<String, serde_json::Value>,
    /// Sorted secret revision descriptors.
    pub secret_revisions: Vec<SecretDescriptor>,
    /// Canonically sorted immutable resource binding descriptors.
    pub binding_descriptors: Vec<BindingDescriptorV1>,
    /// Canonically sorted immutable Queue producer binding descriptors.
    pub queue_binding_descriptors: Vec<QueueProducerBindingDescriptorV1>,
    /// Canonically sorted immutable Workflow binding descriptors.
    pub workflow_binding_descriptors: Vec<open_compute_storage::WorkflowBindingDescriptor>,
    /// Canonically sorted dynamic Service declarations.
    pub service_descriptors: Vec<ServiceDescriptorV1>,
    /// Immutable automatic response-cache policy.
    pub cache_policy: CachePolicyDescriptorV1,
    /// Canonically sorted platform-provided environment bindings.
    pub builtin_binding_descriptors: Vec<BuiltinBindingDescriptorV1>,
    /// SHA-256 of the complete generated system Worker source manifest.
    pub system_worker_sources_sha256: String,
    /// Loader host schema.
    pub loader_schema_version: u32,
}

impl WorkerCodeDescriptorV1 {
    /// Build and validate the canonical descriptor with every current product binding kind.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        account_id: AccountId,
        worker_id: WorkerId,
        version_id: VersionId,
        created_at_ms: i64,
        compatibility_date: String,
        compatibility_flags: Vec<String>,
        artifact: Option<([u8; 32], &WorkerBundleManifest)>,
        assets: Option<(&AssetManifestV1, &AssetRoutingConfigV1)>,
        canonical_vars: BTreeMap<String, serde_json::Value>,
        mut secret_revisions: Vec<SecretDescriptor>,
        mut binding_descriptors: Vec<BindingDescriptorV1>,
        mut queue_binding_descriptors: Vec<QueueProducerBindingDescriptorV1>,
        mut workflow_binding_descriptors: Vec<open_compute_storage::WorkflowBindingDescriptor>,
        mut service_descriptors: Vec<ServiceDescriptorV1>,
        cache_policy: CachePolicyDescriptorV1,
        mut builtin_binding_descriptors: Vec<BuiltinBindingDescriptorV1>,
        loader_schema_version: u32,
    ) -> Result<Self, PlatformError> {
        if created_at_ms < 0
            || compatibility_date != "2026-08-30"
            || !compatibility_flags.is_empty()
        {
            return Err(binding_invariant());
        }
        let content_kind = if artifact.is_some() {
            open_compute_storage::VersionContentKind::Worker
        } else {
            open_compute_storage::VersionContentKind::AssetsOnly
        };
        if artifact.is_none() && assets.is_none() {
            return Err(binding_invariant());
        }
        secret_revisions.sort_by(|a, b| a.name.as_bytes().cmp(b.name.as_bytes()));
        if secret_revisions
            .windows(2)
            .any(|pair| pair[0].name == pair[1].name)
        {
            return Err(PlatformError::new(
                ErrorCode::SecretInvalid,
                "version contains duplicate secret names",
            ));
        }
        let mut env_names: BTreeSet<&str> = canonical_vars.keys().map(String::as_str).collect();
        for secret in &secret_revisions {
            validate_env_name(&secret.name)?;
            if !env_names.insert(&secret.name) {
                return Err(PlatformError::new(
                    ErrorCode::SecretInvalid,
                    "a version env name is used by both a var and a secret",
                ));
            }
            if secret.revision_id.is_empty()
                || secret.ciphertext_sha256.len() != 64
                || !secret
                    .ciphertext_sha256
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit())
            {
                return Err(PlatformError::new(
                    ErrorCode::SecretInvalid,
                    "secret revision descriptor is invalid",
                ));
            }
        }
        binding_descriptors.sort_by(|a, b| a.name.as_bytes().cmp(b.name.as_bytes()));
        for binding in &binding_descriptors {
            if binding.schema_version != 1
                || binding.capability_version != 1
                || binding.resource_spec_generation == 0
            {
                return Err(binding_invariant());
            }
            validate_env_name(&binding.name)?;
            if binding.name.len() > 64 || !env_names.insert(&binding.name) {
                return Err(PlatformError::new(
                    ErrorCode::BindingTypeMismatch,
                    "version binding names are duplicate or conflict with env",
                ));
            }
        }
        queue_binding_descriptors.sort_by(|a, b| a.name.as_bytes().cmp(b.name.as_bytes()));
        for binding in &queue_binding_descriptors {
            if binding.schema_version != 1
                || binding.kind != BindingKind::QueueProducer
                || binding.capability_version != 1
                || binding.queue_lifecycle_generation == 0
            {
                return Err(binding_invariant());
            }
            validate_env_name(&binding.name)?;
            if binding.name.len() > 64 || !env_names.insert(&binding.name) {
                return Err(PlatformError::new(
                    ErrorCode::BindingTypeMismatch,
                    "Queue binding names are duplicate or conflict with env",
                ));
            }
        }
        workflow_binding_descriptors.sort_by(|a, b| a.name.as_bytes().cmp(b.name.as_bytes()));
        for binding in &workflow_binding_descriptors {
            binding.sha256()?;
            if !env_names.insert(&binding.name) {
                return Err(binding_invariant());
            }
        }
        service_descriptors.sort_by(|a, b| a.name.as_bytes().cmp(b.name.as_bytes()));
        for service in &service_descriptors {
            service.canonical_bytes()?;
            validate_env_name(&service.name)?;
            if !env_names.insert(&service.name) {
                return Err(PlatformError::new(
                    ErrorCode::BindingTypeMismatch,
                    "Service binding name conflicts with version env",
                ));
            }
        }
        cache_policy.validate()?;
        builtin_binding_descriptors.sort_by(|a, b| a.name.as_bytes().cmp(b.name.as_bytes()));
        for binding in &builtin_binding_descriptors {
            binding.canonical_bytes()?;
            if !env_names.insert(&binding.name) {
                return Err(PlatformError::new(
                    ErrorCode::BindingTypeMismatch,
                    "platform binding name conflicts with version env",
                ));
            }
        }
        if let Some((manifest, routing)) = assets {
            manifest.validate()?;
            routing.validate()?;
            if let Some(binding) = routing.binding.as_deref()
                && !env_names.insert(binding)
            {
                return Err(PlatformError::new(
                    ErrorCode::BindingTypeMismatch,
                    "asset binding name conflicts with version env",
                ));
            }
        }
        if content_kind == open_compute_storage::VersionContentKind::AssetsOnly
            && (!canonical_vars.is_empty()
                || !secret_revisions.is_empty()
                || !binding_descriptors.is_empty()
                || !queue_binding_descriptors.is_empty()
                || !workflow_binding_descriptors.is_empty()
                || !service_descriptors.is_empty()
                || cache_policy.enabled
                || !cache_policy.entrypoints.is_empty()
                || !builtin_binding_descriptors.is_empty()
                || matches!(
                    assets.map(|(_, routing)| &routing.run_worker_first),
                    Some(
                        crate::assets::RunWorkerFirst::All(true)
                            | crate::assets::RunWorkerFirst::Rules(_)
                    )
                ))
        {
            return Err(PlatformError::new(
                ErrorCode::AssetConfigUnsupported,
                "assets-only versions cannot declare an execution environment",
            ));
        }
        let (artifact_sha256, artifact_schema_version, main_module, ordered_modules) = artifact
            .map_or((None, None, None, Vec::new()), |(digest, manifest)| {
                (
                    Some(hex::encode(digest)),
                    Some(manifest.schema_version),
                    Some(manifest.main_module.clone()),
                    manifest.modules.clone(),
                )
            });
        let assets = assets
            .map(|(manifest, routing)| {
                Ok::<AssetDescriptorV1, PlatformError>(AssetDescriptorV1 {
                    manifest_sha256: hex::encode(manifest.sha256()?),
                    manifest_size: u64::try_from(manifest.canonical_bytes()?.len())
                        .map_err(|_| binding_invariant())?,
                    routing: routing.clone(),
                })
            })
            .transpose()?;
        Ok(Self {
            schema_version: 1,
            loader_key: loader_key(account_id, worker_id, version_id),
            created_at_ms,
            compatibility_date,
            compatibility_flags,
            content_kind,
            artifact_sha256,
            artifact_schema_version,
            main_module,
            ordered_modules,
            assets,
            canonical_vars,
            secret_revisions,
            binding_descriptors,
            queue_binding_descriptors,
            system_worker_sources_sha256: hex::encode(Sha256::digest(SYSTEM_WORKER_MANIFEST)),
            workflow_binding_descriptors,
            service_descriptors,
            cache_policy,
            builtin_binding_descriptors,
            loader_schema_version,
        })
    }

    /// Canonical JSON bytes used as the hash input.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, PlatformError> {
        serde_json::to_vec(self).map_err(|_| {
            PlatformError::new(
                ErrorCode::VersionInvariantViolation,
                "WorkerCode descriptor could not be canonicalized",
            )
        })
    }

    /// SHA-256 of the canonical descriptor.
    pub fn sha256(&self) -> Result<[u8; 32], PlatformError> {
        Ok(Sha256::digest(self.canonical_bytes()?).into())
    }
}

/// Construct the immutable logical loader key.
#[must_use]
pub fn loader_key(account_id: AccountId, worker_id: WorkerId, version_id: VersionId) -> String {
    format!("{account_id}/{worker_id}/{version_id}")
}

/// Strictly parse a loader key without percent decoding or alternate forms.
pub fn parse_loader_key(key: &str) -> Result<(AccountId, WorkerId, VersionId), PlatformError> {
    let mut parts = key.split('/');
    let account = parts.next().ok_or_else(invalid_key)?;
    let worker = parts.next().ok_or_else(invalid_key)?;
    let version = parts.next().ok_or_else(invalid_key)?;
    if parts.next().is_some() || key.contains('%') {
        return Err(invalid_key());
    }
    Ok((
        AccountId::from_str(account).map_err(|_| invalid_key())?,
        WorkerId::from_str(worker).map_err(|_| invalid_key())?,
        VersionId::from_str(version).map_err(|_| invalid_key())?,
    ))
}

/// Canonicalize and validate JSON vars and env names.
#[allow(clippy::type_complexity)]
pub fn canonicalize_vars(
    vars: BTreeMap<String, serde_json::Value>,
    max_count: usize,
    max_bytes: usize,
) -> Result<
    (
        BTreeMap<String, serde_json::Value>,
        BTreeMap<String, Vec<u8>>,
    ),
    PlatformError,
> {
    if vars.len() > max_count {
        return Err(PlatformError::new(
            ErrorCode::ResourceLimitExceeded,
            "version contains too many vars",
        ));
    }
    let mut values = BTreeMap::new();
    let mut bytes = BTreeMap::new();
    let mut total = 0_usize;
    for (name, value) in vars {
        validate_env_name(&name)?;
        let canonical = canonical_json(value, 0)?;
        let encoded = serde_json::to_vec(&canonical).map_err(|_| {
            PlatformError::new(ErrorCode::BundleInvalid, "var JSON could not be encoded")
        })?;
        total = total
            .checked_add(name.len())
            .and_then(|n| n.checked_add(encoded.len()))
            .ok_or_else(env_too_large)?;
        if total > max_bytes {
            return Err(env_too_large());
        }
        values.insert(name.clone(), canonical);
        bytes.insert(name, encoded);
    }
    Ok((values, bytes))
}

/// Validate one P0.2 env name and reject platform/prototype namespaces.
pub fn validate_env_name(name: &str) -> Result<(), PlatformError> {
    let mut bytes = name.bytes();
    let Some(first) = bytes.next() else {
        return Err(invalid_env());
    };
    if !(first.is_ascii_alphabetic() || first == b'_')
        || bytes.any(|b| !(b.is_ascii_alphanumeric() || b == b'_'))
        || name.starts_with("OPEN_COMPUTE_")
        || name.starts_with("__")
        || name.len() > 128
    {
        return Err(invalid_env());
    }
    Ok(())
}

fn canonical_json(
    value: serde_json::Value,
    depth: usize,
) -> Result<serde_json::Value, PlatformError> {
    if depth > 32 {
        return Err(env_too_large());
    }
    match value {
        serde_json::Value::Array(items) => items
            .into_iter()
            .map(|item| canonical_json(item, depth + 1))
            .collect::<Result<Vec<_>, _>>()
            .map(serde_json::Value::Array),
        serde_json::Value::Object(map) => {
            let mut sorted = BTreeMap::new();
            for (key, value) in map {
                if matches!(key.as_str(), "__proto__" | "prototype" | "constructor") {
                    return Err(PlatformError::new(
                        ErrorCode::BundleInvalid,
                        "var JSON contains a reserved prototype key",
                    ));
                }
                sorted.insert(key, canonical_json(value, depth + 1)?);
            }
            let mut canonical = serde_json::Map::new();
            for (key, value) in sorted {
                canonical.insert(key, value);
            }
            Ok(serde_json::Value::Object(canonical))
        }
        scalar => Ok(scalar),
    }
}

/// Hash nonce and ciphertext for a secret descriptor.
#[must_use]
pub fn ciphertext_sha256(nonce: &[u8], ciphertext: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update((nonce.len() as u64).to_be_bytes());
    digest.update(nonce);
    digest.update((ciphertext.len() as u64).to_be_bytes());
    digest.update(ciphertext);
    hex::encode(digest.finalize())
}

fn invalid_key() -> PlatformError {
    PlatformError::new(
        ErrorCode::VersionInvariantViolation,
        "loader key is not the canonical three-ID form",
    )
}

fn invalid_env() -> PlatformError {
    PlatformError::new(
        ErrorCode::BundleInvalid,
        "environment name is invalid or reserved",
    )
}

fn env_too_large() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceLimitExceeded,
        "version environment exceeds its configured limit",
    )
}

fn binding_invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::VersionInvariantViolation,
        "binding descriptor invariant failed",
    )
}
