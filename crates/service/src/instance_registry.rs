//! Explicit `ocd.toml` instance registration.

use open_compute_core::{
    DaemonGatewayConfig, DaemonServerConfig, ErrorCode, InstanceId, InstanceSelector,
    ObjectStorageConfig, PlatformError,
};
use open_compute_storage::{ensure_dir_secure, inspect_control_db};
use serde::{Deserialize, Serialize};
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;
use std::time::SystemTime;

/// System-scoped OCD directory.
pub const SYSTEM_REGISTRY_ROOT: &str = "/var/lib/open-compute";
const MANIFEST_NAME: &str = "ocd.toml";
const INSTANCES_DIR_NAME: &str = "instances";
const MAX_MANIFEST_BYTES: u64 = 256 * 1024;
mod manifest;
mod online;
mod restore;
mod user_home;
pub(crate) use manifest::manifest_digest;
use manifest::{read_manifest, write_manifest};
pub(crate) use user_home::user_home_for_uid;

/// Whether one OCD daemon is managed as a system or user service.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceScope {
    /// systemd system unit / launch daemon.
    System,
    /// systemd user unit / launch agent.
    User,
}

impl ServiceScope {
    /// Stable token used in service identifiers.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
        }
    }
}

/// Non-secret object authority resolved from the current instance config.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RegisteredObjectAuthority {
    /// Local object bytes at `data.path/objects`.
    Local,
    /// External S3 authority, which purge always retains.
    S3 {
        /// Configured endpoint without credentials.
        endpoint: String,
        /// Configured bucket name.
        bucket: String,
        /// Internal platform key prefix.
        prefix: String,
        /// Tenant R2 key prefix.
        r2_prefix: String,
    },
}

/// One validated registered instance.
///
/// Only `config` and `autostart` are persisted. Every other field is a
/// current, derived view used by existing operator workflows.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstanceRecord {
    /// Instance identity read from `control.sqlite`.
    pub instance_id: String,
    /// Optional mutable operator-facing name from `compute.toml`.
    pub name: Option<String>,
    /// Canonical absolute configuration path.
    pub canonical_config_path: String,
    /// SHA-256 of the currently loaded configuration bytes.
    pub config_sha256: String,
    /// Exact absolute data directory resolved from `[data].path`.
    pub data_path: String,
    /// Non-secret object authority resolved from the current config.
    pub object_authority: RegisteredObjectAuthority,
    /// Public base domain declared by this instance, when present.
    pub public_base_domain: Option<String>,
    /// OCD service scope.
    pub service_scope: ServiceScope,
    /// Instance authority creation time in Unix milliseconds.
    pub created_at: u64,
    /// Persistent startup intent from `ocd.toml`.
    pub autostart: bool,
}

impl InstanceRecord {
    /// Parse the identity read from instance storage.
    pub fn instance_id(&self) -> Result<InstanceId, PlatformError> {
        InstanceId::from_str(&self.instance_id)
    }

    /// Canonical config path as [`Path`].
    #[must_use]
    pub fn config_path(&self) -> &Path {
        Path::new(&self.canonical_config_path)
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct OcdManifest {
    #[serde(default)]
    server: DaemonServerConfig,
    #[serde(default)]
    gateway: Option<DaemonGatewayConfig>,
    #[serde(default)]
    artifacts: DaemonArtifactsConfig,
    #[serde(default)]
    metrics: DaemonMetricsConfig,
    #[serde(default)]
    instances: Vec<InstanceRegistration>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub(crate) struct DaemonArtifactsConfig {
    pub(crate) max_concurrent_requests: u32,
}

impl Default for DaemonArtifactsConfig {
    fn default() -> Self {
        Self {
            max_concurrent_requests: 16,
        }
    }
}

impl DaemonArtifactsConfig {
    fn validate(&self) -> Result<(), PlatformError> {
        if !(1..=1024).contains(&self.max_concurrent_requests) {
            return Err(manifest_invalid(
                "artifacts.max_concurrent_requests must be between 1 and 1024",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub(crate) struct DaemonMetricsConfig {
    pub(crate) max_series: u64,
}

impl Default for DaemonMetricsConfig {
    fn default() -> Self {
        Self { max_series: 1024 }
    }
}

impl DaemonMetricsConfig {
    fn validate(&self) -> Result<(), PlatformError> {
        if self.max_series < crate::metrics::REQUIRED_SERIES {
            return Err(manifest_invalid(
                "metrics.max_series cannot contain the required fixed series set",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct InstanceRegistration {
    config: PathBuf,
    autostart: bool,
}

/// Read/write view over the user and system `ocd.toml` manifests.
#[derive(Clone, Debug)]
pub struct InstanceRegistry {
    system_root: PathBuf,
    user_root: PathBuf,
}

impl InstanceRegistry {
    /// Production OCD roots for the current process.
    pub fn production() -> Result<Self, PlatformError> {
        #[cfg(any(test, feature = "test-support"))]
        if let Some(root) = std::env::var_os("OPEN_COMPUTE_TEST_OCD_ROOT") {
            let root = PathBuf::from(root);
            if !root.is_absolute() {
                return Err(manifest_invalid("test OCD root must be absolute"));
            }
            return Ok(Self::with_roots(root.join("system"), root.join("user")));
        }
        Ok(Self {
            system_root: PathBuf::from(SYSTEM_REGISTRY_ROOT),
            user_root: default_user_ocd_root()?,
        })
    }

    /// Test or fixture registry with explicit OCD roots.
    #[must_use]
    pub fn with_roots(system_root: PathBuf, user_root: PathBuf) -> Self {
        Self {
            system_root,
            user_root,
        }
    }

    /// OCD root directory for `scope`.
    #[must_use]
    pub fn root_for(&self, scope: ServiceScope) -> &Path {
        match scope {
            ServiceScope::System => &self.system_root,
            ServiceScope::User => &self.user_root,
        }
    }

    /// Read the one shared listener configuration for this OCD scope.
    pub fn server_config(&self, scope: ServiceScope) -> Result<DaemonServerConfig, PlatformError> {
        Ok(read_manifest(self.root_for(scope))?.server)
    }

    /// Read the one optional shared public gateway configuration.
    pub fn gateway_config(
        &self,
        scope: ServiceScope,
    ) -> Result<Option<DaemonGatewayConfig>, PlatformError> {
        Ok(read_manifest(self.root_for(scope))?.gateway)
    }

    pub(crate) fn artifacts_config(
        &self,
        scope: ServiceScope,
    ) -> Result<DaemonArtifactsConfig, PlatformError> {
        Ok(read_manifest(self.root_for(scope))?.artifacts)
    }

    pub(crate) fn metrics_config(
        &self,
        scope: ServiceScope,
    ) -> Result<DaemonMetricsConfig, PlatformError> {
        Ok(read_manifest(self.root_for(scope))?.metrics)
    }

    /// List only the instances explicitly registered in one OCD scope.
    pub fn list_scope(&self, scope: ServiceScope) -> Result<Vec<InstanceRecord>, PlatformError> {
        let root = self.root_for(scope);
        let manifest = read_manifest(root)?;
        if manifest.instances.is_empty() {
            return Ok(Vec::new());
        }
        validate_scope_owner(scope, root)?;
        let mut records = Vec::with_capacity(manifest.instances.len());
        for registration in manifest.instances {
            let config = resolve_registered_config(root, &registration.config)?;
            records.push(load_record(root, &config, scope, registration.autostart)?);
        }
        validate_unique_records(&records)?;
        records.sort_by(|a, b| a.instance_id.cmp(&b.instance_id));
        Ok(records)
    }

    /// Resolve one instance only within the explicitly selected OCD scope.
    pub(crate) fn get_scope(
        &self,
        scope: ServiceScope,
        selector: &InstanceSelector,
    ) -> Result<InstanceRecord, PlatformError> {
        self.list_scope(scope)?
            .into_iter()
            .find(|record| {
                record.instance_id == selector.as_str()
                    || record.name.as_deref() == Some(selector.as_str())
            })
            .ok_or_else(|| {
                PlatformError::new(ErrorCode::InstanceNotFound, "instance is not registered")
            })
    }

    /// Look up a configuration only in the explicitly selected OCD scope.
    pub(crate) fn get_by_config_scope(
        &self,
        scope: ServiceScope,
        config: &Path,
    ) -> Result<InstanceRecord, PlatformError> {
        self.list_scope(scope)?
            .into_iter()
            .find(|record| record.config_path() == config)
            .ok_or_else(|| {
                PlatformError::new(
                    ErrorCode::InstanceNotFound,
                    "instance configuration is not registered in this OCD scope",
                )
            })
    }

    /// Resolve the registration scope without opening instance storage.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn scope_for_config(&self, config: &Path) -> Result<ServiceScope, PlatformError> {
        let mut matched = None;
        for scope in [ServiceScope::System, ServiceScope::User] {
            let root = self.root_for(scope);
            for registration in read_manifest(root)?.instances {
                if resolve_registered_config(root, &registration.config)? != config {
                    continue;
                }
                if matched.replace(scope).is_some() {
                    return Err(manifest_invalid(
                        "multiple instance registrations reference the active configuration",
                    ));
                }
            }
        }
        matched.ok_or_else(|| {
            PlatformError::new(
                ErrorCode::InstanceNotFound,
                "active instance configuration is not registered in ocd.toml",
            )
        })
    }

    /// Register an initialized instance with autostart enabled.
    pub fn register(
        &self,
        canonical_config_path: &Path,
        scope: ServiceScope,
        now: SystemTime,
    ) -> Result<InstanceRecord, PlatformError> {
        self.register_inner(canonical_config_path, scope, None, true, now)
    }

    /// Register the first instance while writing its shared listener to the new OCD manifest.
    pub(crate) fn register_first_with_server(
        &self,
        canonical_config_path: &Path,
        scope: ServiceScope,
        server: DaemonServerConfig,
        now: SystemTime,
    ) -> Result<InstanceRecord, PlatformError> {
        self.register_inner(canonical_config_path, scope, Some(server), true, now)
    }

    fn register_inner(
        &self,
        canonical_config_path: &Path,
        scope: ServiceScope,
        server: Option<DaemonServerConfig>,
        autostart: bool,
        _now: SystemTime,
    ) -> Result<InstanceRecord, PlatformError> {
        let root = self.root_for(scope);
        if let Some(server) = &server {
            server.validate()?;
            match fs::symlink_metadata(root.join(MANIFEST_NAME)) {
                Ok(_) => {
                    return Err(manifest_invalid(
                        "first setup refuses to overwrite ocd.toml",
                    ));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(manifest_invalid("failed to inspect ocd.toml")),
            }
        }
        ensure_ocd_root(root)?;
        validate_scope_owner(scope, root)?;
        let canonical_root = canonical_existing_dir(root)?;
        let record = load_record(&canonical_root, canonical_config_path, scope, autostart)?;
        let mut manifest = read_manifest(&canonical_root)?;
        if let Some(server) = server {
            manifest.server = server;
        }
        if manifest.instances.iter().any(|entry| {
            resolve_registered_config(&canonical_root, &entry.config)
                .is_ok_and(|path| path == record.config_path())
        }) {
            return Err(manifest_invalid(
                "instance configuration is already registered",
            ));
        }
        let mut records = self.list_scope(scope)?;
        records.push(record.clone());
        validate_unique_records(&records)?;
        manifest.instances.push(InstanceRegistration {
            config: record.config_path().to_owned(),
            autostart,
        });
        write_manifest(&canonical_root, &manifest)?;
        Ok(record)
    }

    /// Remove the exact manifest entry represented by an already validated record.
    pub(crate) fn remove_record(&self, record: &InstanceRecord) -> Result<(), PlatformError> {
        let root = self.root_for(record.service_scope);
        let canonical_root = canonical_existing_dir(root)?;
        let mut manifest = read_manifest(&canonical_root)?;
        let before = manifest.instances.len();
        manifest.instances.retain(|entry| {
            resolve_registered_config(&canonical_root, &entry.config)
                .map_or(true, |path| path != record.config_path())
        });
        if manifest.instances.len() == before {
            return Err(PlatformError::new(
                ErrorCode::InstanceNotFound,
                "requested instance is not registered",
            ));
        }
        write_manifest(&canonical_root, &manifest)?;
        Ok(())
    }

    /// Reload and validate the registered config and its storage identity.
    pub fn validate_registered_config(&self, record: &InstanceRecord) -> Result<(), PlatformError> {
        let refreshed = load_record(
            self.root_for(record.service_scope),
            record.config_path(),
            record.service_scope,
            record.autostart,
        )?;
        if refreshed.instance_id != record.instance_id {
            return Err(PlatformError::new(
                ErrorCode::InstanceRegistryInvalid,
                "registered instance storage identity changed",
            ));
        }
        Ok(())
    }

    /// Validate one config's resolved data root against its OCD root.
    pub fn validate_config_data_path(
        &self,
        scope: ServiceScope,
        canonical_config_path: &Path,
    ) -> Result<PathBuf, PlatformError> {
        let loaded =
            crate::config_load::load_platform_config_from(canonical_config_path, Path::new("/"))?;
        validate_instance_data_path(self.root_for(scope), &loaded.config.data.path)
    }
}

fn load_record(
    ocd_root: &Path,
    config_path: &Path,
    scope: ServiceScope,
    autostart: bool,
) -> Result<InstanceRecord, PlatformError> {
    if !config_path.is_absolute() {
        return Err(PlatformError::new(
            ErrorCode::ConfigPathInvalid,
            "instance configuration path must be absolute",
        ));
    }
    let loaded = crate::config_load::load_platform_config_from(config_path, Path::new("/"))
        .map_err(|_| manifest_invalid("registered instance config is unavailable or invalid"))?;
    let data_path = validate_instance_data_path(ocd_root, &loaded.config.data.path)?;
    let (_, identity) = inspect_control_db(
        &data_path.join("control.sqlite"),
        loaded.config.data.sqlite_busy_timeout_ms,
    )
    .map_err(|_| {
        PlatformError::new(
            ErrorCode::InstanceRegistryInvalid,
            "registered instance data is not initialized or its identity is invalid",
        )
    })?;
    let instance_id = identity.instance_id;
    let object_authority = match &loaded.config.object_storage {
        ObjectStorageConfig::Local(_) => RegisteredObjectAuthority::Local,
        ObjectStorageConfig::S3(s3) => RegisteredObjectAuthority::S3 {
            endpoint: s3.endpoint.to_string(),
            bucket: s3.bucket.clone(),
            prefix: s3.prefix.clone(),
            r2_prefix: s3.r2_prefix.clone(),
        },
    };
    let created_at = u64::try_from(identity.created_at_ms).map_err(|_| {
        PlatformError::new(
            ErrorCode::InstanceRegistryInvalid,
            "stored instance creation time is invalid",
        )
    })?;
    Ok(InstanceRecord {
        instance_id: instance_id.to_string(),
        name: loaded.config.instance.name.map(|name| name.to_string()),
        canonical_config_path: loaded.path.to_string_lossy().into_owned(),
        config_sha256: loaded.sha256,
        data_path: data_path.to_string_lossy().into_owned(),
        object_authority,
        public_base_domain: loaded
            .config
            .public_gateway
            .map(|gateway| gateway.base_domain),
        service_scope: scope,
        created_at,
        autostart,
    })
}

fn resolve_registered_config(root: &Path, path: &Path) -> Result<PathBuf, PlatformError> {
    let candidate = if path.is_absolute() {
        path.to_owned()
    } else {
        root.join(path)
    };
    let parent = candidate
        .parent()
        .ok_or_else(|| manifest_invalid("registered config path has no parent"))?;
    let parent = fs::canonicalize(parent)
        .map_err(|_| manifest_invalid("registered config parent is unavailable"))?;
    let leaf = candidate
        .file_name()
        .ok_or_else(|| manifest_invalid("registered config path is invalid"))?;
    Ok(parent.join(leaf))
}

fn validate_unique_records(records: &[InstanceRecord]) -> Result<(), PlatformError> {
    for (index, left) in records.iter().enumerate() {
        for right in &records[index + 1..] {
            if left.config_path() == right.config_path() {
                return Err(manifest_invalid(
                    "ocd.toml contains the same instance config more than once",
                ));
            }
            if left.instance_id == right.instance_id {
                return Err(manifest_invalid(
                    "different instance configs expose the same stored identity",
                ));
            }
            if left.name.is_some() && left.name == right.name {
                return Err(manifest_invalid(
                    "registered instance names must be unique within OCD_DIR",
                ));
            }
            if let (Some(left_domain), Some(right_domain)) =
                (&left.public_base_domain, &right.public_base_domain)
                && public_domains_overlap(left_domain, right_domain)
            {
                return Err(manifest_invalid(
                    "registered public gateway base domains overlap",
                ));
            }
            if let (
                RegisteredObjectAuthority::S3 {
                    endpoint: left_endpoint,
                    bucket: left_bucket,
                    prefix: left_prefix,
                    r2_prefix: left_r2,
                },
                RegisteredObjectAuthority::S3 {
                    endpoint: right_endpoint,
                    bucket: right_bucket,
                    prefix: right_prefix,
                    r2_prefix: right_r2,
                },
            ) = (&left.object_authority, &right.object_authority)
                && left_endpoint == right_endpoint
                && left_bucket == right_bucket
                && [left_prefix, left_r2].into_iter().any(|left_prefix| {
                    [right_prefix, right_r2].into_iter().any(|right_prefix| {
                        left_prefix.starts_with(right_prefix)
                            || right_prefix.starts_with(left_prefix)
                    })
                })
            {
                return Err(manifest_invalid(
                    "registered S3 object key prefixes overlap",
                ));
            }
            let left_data = Path::new(&left.data_path);
            let right_data = Path::new(&right.data_path);
            if left_data == right_data
                || left_data.starts_with(right_data)
                || right_data.starts_with(left_data)
            {
                return Err(manifest_invalid(
                    "registered instance data directories overlap",
                ));
            }
        }
    }
    Ok(())
}

pub(crate) fn public_domains_overlap(left: &str, right: &str) -> bool {
    left == right || left.ends_with(&format!(".{right}")) || right.ends_with(&format!(".{left}"))
}

/// Validate and normalize an instance data root against its OCD root.
pub fn validate_instance_data_path(
    ocd_root: &Path,
    data_path: &Path,
) -> Result<PathBuf, PlatformError> {
    if !ocd_root.is_absolute() || !data_path.is_absolute() {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "OCD and instance data paths must be absolute",
        ));
    }
    let normalized_ocd = normalize_real_path(ocd_root)?;
    let normalized_data = normalize_real_path_checked(data_path, Some(&normalized_ocd))?;
    if normalized_data.starts_with(&normalized_ocd) {
        let instances = normalized_ocd.join(INSTANCES_DIR_NAME);
        if normalized_data == instances || !normalized_data.starts_with(&instances) {
            return Err(PlatformError::new(
                ErrorCode::PathInvalid,
                "instance data inside OCD_DIR must be a strict child of OCD_DIR/instances",
            ));
        }
    } else if normalized_ocd.starts_with(&normalized_data) {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "instance data directory must not contain OCD_DIR",
        ));
    }
    Ok(normalized_data)
}

pub(crate) fn normalize_real_path(path: &Path) -> Result<PathBuf, PlatformError> {
    normalize_real_path_checked(path, None)
}

fn normalize_real_path_checked(
    path: &Path,
    protected_root: Option<&Path>,
) -> Result<PathBuf, PlatformError> {
    if !path.is_absolute() {
        return Err(PlatformError::new(
            ErrorCode::PathInvalid,
            "instance path must be absolute",
        ));
    }
    let mut normalized = PathBuf::new();
    let mut missing = false;
    let mut components = path.components().peekable();
    while let Some(component) = components.next() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if missing || !normalized.pop() {
                    return Err(PlatformError::new(
                        ErrorCode::PathInvalid,
                        "instance path contains invalid parent traversal",
                    ));
                }
            }
            Component::Normal(value) => {
                normalized.push(value);
                if missing {
                    continue;
                }
                match fs::symlink_metadata(&normalized) {
                    Ok(metadata) if metadata.file_type().is_symlink() => {
                        if components.peek().is_none()
                            || protected_root.is_some_and(|root| normalized.starts_with(root))
                        {
                            return Err(PlatformError::new(
                                ErrorCode::PathInvalid,
                                "instance path must not traverse a protected symbolic link",
                            ));
                        }
                        normalized = fs::canonicalize(&normalized).map_err(|_| {
                            PlatformError::new(
                                ErrorCode::PathInvalid,
                                "failed to resolve instance path",
                            )
                        })?;
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => missing = true,
                    Err(_) => {
                        return Err(PlatformError::new(
                            ErrorCode::PathInvalid,
                            "failed to inspect instance path",
                        ));
                    }
                }
            }
        }
    }
    Ok(normalized)
}

fn ensure_ocd_root(root: &Path) -> Result<(), PlatformError> {
    if !root.is_absolute() {
        return Err(manifest_invalid("OCD_DIR must be absolute"));
    }
    if let Some(parent) = root.parent() {
        fs::create_dir_all(parent)
            .map_err(|_| manifest_invalid("failed to create OCD_DIR parent"))?;
    }
    ensure_dir_secure(root).map_err(|_| manifest_invalid("failed to secure OCD_DIR"))?;
    fs::create_dir_all(root.join(INSTANCES_DIR_NAME))
        .map_err(|_| manifest_invalid("failed to create OCD_DIR/instances"))?;
    fs::set_permissions(
        root.join(INSTANCES_DIR_NAME),
        fs::Permissions::from_mode(0o700),
    )
    .map_err(|_| manifest_invalid("failed to secure OCD_DIR/instances"))
}

fn canonical_existing_dir(path: &Path) -> Result<PathBuf, PlatformError> {
    fs::canonicalize(path).map_err(|_| manifest_invalid("OCD_DIR is unavailable"))
}

fn validate_scope_owner(scope: ServiceScope, root: &Path) -> Result<(), PlatformError> {
    match scope {
        ServiceScope::User => Ok(()),
        ServiceScope::System => {
            let owner = fs::symlink_metadata(root)
                .map_err(|_| manifest_invalid("system OCD_DIR is unavailable"))?;
            if !owner.is_dir() || owner.file_type().is_symlink() || owner.uid() == 0 {
                return Err(manifest_invalid(
                    "system OCD_DIR must have a non-root owner",
                ));
            }
            Ok(())
        }
    }
}

pub(crate) fn current_binary_path() -> Result<PathBuf, PlatformError> {
    let path = std::env::current_exe().map_err(|_| {
        PlatformError::new(
            ErrorCode::PlatformUnavailable,
            "failed to resolve the current ocd executable path",
        )
    })?;
    Ok(path.canonicalize().unwrap_or(path))
}

fn default_user_ocd_root() -> Result<PathBuf, PlatformError> {
    Ok(user_home_for_uid()?.join(".open-compute"))
}

fn manifest_invalid(message: &'static str) -> PlatformError {
    PlatformError::new(ErrorCode::InstanceRegistryInvalid, message)
}

#[cfg(test)]
mod tests;
