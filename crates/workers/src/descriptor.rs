//! Immutable `WorkerCode` descriptor and loader key grammar.

use crate::bundle::{ModuleManifest, WorkerBundleManifest};
use open_compute_core::{
    AccountId, BindingId, BindingKind, CanonicalBindingConfig, CanonicalPermissions, DeploymentId,
    ErrorCode, PlatformError, ResourceId, WorkerId,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

/// Version of the public-only outbound gateway policy in the descriptor.
pub const GLOBAL_OUTBOUND_POLICY_VERSION: u32 = 1;
/// Reserved dynamic module containing the tenant-local R2 facade.
pub const R2_FACADE_MODULE_NAME: &str = "__open_compute_r2_facade__.js";
/// Reserved deterministic main-module wrapper generated for R2 deployments.
pub const R2_WRAPPER_MODULE_NAME: &str = "__open_compute_r2_wrapper__.js";

const R2_FACADE_SOURCE: &[u8] = include_bytes!("../../../runtime/system-workers/r2-facade.js");
const R2_WRAPPER_GENERATOR_SOURCE: &[u8] =
    include_bytes!("../../../runtime/system-workers/r2-wrapper-generator.js");

/// Exact loaded-isolate source identity frozen into an R2 deployment descriptor.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LoadedIsolateInjectionV1 {
    /// Injection plan schema.
    pub schema_version: u32,
    /// Local R2 facade capability version.
    pub r2_facade_capability_version: u32,
    /// SHA-256 of the exact injected facade module source.
    pub r2_facade_sha256: String,
    /// SHA-256 of the exact deterministic wrapper generator source.
    pub r2_wrapper_generator_sha256: String,
}

impl LoadedIsolateInjectionV1 {
    fn for_bindings(bindings: &[BindingDescriptorV1]) -> Option<Self> {
        bindings
            .iter()
            .any(|binding| binding.kind == BindingKind::R2Bucket)
            .then(|| Self {
                schema_version: 1,
                r2_facade_capability_version: 1,
                r2_facade_sha256: hex::encode(Sha256::digest(R2_FACADE_SOURCE)),
                r2_wrapper_generator_sha256: hex::encode(Sha256::digest(
                    R2_WRAPPER_GENERATOR_SOURCE,
                )),
            })
    }
}

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

/// Canonical immutable descriptor for one deployment resource binding.
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
        if name.len() > 64 || resource_spec_generation == 0 {
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

/// Canonical hash input for every runtime-effective deployment field.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkerCodeDescriptorV1 {
    /// Descriptor schema.
    pub schema_version: u32,
    /// Canonical three-segment loader key.
    pub loader_key: String,
    /// Canonical artifact digest.
    pub artifact_sha256: String,
    /// Artifact schema.
    pub artifact_schema_version: u32,
    /// Main module.
    pub main_module: String,
    /// Ordered module descriptors.
    pub ordered_modules: Vec<ModuleManifest>,
    /// Tenant compatibility date.
    pub compatibility_date: String,
    /// Sorted compatibility flags.
    pub compatibility_flags: Vec<String>,
    /// Canonical JSON vars.
    pub canonical_vars: BTreeMap<String, serde_json::Value>,
    /// Sorted secret revision descriptors.
    pub secret_revisions: Vec<SecretDescriptor>,
    /// Canonically sorted immutable resource binding descriptors.
    pub binding_descriptors: Vec<BindingDescriptorV1>,
    /// Exact loaded-isolate facade sources required by product bindings.
    pub loaded_isolate_injection: Option<LoadedIsolateInjectionV1>,
    /// Immutable limits profile document.
    pub limits: serde_json::Value,
    /// Public egress policy version.
    pub global_outbound_policy_version: u32,
    /// Loader host schema.
    pub loader_schema_version: u32,
}

impl WorkerCodeDescriptorV1 {
    /// Build and validate the canonical descriptor.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        account_id: AccountId,
        worker_id: WorkerId,
        deployment_id: DeploymentId,
        artifact_sha256: [u8; 32],
        manifest: &WorkerBundleManifest,
        compatibility_date: String,
        compatibility_flags: Vec<String>,
        canonical_vars: BTreeMap<String, serde_json::Value>,
        mut secret_revisions: Vec<SecretDescriptor>,
        mut binding_descriptors: Vec<BindingDescriptorV1>,
        limits: serde_json::Value,
        loader_schema_version: u32,
    ) -> Result<Self, PlatformError> {
        let compatibility_flags = validate_compatibility(&compatibility_date, compatibility_flags)?;
        secret_revisions.sort_by(|a, b| a.name.as_bytes().cmp(b.name.as_bytes()));
        if secret_revisions
            .windows(2)
            .any(|pair| pair[0].name == pair[1].name)
        {
            return Err(PlatformError::new(
                ErrorCode::SecretInvalid,
                "deployment contains duplicate secret names",
            ));
        }
        let mut env_names: BTreeSet<&str> = canonical_vars.keys().map(String::as_str).collect();
        for secret in &secret_revisions {
            validate_env_name(&secret.name)?;
            if !env_names.insert(&secret.name) {
                return Err(PlatformError::new(
                    ErrorCode::SecretInvalid,
                    "a deployment env name is used by both a var and a secret",
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
                    "deployment binding names are duplicate or conflict with env",
                ));
            }
        }
        let loaded_isolate_injection = LoadedIsolateInjectionV1::for_bindings(&binding_descriptors);
        validate_limits(&limits)?;
        Ok(Self {
            schema_version: 1,
            loader_key: loader_key(account_id, worker_id, deployment_id),
            artifact_sha256: hex::encode(artifact_sha256),
            artifact_schema_version: manifest.schema_version,
            main_module: manifest.main_module.clone(),
            ordered_modules: manifest.modules.clone(),
            compatibility_date,
            compatibility_flags,
            canonical_vars,
            secret_revisions,
            binding_descriptors,
            loaded_isolate_injection,
            limits,
            global_outbound_policy_version: GLOBAL_OUTBOUND_POLICY_VERSION,
            loader_schema_version,
        })
    }

    /// Canonical JSON bytes used as the hash input.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, PlatformError> {
        serde_json::to_vec(self).map_err(|_| {
            PlatformError::new(
                ErrorCode::DeploymentInvariantViolation,
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
pub fn loader_key(
    account_id: AccountId,
    worker_id: WorkerId,
    deployment_id: DeploymentId,
) -> String {
    format!("{account_id}/{worker_id}/{deployment_id}")
}

/// Strictly parse a loader key without percent decoding or alternate forms.
pub fn parse_loader_key(key: &str) -> Result<(AccountId, WorkerId, DeploymentId), PlatformError> {
    let mut parts = key.split('/');
    let account = parts.next().ok_or_else(invalid_key)?;
    let worker = parts.next().ok_or_else(invalid_key)?;
    let deployment = parts.next().ok_or_else(invalid_key)?;
    if parts.next().is_some() || key.contains('%') {
        return Err(invalid_key());
    }
    Ok((
        AccountId::from_str(account).map_err(|_| invalid_key())?,
        WorkerId::from_str(worker).map_err(|_| invalid_key())?,
        DeploymentId::from_str(deployment).map_err(|_| invalid_key())?,
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
            "deployment contains too many vars",
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

/// Validate and sort tenant compatibility metadata against the pinned P0.2 policy.
pub fn validate_compatibility(
    date: &str,
    flags: Vec<String>,
) -> Result<Vec<String>, PlatformError> {
    if !valid_date(date) || !("2022-01-01"..="2026-08-23").contains(&date) {
        return Err(PlatformError::new(
            ErrorCode::CompatibilityUnsupported,
            "compatibility date is outside the pinned runtime policy",
        ));
    }
    let allowed = ["nodejs_compat", "rpc"];
    let mut unique = BTreeSet::new();
    for flag in flags {
        if !allowed.contains(&flag.as_str()) {
            return Err(PlatformError::new(
                ErrorCode::CompatibilityUnsupported,
                "compatibility flag is not allowed by P0.2 policy",
            ));
        }
        unique.insert(flag);
    }
    Ok(unique.into_iter().collect())
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

fn validate_limits(limits: &serde_json::Value) -> Result<(), PlatformError> {
    let Some(object) = limits.as_object() else {
        return Err(invalid_limits());
    };
    if object.len() != 1
        || object.get("profile").and_then(serde_json::Value::as_str) != Some("default")
    {
        return Err(invalid_limits());
    }
    Ok(())
}

fn valid_date(date: &str) -> bool {
    let bytes = date.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes
            .iter()
            .enumerate()
            .any(|(i, b)| i != 4 && i != 7 && !b.is_ascii_digit())
    {
        return false;
    }
    let year = date[0..4].parse::<u32>().ok();
    let month = date[5..7].parse::<u32>().ok();
    let day = date[8..10].parse::<u32>().ok();
    let (Some(year), Some(month), Some(day)) = (year, month, day) else {
        return false;
    };
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let max_day = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    day >= 1 && day <= max_day
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
        ErrorCode::DeploymentInvariantViolation,
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
        "deployment environment exceeds its configured limit",
    )
}

fn invalid_limits() -> PlatformError {
    PlatformError::new(
        ErrorCode::ResourceLimitExceeded,
        "deployment limits profile is invalid",
    )
}

fn binding_invariant() -> PlatformError {
    PlatformError::new(
        ErrorCode::DeploymentInvariantViolation,
        "binding descriptor invariant failed",
    )
}
