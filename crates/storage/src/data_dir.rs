//! Data directory acquisition and layout.

use crate::fs;
use crate::lock::{DataDirLock, FilesystemDurability};
use open_compute_core::{PlatformError, StartupId, config::DataConfig};
use serde::{Deserialize, Serialize};
use std::io::Read as _;
use std::path::{Path, PathBuf};

const KEYS: &str = "keys";
const RUNTIME: &str = "runtime";
const GATEWAY: &str = "gateway";
const GATEWAY_RUN: &str = "run";
const GATEWAY_STORAGE: &str = "storage";
const GATEWAY_CONFIG_STATE: &str = "config-state";
const EXTENSIONS: &str = "extensions";
const CACHE: &str = "cache";
const TMP: &str = "tmp";
const ARTIFACTS: &str = "artifacts";
const GIT: &str = "git";
const QUARANTINE: &str = "quarantine";
const SHA256: &str = "sha256";
const VERSION_STAGING: &str = "version-staging";
const BACKUP_STAGING: &str = "backup-staging";
const DIAGNOSTICS: &str = "diagnostics";
const OPERATIONS: &str = "operations";
const FAILED_STARTS: &str = "failed-starts";
const LOCK_NAME: &str = "platform.lock";
const CONTROL_DB_NAME: &str = "control.sqlite";
const OBSERVABILITY_DB_NAME: &str = "observability.sqlite";
const DURABLE_OBJECTS: &str = "do";
const DURABLE_OBJECT_WORKERD: &str = "workerd";
const DURABLE_OBJECT_MARKER: &str = "format.json";
/// Stable native Durable Object namespace identity compiled into workerd.
pub const DURABLE_OBJECT_UNIQUE_KEY: &str = "open-compute-do-host-v1";
/// Platform-owned Durable Object local-disk format version.
pub const DURABLE_OBJECT_DATA_FORMAT_VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DurableObjectFormatMarker {
    schema_version: u32,
    instance_id: String,
    unique_key: String,
    workerd_version: String,
}

/// Inspect an existing native Durable Object storage boundary without creating or mutating it.
pub fn inspect_durable_object_storage(
    data_root: &Path,
    instance_id: &str,
    workerd_version: &str,
) -> Result<PathBuf, PlatformError> {
    if !data_root.is_absolute() || instance_id.is_empty() || workerd_version.is_empty() {
        return Err(do_storage_unavailable());
    }
    let parent = data_root.join(DURABLE_OBJECTS);
    let workerd = parent.join(DURABLE_OBJECT_WORKERD);
    let marker = parent.join(DURABLE_OBJECT_MARKER);
    fs::validate_contained(data_root, &parent)?;
    fs::validate_contained(data_root, &workerd)?;
    fs::validate_contained(data_root, &marker)?;
    fs::validate_owned_dir(&parent)?;
    fs::validate_owned_dir(&workerd)?;
    fs::validate_owned_file(&marker)?;
    if DataDirLock::classify_path(&workerd) != FilesystemDurability::ApparentlyLocal {
        return Err(do_storage_unavailable());
    }
    let bytes = read_durable_object_marker(&marker)?;
    let actual: DurableObjectFormatMarker =
        serde_json::from_slice(&bytes).map_err(|_| do_storage_unavailable())?;
    let expected = DurableObjectFormatMarker {
        schema_version: DURABLE_OBJECT_DATA_FORMAT_VERSION,
        instance_id: instance_id.to_owned(),
        unique_key: DURABLE_OBJECT_UNIQUE_KEY.to_owned(),
        workerd_version: workerd_version.to_owned(),
    };
    if actual != expected {
        return Err(do_storage_unavailable());
    }
    Ok(workerd)
}

/// RAII owner of a data directory and its exclusive lock.
#[derive(Debug)]
pub struct DataDir {
    root: PathBuf,
    lock: DataDirLock,
}

impl DataDir {
    /// Acquire exclusive ownership of `config.data.path`.
    pub fn acquire(config: &DataConfig) -> Result<Self, PlatformError> {
        let root = &config.path;
        fs::require_absolute(root)?;
        if root.exists() {
            fs::validate_root(root)?;
        } else {
            fs::create_root_first_run(root)?;
        }
        create_layout(root)?;
        let lock_path = config.data_lock_path();
        fs::validate_contained(root, &lock_path)?;
        let lock = DataDirLock::acquire(&lock_path, StartupId::generate())?;
        let data_dir = Self {
            root: root.clone(),
            lock,
        };
        data_dir.clear_version_staging()?;
        Ok(data_dir)
    }

    /// Acquire an already initialized data directory for an offline command.
    ///
    /// This path never creates layout, generates a key, opens a database, or runs a migration.
    pub fn acquire_existing_offline(config: &DataConfig) -> Result<Self, PlatformError> {
        let root = &config.path;
        fs::require_absolute(root)?;
        fs::validate_root(root)?;
        let lock_path = config.data_lock_path();
        fs::validate_contained(root, &lock_path)?;
        fs::validate_owned_file(&lock_path)?;
        let lock = DataDirLock::acquire(&lock_path, StartupId::generate())?;
        let data_dir = Self {
            root: root.clone(),
            lock,
        };
        Ok(data_dir)
    }

    /// Absolute data root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Startup generation that owns the exclusive data-directory lock.
    #[must_use]
    pub fn startup_id(&self) -> StartupId {
        self.lock.startup_id()
    }

    /// Control database path: `<data_dir>/control.sqlite`.
    #[must_use]
    pub fn control_db_path(&self) -> PathBuf {
        self.root.join(CONTROL_DB_NAME)
    }

    /// Scheduler database path: `<data_dir>/control.sqlite`.
    #[must_use]
    pub fn scheduler_db_path(&self) -> PathBuf {
        self.control_db_path()
    }

    /// Workers Logs database path: `<data_dir>/observability.sqlite`.
    #[must_use]
    pub fn observability_db_path(&self) -> PathBuf {
        self.root.join(OBSERVABILITY_DB_NAME)
    }

    /// Keys directory.
    #[must_use]
    pub fn keys_dir(&self) -> PathBuf {
        self.root.join(KEYS)
    }

    /// Runtime compile-cache directory.
    #[must_use]
    pub fn runtime_dir(&self) -> PathBuf {
        self.root.join(RUNTIME)
    }

    /// Prepare this instance's private temporary-task root.
    pub fn prepare_tmp_dir(&self) -> Result<PathBuf, PlatformError> {
        let path = self.root.join(TMP);
        fs::validate_contained(&self.root, &path)?;
        fs::create_dir_secure(&path)?;
        Ok(path)
    }

    /// Create the private directories for managed gateway state.
    pub fn prepare_gateway_dir(&self) -> Result<PathBuf, PlatformError> {
        let gateway = self.root.join(GATEWAY);
        fs::validate_contained(&self.root, &gateway)?;
        fs::create_dir_secure(&gateway)?;
        for name in [GATEWAY_RUN, GATEWAY_STORAGE, GATEWAY_CONFIG_STATE] {
            let path = gateway.join(name);
            fs::validate_contained(&self.root, &path)?;
            fs::create_dir_secure(&path)?;
        }
        Ok(gateway)
    }

    /// Create one private working directory for a configured local extension provider.
    pub fn prepare_extension_provider_dir(&self, name: &str) -> Result<PathBuf, PlatformError> {
        open_compute_core::validate_local_extension_name(name)?;
        let parent = self.runtime_dir().join(EXTENSIONS);
        let path = parent.join(name);
        fs::validate_contained(&self.root, &parent)?;
        fs::create_dir_secure(&parent)?;
        fs::validate_contained(&self.root, &path)?;
        fs::create_dir_secure(&path)?;
        Ok(path)
    }

    /// List existing private provider directories, including removed extensions.
    pub fn existing_extension_provider_dirs(
        &self,
    ) -> Result<Vec<(String, PathBuf)>, PlatformError> {
        let parent = self.runtime_dir().join(EXTENSIONS);
        fs::validate_contained(&self.root, &parent)?;
        if !parent.try_exists().map_err(|_| extension_path_invalid())? {
            return Ok(Vec::new());
        }
        fs::validate_owned_dir(&parent)?;
        let mut paths = std::fs::read_dir(&parent)
            .map_err(|_| extension_path_invalid())?
            .map(|entry| {
                let entry = entry.map_err(|_| extension_path_invalid())?;
                let name = entry
                    .file_name()
                    .into_string()
                    .map_err(|_| extension_path_invalid())?;
                open_compute_core::validate_local_extension_name(&name)?;
                let path = entry.path();
                fs::validate_contained(&self.root, &path)?;
                fs::validate_owned_dir(&path)?;
                Ok((name, path))
            })
            .collect::<Result<Vec<_>, PlatformError>>()?;
        paths.sort_by(|left, right| left.0.cmp(&right.0));
        Ok(paths)
    }

    /// Artifact cache directory: `<data>/cache/artifacts`.
    #[must_use]
    pub fn artifact_cache_dir(&self) -> PathBuf {
        self.root.join(CACHE).join(ARTIFACTS)
    }

    /// Authoritative bare Git repository root for Cloudflare Artifacts.
    #[must_use]
    pub fn artifact_git_dir(&self) -> PathBuf {
        self.root.join(ARTIFACTS).join(GIT)
    }

    /// Recoverable quarantine for deleted or corrupt Git repository directories.
    #[must_use]
    pub fn artifact_quarantine_dir(&self) -> PathBuf {
        self.root.join(ARTIFACTS).join(QUARANTINE)
    }

    /// Private crash-recoverable staging directory for streamed version uploads.
    #[must_use]
    pub fn version_staging_dir(&self) -> PathBuf {
        self.root.join(VERSION_STAGING)
    }

    /// Private staging directory for online backups and verified restores.
    #[must_use]
    pub fn backup_staging_dir(&self) -> PathBuf {
        self.root.join(BACKUP_STAGING)
    }

    /// Atomically write one non-authoritative operator receipt below `operations/`.
    pub fn write_operation_receipt(
        &self,
        name: &str,
        contents: &[u8],
    ) -> Result<(), PlatformError> {
        if !valid_operation_receipt_name(name) {
            return Err(PlatformError::new(
                open_compute_core::ErrorCode::PathInvalid,
                "operation receipt name is invalid",
            ));
        }
        let operations = self.root.join(OPERATIONS);
        fs::validate_contained(&self.root, &operations)?;
        fs::create_dir_secure(&operations)?;
        let path = operations.join(name);
        fs::validate_contained(&self.root, &path)?;
        fs::atomic_write(&path, contents)
    }

    /// Read one bounded, regular operator receipt.
    pub fn read_operation_receipt(
        &self,
        name: &str,
        max_bytes: u64,
    ) -> Result<Vec<u8>, PlatformError> {
        read_operation_receipt(&self.root, name, max_bytes)
    }

    /// Platform-owned parent for native Durable Object storage and its marker.
    #[must_use]
    pub fn durable_objects_dir(&self) -> PathBuf {
        self.root.join(DURABLE_OBJECTS)
    }

    /// Writable directory mapped exclusively to workerd's native DO disk service.
    #[must_use]
    pub fn durable_object_workerd_dir(&self) -> PathBuf {
        self.durable_objects_dir().join(DURABLE_OBJECT_WORKERD)
    }

    /// Create and verify the local-only native Durable Object storage boundary.
    pub fn prepare_durable_object_storage(
        &self,
        instance_id: &str,
        workerd_version: &str,
    ) -> Result<PathBuf, PlatformError> {
        if instance_id.is_empty() || workerd_version.is_empty() {
            return Err(do_storage_unavailable());
        }
        let parent = self.durable_objects_dir();
        let workerd = self.durable_object_workerd_dir();
        fs::validate_contained(&self.root, &parent)?;
        fs::create_dir_secure(&parent)?;
        fs::validate_contained(&self.root, &workerd)?;
        fs::create_dir_secure(&workerd)?;
        if DataDirLock::classify_path(&workerd) != FilesystemDurability::ApparentlyLocal {
            return Err(do_storage_unavailable());
        }
        let marker = parent.join(DURABLE_OBJECT_MARKER);
        fs::validate_contained(&self.root, &marker)?;
        let expected = DurableObjectFormatMarker {
            schema_version: DURABLE_OBJECT_DATA_FORMAT_VERSION,
            instance_id: instance_id.to_owned(),
            unique_key: DURABLE_OBJECT_UNIQUE_KEY.to_owned(),
            workerd_version: workerd_version.to_owned(),
        };
        if marker.exists() || std::fs::symlink_metadata(&marker).is_ok() {
            fs::validate_owned_file(&marker)?;
            let bytes = read_durable_object_marker(&marker)?;
            let actual: DurableObjectFormatMarker =
                serde_json::from_slice(&bytes).map_err(|_| do_storage_unavailable())?;
            if actual != expected {
                return Err(do_storage_unavailable());
            }
        } else {
            let bytes = serde_json::to_vec(&expected).map_err(|_| do_storage_unavailable())?;
            fs::atomic_write(&marker, &bytes)?;
        }
        fs::validate_owned_dir(&workerd)?;
        Ok(workerd)
    }

    /// Held lock.
    #[must_use]
    pub fn lock(&self) -> &DataDirLock {
        &self.lock
    }

    /// Filesystem durability hint for doctor.
    #[must_use]
    pub fn filesystem_durability(&self) -> FilesystemDurability {
        self.lock.filesystem_durability()
    }

    pub(crate) fn record_instance_id(&self, instance_id: &str) -> Result<(), PlatformError> {
        self.lock.write_metadata(Some(instance_id))
    }

    /// Create `control.sqlite` as a 0600 regular file after the master key is resolved.
    pub(crate) fn ensure_control_db(&self) -> Result<PathBuf, PlatformError> {
        let db_path = self.control_db_path();
        fs::validate_contained(&self.root, &db_path)?;
        fs::ensure_file_secure(&db_path)?;
        fs::validate_contained(&self.root, &db_path)?;
        Ok(db_path)
    }

    /// Ensure the shared `control.sqlite` file for scheduler access.
    pub fn ensure_scheduler_db(&self) -> Result<PathBuf, PlatformError> {
        if self.filesystem_durability() != FilesystemDurability::ApparentlyLocal {
            return Err(PlatformError::new(
                open_compute_core::ErrorCode::SchedulerUnavailable,
                "scheduler database requires a local filesystem",
            ));
        }
        let db_path = self.scheduler_db_path();
        fs::validate_contained(&self.root, &db_path)?;
        fs::ensure_file_secure(&db_path)?;
        fs::validate_contained(&self.root, &db_path)?;
        Ok(db_path)
    }

    /// Create `observability.sqlite` as a 0600 regular file under the owned data directory.
    pub fn ensure_observability_db(&self) -> Result<PathBuf, PlatformError> {
        if self.filesystem_durability() != FilesystemDurability::ApparentlyLocal {
            return Err(PlatformError::new(
                open_compute_core::ErrorCode::PlatformUnavailable,
                "observability database requires a local filesystem",
            ));
        }
        let db_path = self.observability_db_path();
        fs::validate_contained(&self.root, &db_path)?;
        fs::ensure_file_secure(&db_path)?;
        fs::validate_contained(&self.root, &db_path)?;
        Ok(db_path)
    }

    /// Refuse file-level scheduler recovery because control and scheduler share one file.
    pub fn recover_corrupt_scheduler_db(
        &self,
        _backup_name: &str,
        _busy_timeout_ms: u64,
        _now_ms: i64,
    ) -> Result<PathBuf, PlatformError> {
        Err(PlatformError::new(
            open_compute_core::ErrorCode::SchedulerUnavailable,
            "scheduler state shares control.sqlite; full snapshot restore is required",
        ))
    }

    fn clear_version_staging(&self) -> Result<(), PlatformError> {
        let staging = self.version_staging_dir();
        for entry in std::fs::read_dir(&staging).map_err(|_| {
            PlatformError::new(
                open_compute_core::ErrorCode::PathInvalid,
                "failed to inspect version staging directory",
            )
        })? {
            let entry = entry.map_err(|_| {
                PlatformError::new(
                    open_compute_core::ErrorCode::PathInvalid,
                    "failed to inspect version staging entry",
                )
            })?;
            std::fs::remove_file(entry.path()).map_err(|_| {
                PlatformError::new(
                    open_compute_core::ErrorCode::PathInvalid,
                    "failed to clear stale version staging file",
                )
            })?;
        }
        Ok(())
    }
}

/// Read one bounded operator receipt from an existing data root.
pub fn read_operation_receipt(
    data_root: &Path,
    name: &str,
    max_bytes: u64,
) -> Result<Vec<u8>, PlatformError> {
    if !valid_operation_receipt_name(name) || max_bytes == 0 || max_bytes > 1_048_576 {
        return Err(PlatformError::new(
            open_compute_core::ErrorCode::PathInvalid,
            "operation receipt read limit is invalid",
        ));
    }
    let operations = data_root.join(OPERATIONS);
    fs::validate_contained(data_root, &operations)?;
    fs::validate_owned_dir(&operations)?;
    let path = operations.join(name);
    fs::validate_contained(data_root, &path)?;
    let mut file = fs::open_file(&path, false, false)?;
    fs::validate_authority_fd(&file)?;
    let mut bytes = Vec::new();
    file.by_ref()
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| {
            PlatformError::new(
                open_compute_core::ErrorCode::PathInvalid,
                "operation receipt could not be read",
            )
        })?;
    if bytes.len() as u64 > max_bytes {
        return Err(PlatformError::new(
            open_compute_core::ErrorCode::LimitInvalid,
            "operation receipt exceeds its read limit",
        ));
    }
    Ok(bytes)
}

fn valid_operation_receipt_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.contains('/')
        && !name.contains('\\')
        && !matches!(name, "." | "..")
}

fn read_durable_object_marker(path: &Path) -> Result<Vec<u8>, PlatformError> {
    let mut file = fs::open_file(path, false, false).map_err(|_| do_storage_unavailable())?;
    fs::validate_authority_fd(&file).map_err(|_| do_storage_unavailable())?;
    let mut bytes = Vec::new();
    file.by_ref()
        .take(4097)
        .read_to_end(&mut bytes)
        .map_err(|_| do_storage_unavailable())?;
    if bytes.len() > 4096 {
        return Err(do_storage_unavailable());
    }
    Ok(bytes)
}

fn extension_path_invalid() -> PlatformError {
    PlatformError::new(
        open_compute_core::ErrorCode::PathInvalid,
        "local extension provider directory is invalid",
    )
}

fn do_storage_unavailable() -> PlatformError {
    PlatformError::new(
        open_compute_core::ErrorCode::DoStorageUnavailable,
        "Durable Object local storage is unavailable or incompatible",
    )
}

fn create_layout(root: &Path) -> Result<(), PlatformError> {
    fs::create_dir_secure(&root.join(KEYS))?;
    fs::create_dir_secure(&root.join(RUNTIME))?;
    fs::create_dir_secure(&root.join(TMP))?;
    fs::create_dir_secure(&root.join(CACHE))?;
    fs::create_dir_secure(&root.join(CACHE).join(ARTIFACTS))?;
    fs::create_dir_secure(&root.join(CACHE).join(ARTIFACTS).join(SHA256))?;
    fs::create_dir_secure(&root.join(ARTIFACTS))?;
    fs::create_dir_secure(&root.join(ARTIFACTS).join(GIT))?;
    fs::create_dir_secure(&root.join(ARTIFACTS).join(QUARANTINE))?;
    fs::create_dir_secure(&root.join(VERSION_STAGING))?;
    fs::create_dir_secure(&root.join(BACKUP_STAGING))?;
    fs::create_dir_secure(&root.join(DIAGNOSTICS))?;
    fs::create_dir_secure(&root.join(DIAGNOSTICS).join(FAILED_STARTS))?;
    Ok(())
}

/// Recreate excluded runtime/cache/lock layout inside a validated restore staging root.
pub(crate) fn initialize_restored_layout(root: &Path) -> Result<(), PlatformError> {
    fs::validate_root(root)?;
    create_layout(root)?;
    fs::ensure_file_secure(&root.join(LOCK_NAME))
}
