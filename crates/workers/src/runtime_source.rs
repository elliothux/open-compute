//! Scoped immutable `RuntimeSource` assembly from `SQLite` and verified artifacts.

use crate::bundle::{BundleLimits, CanonicalBundle, ModuleType};
use crate::descriptor::{
    SecretDescriptor, WorkerCodeDescriptorV1, ciphertext_sha256, parse_loader_key,
};
use base64::Engine as _;
use open_compute_artifacts::{ARTIFACT_KEY_VERSION, ArtifactCache, ArtifactRef, ArtifactStore};
use open_compute_core::{ErrorCode, PlatformError, SecretString};
use open_compute_storage::{DeploymentState, PlatformStorage, WorkerRepository};
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

impl std::fmt::Debug for RuntimeModule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeModule")
            .field("name", &self.name)
            .field("module_type", &self.module_type)
            .field("size", &self.bytes.len())
            .finish()
    }
}

/// Fully verified immutable deployment assembly.
#[derive(Clone)]
pub struct RuntimeSnapshot {
    /// Canonical loader key.
    pub loader_key: String,
    /// Descriptor digest checked before loader get.
    pub worker_code_sha256: String,
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
                let plaintext = self.storage.crypto().decrypt_revision(
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
            main_module: bundle.manifest().main_module.clone(),
            compatibility_date: snapshot.deployment.compatibility_date,
            compatibility_flags: snapshot.deployment.compatibility_flags,
            modules,
            vars,
            secrets,
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
            main_module: &'a str,
            compatibility_date: &'a str,
            compatibility_flags: &'a [String],
            modules: Vec<Module<'a>>,
            env: BTreeMap<&'a str, serde_json::Value>,
            limits: &'a serde_json::Value,
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
        let bytes = serde_json::to_vec(&Payload {
            schema_version: 1,
            loader_key: &snapshot.loader_key,
            worker_code_sha256: &snapshot.worker_code_sha256,
            main_module: &snapshot.main_module,
            compatibility_date: &snapshot.compatibility_date,
            compatibility_flags: &snapshot.compatibility_flags,
            modules,
            env,
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
