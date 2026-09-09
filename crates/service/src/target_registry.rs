//! Secure per-user registry of explicit remote Wrangler targets.

use open_compute_core::{
    CloudflareAccountId, ErrorCode, PlatformError, SecretString, TargetApiBaseUrl, TargetName,
};
use open_compute_storage::{atomic_write, ensure_dir_secure};
use rustix::fs::{FlockOperation, Mode, OFlags, flock};
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::Read;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

/// Current target registry schema.
pub const TARGET_REGISTRY_SCHEMA_VERSION: u32 = 1;
const MAX_TARGETS: usize = 128;
const MAX_REGISTRY_BYTES: u64 = 256 * 1024;
const MAX_TOKEN_BYTES: u64 = 256;

/// One explicit remote open-compute deployment target.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetRecord {
    /// Registry schema version recorded with this target.
    pub schema_version: u32,
    /// Developer-chosen local alias.
    pub name: TargetName,
    /// Normalized remote API base URL.
    pub api_base_url: TargetApiBaseUrl,
    /// Cloudflare-compatible public account ID.
    pub account_id: CloudflareAccountId,
    /// Absolute external deployer-token file reference.
    pub token_file: PathBuf,
    /// Unix epoch milliseconds at creation.
    pub created_at: u64,
}

impl TargetRecord {
    fn validate(&self) -> Result<(), PlatformError> {
        if self.schema_version != TARGET_REGISTRY_SCHEMA_VERSION {
            return Err(registry_invalid(
                "target record schema version is unsupported",
            ));
        }
        validate_token_path_form(&self.token_file)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistryFile {
    schema_version: u32,
    targets: Vec<TargetRecord>,
}

impl Default for RegistryFile {
    fn default() -> Self {
        Self {
            schema_version: TARGET_REGISTRY_SCHEMA_VERSION,
            targets: Vec::new(),
        }
    }
}

/// Read/write authority for one per-user `targets.toml`.
#[derive(Clone, Debug)]
pub struct TargetRegistry {
    path: PathBuf,
}

impl TargetRegistry {
    /// Resolve the current user's platform-specific target registry path.
    pub fn production() -> Result<Self, PlatformError> {
        Ok(Self {
            path: default_target_registry_path()?,
        })
    }

    /// Build a registry at an explicit absolute test or client path.
    #[must_use]
    pub fn at(path: PathBuf) -> Self {
        Self { path }
    }

    /// Registry path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// List all records in deterministic name order without reading token values.
    pub fn list(&self) -> Result<Vec<TargetRecord>, PlatformError> {
        let mut file = self.read_file()?;
        file.targets
            .sort_by(|left, right| left.name.cmp(&right.name));
        Ok(file.targets)
    }

    /// Read one exact target record without reading its token value.
    pub fn get(&self, name: &TargetName) -> Result<TargetRecord, PlatformError> {
        self.list()?
            .into_iter()
            .find(|record| &record.name == name)
            .ok_or_else(|| {
                PlatformError::new(
                    ErrorCode::TargetNotFound,
                    "requested target is not registered",
                )
            })
    }

    /// Add one new unique target after validating its external token file.
    pub fn add(
        &self,
        name: TargetName,
        api_base_url: TargetApiBaseUrl,
        account_id: CloudflareAccountId,
        token_file: PathBuf,
        now: SystemTime,
    ) -> Result<TargetRecord, PlatformError> {
        validate_token_file(&token_file)?;
        let _lock = self.lock_mutation()?;
        let mut registry = self.read_file()?;
        if registry.targets.len() >= MAX_TARGETS {
            return Err(registry_invalid("target registry exceeds its record limit"));
        }
        if registry.targets.iter().any(|record| record.name == name) {
            return Err(registry_invalid("target name is already registered"));
        }
        if registry
            .targets
            .iter()
            .any(|record| record.api_base_url == api_base_url && record.account_id == account_id)
        {
            return Err(registry_invalid(
                "target API base URL and account pair is already registered",
            ));
        }
        let created_at = open_compute_core::unix_time_ms(now)
            .and_then(|value| u64::try_from(value).ok())
            .ok_or_else(|| {
                registry_invalid("target creation time is outside the supported range")
            })?;
        let record = TargetRecord {
            schema_version: TARGET_REGISTRY_SCHEMA_VERSION,
            name,
            api_base_url,
            account_id,
            token_file,
            created_at,
        };
        registry.targets.push(record.clone());
        self.write_file(&registry)?;
        Ok(record)
    }

    /// Remove one record without deleting its external token file.
    pub fn remove(&self, name: &TargetName) -> Result<TargetRecord, PlatformError> {
        let _lock = self.lock_mutation()?;
        let mut registry = self.read_file()?;
        let position = registry
            .targets
            .iter()
            .position(|record| &record.name == name)
            .ok_or_else(|| {
                PlatformError::new(
                    ErrorCode::TargetNotFound,
                    "requested target is not registered",
                )
            })?;
        let removed = registry.targets.remove(position);
        self.write_file(&registry)?;
        Ok(removed)
    }

    fn read_file(&self) -> Result<RegistryFile, PlatformError> {
        if !self.path.is_absolute() {
            return Err(registry_invalid("target registry path must be absolute"));
        }
        let meta = match fs::symlink_metadata(&self.path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(RegistryFile::default());
            }
            Err(_) => return Err(registry_invalid("target registry could not be inspected")),
            Ok(meta) => meta,
        };
        ensure_target_directory(
            self.path
                .parent()
                .ok_or_else(|| registry_invalid("target registry path has no parent"))?,
        )?;
        validate_registry_metadata(&meta)?;
        let fd = rustix::fs::open(
            &self.path,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| {
            registry_invalid("target registry could not be opened without following links")
        })?;
        let mut file = File::from(fd);
        let opened = file
            .metadata()
            .map_err(|_| registry_invalid("target registry could not be inspected"))?;
        validate_registry_metadata(&opened)?;
        if opened.len() > MAX_REGISTRY_BYTES {
            return Err(registry_invalid("target registry exceeds its size limit"));
        }
        let mut bytes = Vec::new();
        file.by_ref()
            .take(MAX_REGISTRY_BYTES.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|_| registry_invalid("target registry could not be read"))?;
        if bytes.len() as u64 > MAX_REGISTRY_BYTES {
            return Err(registry_invalid("target registry exceeds its size limit"));
        }
        let registry: RegistryFile = toml::from_slice(&bytes)
            .map_err(|_| registry_invalid("target registry is not valid TOML"))?;
        validate_registry(&registry)?;
        Ok(registry)
    }

    fn write_file(&self, registry: &RegistryFile) -> Result<(), PlatformError> {
        validate_registry(registry)?;
        let parent = self
            .path
            .parent()
            .ok_or_else(|| registry_invalid("target registry path has no parent"))?;
        ensure_target_directory(parent)?;
        if let Ok(meta) = fs::symlink_metadata(&self.path) {
            validate_registry_metadata(&meta)?;
        }
        let body = toml::to_string_pretty(registry)
            .map_err(|_| registry_invalid("target registry could not be encoded"))?;
        if body.len() as u64 > MAX_REGISTRY_BYTES {
            return Err(registry_invalid("target registry exceeds its size limit"));
        }
        atomic_write(&self.path, body.as_bytes())
            .map_err(|_| registry_invalid("target registry atomic write failed"))
    }

    fn lock_mutation(&self) -> Result<MutationLock, PlatformError> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| registry_invalid("target registry path has no parent"))?;
        ensure_target_directory(parent)?;
        let lock_path = parent.join(".targets.lock");
        let fd = rustix::fs::open(
            &lock_path,
            OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )
        .map_err(|_| {
            registry_invalid("target registry lock could not be opened without following links")
        })?;
        let file = File::from(fd);
        validate_registry_metadata(
            &file
                .metadata()
                .map_err(|_| registry_invalid("target registry lock could not be inspected"))?,
        )?;
        flock(&file, FlockOperation::LockExclusive)
            .map_err(|_| registry_invalid("target registry lock could not be acquired"))?;
        Ok(MutationLock(file))
    }
}

struct MutationLock(File);

impl Drop for MutationLock {
    fn drop(&mut self) {
        let _ = flock(&self.0, FlockOperation::Unlock);
    }
}

/// Validate the token reference and return its bounded UTF-8 value.
pub fn read_target_token(path: &Path) -> Result<SecretString, PlatformError> {
    validate_token_path_form(path)?;
    let fd = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| target_invalid("target token file could not be opened without following links"))?;
    let file = File::from(fd);
    validate_token_metadata(
        &file
            .metadata()
            .map_err(|_| target_invalid("target token file could not be inspected"))?,
    )?;
    let mut value = String::new();
    file.take(MAX_TOKEN_BYTES.saturating_add(1))
        .read_to_string(&mut value)
        .map_err(|_| target_invalid("target token file is not valid bounded UTF-8"))?;
    if value.len() as u64 > MAX_TOKEN_BYTES {
        return Err(target_invalid("target token file exceeds its size limit"));
    }
    let value = value.trim_end_matches(['\n', '\r']);
    if value.is_empty() || !value.bytes().all(|byte| matches!(byte, b'!'..=b'~')) {
        return Err(target_invalid(
            "target token file must contain one nonempty visible ASCII token",
        ));
    }
    Ok(SecretString::new(value))
}

fn validate_token_file(path: &Path) -> Result<(), PlatformError> {
    read_target_token(path).map(|_| ())
}

fn validate_token_path_form(path: &Path) -> Result<(), PlatformError> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| component == Component::ParentDir)
    {
        return Err(target_invalid(
            "target token file path must be absolute without parent traversal",
        ));
    }
    Ok(())
}

fn validate_token_metadata(meta: &fs::Metadata) -> Result<(), PlatformError> {
    if !meta.file_type().is_file() {
        return Err(target_invalid("target token file must be a regular file"));
    }
    if meta.permissions().mode() & 0o777 != 0o600 {
        return Err(target_invalid("target token file must have mode 0600"));
    }
    if meta.uid() != rustix::process::getuid().as_raw() {
        return Err(target_invalid(
            "target token file must be owned by the current user",
        ));
    }
    if meta.len() == 0 || meta.len() > MAX_TOKEN_BYTES {
        return Err(target_invalid("target token file has an invalid size"));
    }
    Ok(())
}

fn validate_registry(registry: &RegistryFile) -> Result<(), PlatformError> {
    if registry.schema_version != TARGET_REGISTRY_SCHEMA_VERSION {
        return Err(registry_invalid(
            "target registry schema version is unsupported",
        ));
    }
    if registry.targets.len() > MAX_TARGETS {
        return Err(registry_invalid("target registry exceeds its record limit"));
    }
    for record in &registry.targets {
        record.validate()?;
    }
    for (index, record) in registry.targets.iter().enumerate() {
        if registry.targets[..index].iter().any(|prior| {
            prior.name == record.name
                || (prior.api_base_url == record.api_base_url
                    && prior.account_id == record.account_id)
        }) {
            return Err(registry_invalid(
                "target registry contains duplicate authority",
            ));
        }
    }
    Ok(())
}

fn validate_registry_metadata(meta: &fs::Metadata) -> Result<(), PlatformError> {
    if meta.file_type().is_symlink() || !meta.file_type().is_file() {
        return Err(registry_invalid(
            "target registry must be a regular non-symlink file",
        ));
    }
    if meta.permissions().mode() & 0o777 != 0o600 {
        return Err(registry_invalid("target registry must have mode 0600"));
    }
    if meta.uid() != rustix::process::getuid().as_raw() {
        return Err(registry_invalid(
            "target registry must be owned by the current user",
        ));
    }
    Ok(())
}

fn ensure_target_directory(path: &Path) -> Result<(), PlatformError> {
    if !path.is_absolute() {
        return Err(registry_invalid(
            "target registry directory must be absolute",
        ));
    }
    if !path.exists() {
        if let Some(parent) = path.parent()
            && !parent.exists()
        {
            fs::create_dir_all(parent).map_err(|_| {
                registry_invalid("target registry parent directories could not be created")
            })?;
        }
        ensure_dir_secure(path)
            .map_err(|_| registry_invalid("target registry directory could not be created"))?;
    }
    let meta = fs::symlink_metadata(path)
        .map_err(|_| registry_invalid("target registry directory could not be inspected"))?;
    if meta.file_type().is_symlink() || !meta.file_type().is_dir() {
        return Err(registry_invalid(
            "target registry directory must be a non-symlink directory",
        ));
    }
    if meta.permissions().mode() & 0o777 != 0o700 {
        return Err(registry_invalid(
            "target registry directory must have mode 0700",
        ));
    }
    if meta.uid() != rustix::process::getuid().as_raw() {
        return Err(registry_invalid(
            "target registry directory must be owned by the current user",
        ));
    }
    Ok(())
}

fn default_target_registry_path() -> Result<PathBuf, PlatformError> {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME")
        && !xdg.is_empty()
    {
        return Ok(PathBuf::from(xdg).join("open-compute/targets.toml"));
    }
    let home = std::env::var_os("HOME")
        .ok_or_else(|| registry_invalid("HOME is unavailable for the target registry"))?;
    #[cfg(target_os = "macos")]
    return Ok(PathBuf::from(home).join("Library/Application Support/open-compute/targets.toml"));
    #[cfg(not(target_os = "macos"))]
    Ok(PathBuf::from(home).join(".config/open-compute/targets.toml"))
}

fn target_invalid(message: &'static str) -> PlatformError {
    PlatformError::new(ErrorCode::TargetInvalid, message)
}

fn registry_invalid(message: &'static str) -> PlatformError {
    PlatformError::new(ErrorCode::TargetRegistryInvalid, message)
}

#[cfg(test)]
#[path = "target_registry_tests.rs"]
mod tests;
