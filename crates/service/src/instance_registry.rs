//! Local operator instance registry for managed ocd deployments.

use open_compute_core::{
    ErrorCode, InstanceId, InstanceSelector, ObjectStorageConfig, PlatformError,
    digest_canonical_config_path,
};
use open_compute_storage::{atomic_write, ensure_dir_secure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Current on-disk registry schema.
pub const REGISTRY_SCHEMA_VERSION: u32 = 2;

/// System-scope registry root on Unix hosts.
pub const SYSTEM_REGISTRY_ROOT: &str = "/var/lib/open-compute-registry";

/// Whether an instance is managed as a system or user service.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceScope {
    /// systemd system unit / launch daemon.
    System,
    /// systemd user unit / launch agent.
    User,
}

/// Persisted non-secret object-authority location needed for lifecycle plans.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RegisteredObjectAuthority {
    /// Local object bytes at one exact absolute root.
    Local {
        /// Exact local object root.
        path: String,
    },
    /// External S3 authority, which purge always retains.
    S3 {
        /// Configured endpoint without credentials.
        endpoint: String,
        /// Configured bucket name.
        bucket: String,
    },
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

/// One registered local operator instance.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceRecord {
    /// Registry schema version.
    pub schema_version: u32,
    /// Persisted short instance ID.
    pub instance_id: String,
    /// Hex-encoded SHA-256 digest of the canonical config path.
    pub digest_sha256: String,
    /// Canonical absolute configuration path.
    pub canonical_config_path: String,
    /// SHA-256 of the exact configuration bytes at registration time.
    pub config_sha256: String,
    /// Exact absolute data directory retained or purged with this instance.
    pub data_path: String,
    /// Non-secret object authority location captured at registration.
    pub object_authority: RegisteredObjectAuthority,
    /// Absolute `ocd` executable that owns this service registration.
    pub binary_path: String,
    /// Service manager scope.
    pub service_scope: ServiceScope,
    /// Non-root account used by a system service; absent for user services.
    pub service_user: Option<String>,
    /// OS service identifier that embeds the instance ID.
    pub service_identifier: String,
    /// Unix epoch milliseconds when the record was created.
    pub created_at: u64,
}

impl InstanceRecord {
    /// Parse and verify the stored identity against the digest and path.
    pub fn instance_id(&self) -> Result<InstanceId, PlatformError> {
        let digest = decode_digest(&self.digest_sha256)?;
        let id = InstanceId::from_short_and_digest(&self.instance_id, digest)?;
        let path = PathBuf::from(&self.canonical_config_path);
        let expected = digest_canonical_config_path(&path)?;
        if expected != digest {
            return Err(PlatformError::new(
                ErrorCode::InstanceRegistryInvalid,
                "instance registry digest does not match the canonical config path",
            ));
        }
        Ok(id)
    }

    /// Canonical config path as [`Path`].
    #[must_use]
    pub fn config_path(&self) -> &Path {
        Path::new(&self.canonical_config_path)
    }

    /// Registered executable owner as [`Path`].
    #[must_use]
    pub fn binary_path(&self) -> &Path {
        Path::new(&self.binary_path)
    }
}

/// Readable view over the system and user instance registries.
#[derive(Clone, Debug)]
pub struct InstanceRegistry {
    system_root: PathBuf,
    user_root: PathBuf,
}

impl InstanceRegistry {
    /// Production registry roots for the current process.
    pub fn production() -> Result<Self, PlatformError> {
        Ok(Self {
            system_root: PathBuf::from(SYSTEM_REGISTRY_ROOT),
            user_root: default_user_registry_root()?,
        })
    }

    /// Test or fixture registry with explicit roots.
    #[must_use]
    pub fn with_roots(system_root: PathBuf, user_root: PathBuf) -> Self {
        Self {
            system_root,
            user_root,
        }
    }

    /// Root directory for `scope`.
    #[must_use]
    pub fn root_for(&self, scope: ServiceScope) -> &Path {
        match scope {
            ServiceScope::System => &self.system_root,
            ServiceScope::User => &self.user_root,
        }
    }

    /// List every readable record the current caller can manage.
    pub fn list(&self) -> Result<Vec<InstanceRecord>, PlatformError> {
        let mut records = self.list_scope(ServiceScope::System)?;
        records.extend(self.list_scope(ServiceScope::User)?);
        records.sort_by(|a, b| a.instance_id.cmp(&b.instance_id));
        Ok(records)
    }

    pub(crate) fn list_scope(
        &self,
        scope: ServiceScope,
    ) -> Result<Vec<InstanceRecord>, PlatformError> {
        let mut records = Vec::new();
        self.read_root(scope, &mut records)?;
        records.sort_by(|a, b| a.instance_id.cmp(&b.instance_id));
        Ok(records)
    }

    /// Look up one record by exact short ID.
    pub fn get(&self, selector: &InstanceSelector) -> Result<InstanceRecord, PlatformError> {
        let mut matches = Vec::new();
        for record in self.list()? {
            if record.instance_id == selector.as_str() {
                matches.push(record);
            }
        }
        match matches.len() {
            1 => Ok(matches.remove(0)),
            0 => Err(PlatformError::new(
                ErrorCode::InstanceNotFound,
                "requested instance is not registered",
            )),
            _ => Err(PlatformError::new(
                ErrorCode::InstanceRegistryInvalid,
                "instance registry contains duplicate short IDs",
            )),
        }
    }

    /// Register a new instance for `canonical_config_path`, extending the short ID on collision.
    pub fn register(
        &self,
        canonical_config_path: &Path,
        scope: ServiceScope,
        now: SystemTime,
    ) -> Result<InstanceRecord, PlatformError> {
        self.register_with_service_user(canonical_config_path, scope, None, now)
    }

    /// Register an instance with the validated account required by system scope.
    pub fn register_with_service_user(
        &self,
        canonical_config_path: &Path,
        scope: ServiceScope,
        service_user: Option<&str>,
        now: SystemTime,
    ) -> Result<InstanceRecord, PlatformError> {
        let binary_path = current_binary_path()?;
        self.register_owned(
            canonical_config_path,
            &binary_path,
            scope,
            service_user,
            now,
        )
    }

    /// Register an instance owned by an exact executable path.
    pub fn register_owned(
        &self,
        canonical_config_path: &Path,
        binary_path: &Path,
        scope: ServiceScope,
        service_user: Option<&str>,
        now: SystemTime,
    ) -> Result<InstanceRecord, PlatformError> {
        if !canonical_config_path.is_absolute() {
            return Err(PlatformError::new(
                ErrorCode::ConfigPathInvalid,
                "registry registration requires a canonical absolute config path",
            ));
        }
        if !binary_path.is_absolute() {
            return Err(PlatformError::new(
                ErrorCode::InstanceRegistryInvalid,
                "instance executable path must be absolute",
            ));
        }
        let (data_path, object_authority, config_sha256) =
            registration_locations(canonical_config_path)?;
        match (scope, service_user) {
            (ServiceScope::System, Some(user)) if !user.is_empty() && user != "root" => {}
            (ServiceScope::System, _) => {
                return Err(PlatformError::new(
                    ErrorCode::InstanceRegistryInvalid,
                    "system instances require an explicit non-root service account",
                ));
            }
            (ServiceScope::User, None) => {}
            (ServiceScope::User, Some(_)) => {
                return Err(PlatformError::new(
                    ErrorCode::InstanceRegistryInvalid,
                    "user instances must run as the current user",
                ));
            }
        }
        let existing = self.list()?;
        if let Some(found) = existing
            .iter()
            .find(|record| record.canonical_config_path == canonical_config_path.to_string_lossy())
        {
            if found.binary_path() != binary_path
                || found.service_scope != scope
                || found.service_user.as_deref() != service_user
            {
                return Err(PlatformError::new(
                    ErrorCode::InstanceRegistryInvalid,
                    "configuration is already registered to a different executable or service scope",
                ));
            }
            self.validate_registered_config(found)?;
            return Ok(found.clone());
        }

        let occupied: Vec<(String, [u8; 32])> = existing
            .iter()
            .map(|record| {
                Ok((
                    record.instance_id.clone(),
                    decode_digest(&record.digest_sha256)?,
                ))
            })
            .collect::<Result<Vec<_>, PlatformError>>()?;
        let candidate = allocate_instance_id(canonical_config_path, &occupied)?;

        let created_at = open_compute_core::unix_time_ms(now).ok_or_else(|| {
            PlatformError::new(
                ErrorCode::InstanceRegistryInvalid,
                "system clock is outside the supported Unix timestamp range",
            )
        })?;
        let created_at = u64::try_from(created_at).map_err(|_| {
            PlatformError::new(
                ErrorCode::InstanceRegistryInvalid,
                "instance creation timestamp overflows",
            )
        })?;
        let record = InstanceRecord {
            schema_version: REGISTRY_SCHEMA_VERSION,
            instance_id: candidate.as_str().to_owned(),
            digest_sha256: hex::encode(candidate.digest()),
            canonical_config_path: canonical_config_path.to_string_lossy().into_owned(),
            config_sha256,
            data_path,
            object_authority,
            binary_path: binary_path.to_string_lossy().into_owned(),
            service_scope: scope,
            service_user: service_user.map(str::to_owned),
            service_identifier: format!("dev.open-compute.ocd.{}", candidate.as_str()),
            created_at,
        };
        self.write_record(&record)?;
        Ok(record)
    }

    /// Remove a stopped instance registration without deleting config or data.
    pub fn remove(&self, selector: &InstanceSelector) -> Result<InstanceRecord, PlatformError> {
        let record = self.get(selector)?;
        let path = self.record_path(record.service_scope, &record.instance_id);
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(PlatformError::new(
                    ErrorCode::InstanceRegistryInvalid,
                    "instance registry entry must not be a symlink",
                ));
            }
            Ok(_) => fs::remove_file(&path).map_err(|_| {
                PlatformError::new(
                    ErrorCode::InstanceRegistryInvalid,
                    "failed to remove instance registry entry",
                )
            })?,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {
                return Err(PlatformError::new(
                    ErrorCode::InstanceRegistryInvalid,
                    "failed to inspect instance registry entry",
                ));
            }
        }
        Ok(record)
    }

    /// Verify that the registered config bytes and owned local roots are unchanged.
    pub fn validate_registered_config(&self, record: &InstanceRecord) -> Result<(), PlatformError> {
        let bytes = fs::read(record.config_path()).map_err(|_| {
            PlatformError::new(
                ErrorCode::ConfigPathInvalid,
                "registered instance configuration is unavailable",
            )
        })?;
        if hex::encode(Sha256::digest(bytes)) != record.config_sha256 {
            return Err(PlatformError::new(
                ErrorCode::InstanceRegistryInvalid,
                "registered instance configuration changed; stop and unregister it before registering the new configuration",
            ));
        }
        let (data_path, object_authority, _) =
            current_registration_locations(record.config_path())?;
        if data_path != record.data_path || object_authority != record.object_authority {
            return Err(PlatformError::new(
                ErrorCode::InstanceRegistryInvalid,
                "registered instance owned paths changed; stop and unregister it before registering the new configuration",
            ));
        }
        Ok(())
    }

    fn write_record(&self, record: &InstanceRecord) -> Result<(), PlatformError> {
        let root = self.root_for(record.service_scope);
        ensure_registry_tree(root)?;
        let path = self.record_path(record.service_scope, &record.instance_id);
        if path.exists() {
            return Err(PlatformError::new(
                ErrorCode::InstanceRegistryInvalid,
                "refusing to overwrite an existing instance registry entry",
            ));
        }
        let body = serde_json::to_vec_pretty(record).map_err(|_| {
            PlatformError::new(
                ErrorCode::InstanceRegistryInvalid,
                "failed to encode instance registry entry",
            )
        })?;
        atomic_write(&path, &body).map_err(|_| {
            PlatformError::new(
                ErrorCode::InstanceRegistryInvalid,
                "failed to write instance registry entry",
            )
        })?;
        if matches!(record.service_scope, ServiceScope::System) {
            // System registry records are secret-free and remain root-owned.
            // Read/execute access lets the configured non-root daemon recover
            // its persisted ID and scope without transferring registry write
            // authority to one service account.
            fs::set_permissions(root, fs::Permissions::from_mode(0o755)).map_err(|_| {
                PlatformError::new(
                    ErrorCode::InstanceRegistryInvalid,
                    "failed to make the system registry readable by managed services",
                )
            })?;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).map_err(|_| {
                PlatformError::new(
                    ErrorCode::InstanceRegistryInvalid,
                    "failed to make a system registry entry readable by managed services",
                )
            })?;
            File::open(&path)
                .and_then(|file| file.sync_all())
                .map_err(|_| {
                    PlatformError::new(
                        ErrorCode::InstanceRegistryInvalid,
                        "failed to persist system registry permissions",
                    )
                })?;
        }
        Ok(())
    }

    fn record_path(&self, scope: ServiceScope, instance_id: &str) -> PathBuf {
        self.root_for(scope).join(format!("{instance_id}.json"))
    }

    fn read_root(
        &self,
        scope: ServiceScope,
        out: &mut Vec<InstanceRecord>,
    ) -> Result<(), PlatformError> {
        let root = self.root_for(scope);
        match fs::symlink_metadata(root) {
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => {
                return Err(PlatformError::new(
                    ErrorCode::InstanceRegistryInvalid,
                    "failed to inspect instance registry root",
                ));
            }
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(PlatformError::new(
                    ErrorCode::InstanceRegistryInvalid,
                    "instance registry root must not be a symlink",
                ));
            }
            Ok(meta) if !meta.file_type().is_dir() => {
                return Err(PlatformError::new(
                    ErrorCode::InstanceRegistryInvalid,
                    "instance registry root must be a directory",
                ));
            }
            Ok(meta) => {
                let mode = meta.permissions().mode() & 0o777;
                if mode & 0o022 != 0 {
                    return Err(PlatformError::new(
                        ErrorCode::InstanceRegistryInvalid,
                        "instance registry root must not be group or world writable",
                    ));
                }
                if matches!(scope, ServiceScope::System) && mode & 0o055 != 0o055 {
                    return Err(PlatformError::new(
                        ErrorCode::InstanceRegistryInvalid,
                        "system registry root must be readable by managed services",
                    ));
                }
            }
        }
        let entries = fs::read_dir(root).map_err(|_| {
            PlatformError::new(
                ErrorCode::InstanceRegistryInvalid,
                "failed to read instance registry root",
            )
        })?;
        for entry in entries {
            let entry = entry.map_err(|_| {
                PlatformError::new(
                    ErrorCode::InstanceRegistryInvalid,
                    "failed to read instance registry entry",
                )
            })?;
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !name.ends_with(".json") || name.starts_with('.') {
                continue;
            }
            let meta = fs::symlink_metadata(&path).map_err(|_| {
                PlatformError::new(
                    ErrorCode::InstanceRegistryInvalid,
                    "failed to inspect instance registry entry",
                )
            })?;
            if meta.file_type().is_symlink() {
                return Err(PlatformError::new(
                    ErrorCode::InstanceRegistryInvalid,
                    "instance registry entry must not be a symlink",
                ));
            }
            if !meta.file_type().is_file() {
                return Err(PlatformError::new(
                    ErrorCode::InstanceRegistryInvalid,
                    "instance registry entry must be a regular file",
                ));
            }
            let mode = meta.permissions().mode() & 0o777;
            if mode & 0o022 != 0 {
                return Err(PlatformError::new(
                    ErrorCode::InstanceRegistryInvalid,
                    "instance registry entry must not be group or world writable",
                ));
            }
            if matches!(scope, ServiceScope::System) && mode & 0o044 != 0o044 {
                return Err(PlatformError::new(
                    ErrorCode::InstanceRegistryInvalid,
                    "system registry entry must be readable by managed services",
                ));
            }
            let bytes = fs::read(&path).map_err(|_| {
                PlatformError::new(
                    ErrorCode::InstanceRegistryInvalid,
                    "failed to read instance registry entry",
                )
            })?;
            let record: InstanceRecord = serde_json::from_slice(&bytes).map_err(|_| {
                PlatformError::new(
                    ErrorCode::InstanceRegistryInvalid,
                    "instance registry entry is not valid JSON",
                )
            })?;
            if record.schema_version != REGISTRY_SCHEMA_VERSION {
                return Err(PlatformError::new(
                    ErrorCode::InstanceRegistryInvalid,
                    "instance registry schema version is unsupported",
                ));
            }
            if record.service_scope != scope {
                return Err(PlatformError::new(
                    ErrorCode::InstanceRegistryInvalid,
                    "instance registry entry scope does not match its directory",
                ));
            }
            if !record.binary_path().is_absolute() {
                return Err(PlatformError::new(
                    ErrorCode::InstanceRegistryInvalid,
                    "instance registry executable path must be absolute",
                ));
            }
            if record.config_sha256.len() != 64 || hex::decode(&record.config_sha256).is_err() {
                return Err(PlatformError::new(
                    ErrorCode::InstanceRegistryInvalid,
                    "instance registry configuration checksum is invalid",
                ));
            }
            if !Path::new(&record.data_path).is_absolute() {
                return Err(PlatformError::new(
                    ErrorCode::InstanceRegistryInvalid,
                    "instance registry data path must be absolute",
                ));
            }
            match &record.object_authority {
                RegisteredObjectAuthority::Local { path } if Path::new(path).is_absolute() => {}
                RegisteredObjectAuthority::S3 { endpoint, bucket }
                    if !endpoint.is_empty() && !bucket.is_empty() => {}
                _ => {
                    return Err(PlatformError::new(
                        ErrorCode::InstanceRegistryInvalid,
                        "instance registry object authority is invalid",
                    ));
                }
            }
            match (record.service_scope, record.service_user.as_deref()) {
                (ServiceScope::System, Some(user)) if !user.is_empty() && user != "root" => {}
                (ServiceScope::User, None) => {}
                _ => {
                    return Err(PlatformError::new(
                        ErrorCode::InstanceRegistryInvalid,
                        "instance registry service account does not match its scope",
                    ));
                }
            }
            let expected_name = format!("{}.json", record.instance_id);
            if name != expected_name {
                return Err(PlatformError::new(
                    ErrorCode::InstanceRegistryInvalid,
                    "instance registry entry name does not match its instance ID",
                ));
            }
            let _ = record.instance_id()?;
            out.push(record);
        }
        Ok(())
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

fn registration_locations(
    canonical_config_path: &Path,
) -> Result<(String, RegisteredObjectAuthority, String), PlatformError> {
    current_registration_locations(canonical_config_path)
}

fn current_registration_locations(
    canonical_config_path: &Path,
) -> Result<(String, RegisteredObjectAuthority, String), PlatformError> {
    let loaded =
        crate::config_load::load_platform_config_from(canonical_config_path, Path::new("/"))?;
    let (data_path, authority) = locations_from_config(&loaded.config);
    Ok((data_path, authority, loaded.sha256))
}

fn locations_from_config(
    config: &open_compute_core::PlatformConfig,
) -> (String, RegisteredObjectAuthority) {
    let authority = match &config.object_storage {
        ObjectStorageConfig::Local(local) => RegisteredObjectAuthority::Local {
            path: local.path.to_string_lossy().into_owned(),
        },
        ObjectStorageConfig::S3(s3) => RegisteredObjectAuthority::S3 {
            endpoint: s3.endpoint.to_string(),
            bucket: s3.bucket.clone(),
        },
    };
    (config.data.path.to_string_lossy().into_owned(), authority)
}

fn ensure_registry_tree(root: &Path) -> Result<(), PlatformError> {
    if let Some(parent) = root.parent()
        && !parent.exists()
    {
        fs::create_dir_all(parent).map_err(|_| {
            PlatformError::new(
                ErrorCode::InstanceRegistryInvalid,
                "failed to create instance registry parent directories",
            )
        })?;
    }
    ensure_dir_secure(root).map_err(|_| {
        PlatformError::new(
            ErrorCode::InstanceRegistryInvalid,
            "failed to create or validate instance registry root",
        )
    })
}

fn default_user_registry_root() -> Result<PathBuf, PlatformError> {
    if let Ok(xdg) = std::env::var("XDG_STATE_HOME")
        && !xdg.is_empty()
    {
        return Ok(PathBuf::from(xdg).join("open-compute/instances"));
    }
    let home = std::env::var_os("HOME").ok_or_else(|| {
        PlatformError::new(
            ErrorCode::InstanceRegistryInvalid,
            "HOME is unavailable for the user instance registry",
        )
    })?;
    #[cfg(target_os = "macos")]
    {
        Ok(PathBuf::from(home).join("Library/Application Support/open-compute/instances"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(PathBuf::from(home).join(".local/state/open-compute/instances"))
    }
}

fn decode_digest(hex_digest: &str) -> Result<[u8; 32], PlatformError> {
    let bytes = hex::decode(hex_digest).map_err(|_| {
        PlatformError::new(
            ErrorCode::InstanceRegistryInvalid,
            "instance registry digest is not valid hex",
        )
    })?;
    <[u8; 32]>::try_from(bytes.as_slice()).map_err(|_| {
        PlatformError::new(
            ErrorCode::InstanceRegistryInvalid,
            "instance registry digest must be 32 bytes",
        )
    })
}

/// Choose a collision-free short ID for `canonical_config_path`.
fn allocate_instance_id(
    canonical_config_path: &Path,
    occupied: &[(String, [u8; 32])],
) -> Result<InstanceId, PlatformError> {
    let mut candidate = InstanceId::from_canonical_config_path(canonical_config_path)?;
    loop {
        if let Some((_, digest)) = occupied
            .iter()
            .find(|(short, _)| short == candidate.as_str())
        {
            if digest == candidate.digest() {
                return Ok(candidate);
            }
            candidate = candidate.extend_one()?;
            continue;
        }
        return Ok(candidate);
    }
}

#[cfg(test)]
mod tests;
