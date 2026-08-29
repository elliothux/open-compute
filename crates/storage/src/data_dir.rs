//! Data directory acquisition and layout.

use crate::fs;
use crate::lock::{DataDirLock, FilesystemDurability};
use open_compute_core::{PlatformError, StartupId, config::StorageConfig};
use serde::{Deserialize, Serialize};
use std::io::Read as _;
use std::path::{Path, PathBuf};

const KEYS: &str = "keys";
const RUNTIME: &str = "runtime";
const CACHE: &str = "cache";
const ARTIFACTS: &str = "artifacts";
const SHA256: &str = "sha256";
const DEPLOYMENT_STAGING: &str = "deployment-staging";
const BACKUP_STAGING: &str = "backup-staging";
const DIAGNOSTICS: &str = "diagnostics";
const OPERATIONS: &str = "operations";
const FAILED_STARTS: &str = "failed-starts";
const SCHEDULER_RECOVERY: &str = "scheduler-recovery";
const LOCK_NAME: &str = "platform.lock";
const CONTROL_DB_NAME: &str = "control.sqlite";
const SCHEDULER_DB_NAME: &str = "scheduler.sqlite";
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
    platform_id: String,
    unique_key: String,
    workerd_version: String,
}

/// Inspect an existing native Durable Object storage boundary without creating or mutating it.
pub fn inspect_durable_object_storage(
    data_root: &Path,
    platform_id: &str,
    workerd_version: &str,
) -> Result<PathBuf, PlatformError> {
    if !data_root.is_absolute() || platform_id.is_empty() || workerd_version.is_empty() {
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
    fs::validate_owned_file(&marker, true)?;
    if DataDirLock::classify_path(&workerd) != FilesystemDurability::ApparentlyLocal {
        return Err(do_storage_unavailable());
    }
    let bytes = read_durable_object_marker(&marker)?;
    let actual: DurableObjectFormatMarker =
        serde_json::from_slice(&bytes).map_err(|_| do_storage_unavailable())?;
    let expected = DurableObjectFormatMarker {
        schema_version: DURABLE_OBJECT_DATA_FORMAT_VERSION,
        platform_id: platform_id.to_owned(),
        unique_key: DURABLE_OBJECT_UNIQUE_KEY.to_owned(),
        workerd_version: workerd_version.to_owned(),
    };
    if actual != expected {
        return Err(do_storage_unavailable());
    }
    Ok(workerd)
}

/// P0.1 layout names that must not be pre-created as tenant resource files.
pub const FORBIDDEN_PRECREATE: &[&str] = &["do", "kv", "d1"];

/// RAII owner of a data directory and its exclusive lock.
#[derive(Debug)]
pub struct DataDir {
    root: PathBuf,
    lock: DataDirLock,
}

impl DataDir {
    /// Acquire exclusive ownership of `config.storage.data_dir`.
    pub fn acquire(config: &StorageConfig) -> Result<Self, PlatformError> {
        let root = &config.data_dir;
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
        data_dir.validate_children()?;
        data_dir.clear_deployment_staging()?;
        Ok(data_dir)
    }

    /// Acquire an already initialized data directory for an offline command.
    ///
    /// This path never creates layout, generates a key, opens a database, or runs a migration.
    pub fn acquire_existing_offline(config: &StorageConfig) -> Result<Self, PlatformError> {
        let root = &config.data_dir;
        fs::require_absolute(root)?;
        fs::validate_root(root)?;
        let lock_path = config.data_lock_path();
        fs::validate_contained(root, &lock_path)?;
        fs::validate_owned_file(&lock_path, true)?;
        let lock = DataDirLock::acquire(&lock_path, StartupId::generate())?;
        let data_dir = Self {
            root: root.clone(),
            lock,
        };
        data_dir.validate_children()?;
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

    /// Scheduler database path: `<data_dir>/scheduler.sqlite`.
    #[must_use]
    pub fn scheduler_db_path(&self) -> PathBuf {
        self.root.join(SCHEDULER_DB_NAME)
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

    /// Artifact cache directory: `<data>/cache/artifacts`.
    #[must_use]
    pub fn artifact_cache_dir(&self) -> PathBuf {
        self.root.join(CACHE).join(ARTIFACTS)
    }

    /// Private crash-recoverable staging directory for streamed deployment uploads.
    #[must_use]
    pub fn deployment_staging_dir(&self) -> PathBuf {
        self.root.join(DEPLOYMENT_STAGING)
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

    /// Read one bounded, regular, mode-0600 operator receipt without following symlinks.
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
        platform_id: &str,
        workerd_version: &str,
    ) -> Result<PathBuf, PlatformError> {
        if platform_id.is_empty() || workerd_version.is_empty() {
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
            platform_id: platform_id.to_owned(),
            unique_key: DURABLE_OBJECT_UNIQUE_KEY.to_owned(),
            workerd_version: workerd_version.to_owned(),
        };
        if marker.exists() || std::fs::symlink_metadata(&marker).is_ok() {
            fs::validate_owned_file(&marker, true)?;
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

    pub(crate) fn record_platform_id(&self, platform_id: &str) -> Result<(), PlatformError> {
        self.lock.write_metadata(Some(platform_id))
    }

    /// Create `control.sqlite` as a 0600 regular file after the master key is resolved.
    pub(crate) fn ensure_control_db(&self) -> Result<PathBuf, PlatformError> {
        let db_path = self.control_db_path();
        fs::validate_contained(&self.root, &db_path)?;
        fs::ensure_file_secure(&db_path)?;
        fs::validate_contained(&self.root, &db_path)?;
        Ok(db_path)
    }

    /// Create `scheduler.sqlite` as a 0600 regular file under the owned data directory.
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

    /// Explicitly quarantine an uninspectable scheduler database and create an empty replacement.
    ///
    /// The exclusive data-directory lock must be held by this [`DataDir`], so a running
    /// `platformd` cannot race the recovery. Only a verified alarm-only control authority
    /// permits rebuilding: Queue, Cron, and Workflow history require a full snapshot restore.
    /// The caller must subsequently run bounded alarm repair from live Durable Objects.
    pub fn recover_corrupt_scheduler_db(
        &self,
        backup_name: &str,
        busy_timeout_ms: u64,
        now_ms: i64,
    ) -> Result<PathBuf, PlatformError> {
        if !valid_scheduler_backup_name(backup_name) {
            return Err(PlatformError::new(
                open_compute_core::ErrorCode::PathInvalid,
                "scheduler recovery backup name is invalid",
            ));
        }
        let source = self.scheduler_db_path();
        fs::validate_contained(&self.root, &source)?;
        fs::validate_owned_file(&source, true)?;
        if crate::scheduler::inspect_scheduler_db(&source, busy_timeout_ms, now_ms).is_ok() {
            return Err(PlatformError::new(
                open_compute_core::ErrorCode::ConfigInvalid,
                "scheduler recovery refuses an intact database",
            ));
        }
        self.ensure_scheduler_rebuild_safe(busy_timeout_ms)?;

        let mut sources = Vec::new();
        for suffix in ["", "-wal", "-shm"] {
            let path = append_suffix(&source, suffix);
            if path.exists() || std::fs::symlink_metadata(&path).is_ok() {
                fs::validate_owned_file(&path, true)?;
                sources.push(path);
            }
        }

        let parent = self.root.join(DIAGNOSTICS).join(SCHEDULER_RECOVERY);
        fs::validate_contained(&self.root, &parent)?;
        fs::create_dir_secure(&parent)?;
        let backup = parent.join(backup_name);
        fs::validate_contained(&self.root, &backup)?;
        if backup.exists() || std::fs::symlink_metadata(&backup).is_ok() {
            return Err(PlatformError::new(
                open_compute_core::ErrorCode::PathInvalid,
                "scheduler recovery backup already exists",
            ));
        }
        fs::create_dir_secure(&backup)?;

        let mut moved = Vec::new();
        for from in sources {
            let file_name = from.file_name().ok_or_else(recovery_failed)?;
            let to = backup.join(file_name);
            if std::fs::rename(&from, &to).is_err() {
                for (restore_from, restore_to) in moved.into_iter().rev() {
                    let _ = std::fs::rename(restore_to, restore_from);
                }
                let _ = std::fs::remove_dir(&backup);
                return Err(recovery_failed());
            }
            moved.push((from, to));
        }
        fs::fsync_dir(&self.root)?;
        fs::fsync_dir(&backup)?;

        let replacement = self.ensure_scheduler_db()?;
        match crate::scheduler::SchedulerStore::open(&replacement, busy_timeout_ms, now_ms) {
            Ok(store) => {
                drop(store);
                fs::fsync_dir(&self.root)?;
                Ok(backup)
            }
            Err(error) => {
                for suffix in ["-shm", "-wal", ""] {
                    let path = append_suffix(&replacement, suffix);
                    if path.exists() || std::fs::symlink_metadata(&path).is_ok() {
                        let _ = std::fs::remove_file(path);
                    }
                }
                let mut restored = true;
                for (from, to) in moved.into_iter().rev() {
                    if std::fs::rename(to, from).is_err() {
                        restored = false;
                    }
                }
                let _ = std::fs::remove_dir(&backup);
                let _ = fs::fsync_dir(&self.root);
                if restored {
                    Err(error)
                } else {
                    Err(recovery_failed())
                }
            }
        }
    }

    fn ensure_scheduler_rebuild_safe(&self, busy_timeout_ms: u64) -> Result<(), PlatformError> {
        let path = self.control_db_path();
        fs::validate_contained(&self.root, &path)?;
        fs::validate_owned_file(&path, true)?;
        let control = crate::ControlDb::open_readonly_wal_aware(&path, busy_timeout_ms)?;
        control.quick_check()?;
        let schema = crate::migrations::inspect_schema(&control)?;
        // These are checked schema-owned tables, never operator-supplied SQL identifiers.
        for (since, table) in [
            (8, "queues"),
            (10, "cron_activations"),
            (11, "workflow_instance_referrers"),
            (11, "workflow_instance_operations"),
        ] {
            if schema >= since {
                let retained: bool = control.with_read(|connection| {
                    connection
                        .query_row(
                            &format!("SELECT EXISTS(SELECT 1 FROM {table})"),
                            [],
                            |row| row.get(0),
                        )
                        .map_err(|_| recovery_failed())
                })?;
                if retained {
                    return Err(PlatformError::new(
                        open_compute_core::ErrorCode::SchedulerUnavailable,
                        "scheduler retains product authority; full snapshot restore is required",
                    ));
                }
            }
        }
        if schema >= 11 {
            // A purge can already have released its control reservation while its scheduler
            // receipt still awaits acknowledgement. Immutable Workflow versions are never deleted,
            // so this catalog evidence also fences recovery when the corrupt file cannot
            // reliably prove whether such receipts or operation watermarks remain.
            let durable_workflow: bool = control.with_read(|connection| {
                connection
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM workflow_versions)",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(|_| recovery_failed())
            })?;
            if durable_workflow {
                return Err(PlatformError::new(
                    open_compute_core::ErrorCode::SchedulerUnavailable,
                    "scheduler may retain durable Workflow authority; full snapshot restore is required",
                ));
            }
        }
        Ok(())
    }

    fn validate_children(&self) -> Result<(), PlatformError> {
        for rel in [
            LOCK_NAME,
            CONTROL_DB_NAME,
            SCHEDULER_DB_NAME,
            KEYS,
            RUNTIME,
            CACHE,
            DEPLOYMENT_STAGING,
            BACKUP_STAGING,
            DIAGNOSTICS,
        ] {
            let child = self.root.join(rel);
            fs::validate_contained(&self.root, &child)?;
        }
        fs::validate_owned_file(&self.root.join(LOCK_NAME), true)?;
        let db_path = self.root.join(CONTROL_DB_NAME);
        if db_path.exists() || std::fs::symlink_metadata(&db_path).is_ok() {
            fs::validate_owned_file(&db_path, true)?;
        }
        let scheduler_path = self.root.join(SCHEDULER_DB_NAME);
        if scheduler_path.exists() || std::fs::symlink_metadata(&scheduler_path).is_ok() {
            fs::validate_owned_file(&scheduler_path, true)?;
        }
        for dir in [
            self.keys_dir(),
            self.root.join(RUNTIME),
            self.root.join(CACHE),
            self.root.join(CACHE).join(ARTIFACTS),
            self.root.join(CACHE).join(ARTIFACTS).join(SHA256),
            self.deployment_staging_dir(),
            self.root.join(BACKUP_STAGING),
            self.root.join(DIAGNOSTICS),
            self.root.join(DIAGNOSTICS).join(FAILED_STARTS),
        ] {
            fs::validate_owned_dir(&dir)?;
            fs::validate_contained(&self.root, &dir)?;
        }
        Ok(())
    }

    fn clear_deployment_staging(&self) -> Result<(), PlatformError> {
        let staging = self.deployment_staging_dir();
        for entry in std::fs::read_dir(&staging).map_err(|_| {
            PlatformError::new(
                open_compute_core::ErrorCode::PathInvalid,
                "failed to inspect deployment staging directory",
            )
        })? {
            let entry = entry.map_err(|_| {
                PlatformError::new(
                    open_compute_core::ErrorCode::PathInvalid,
                    "failed to inspect deployment staging entry",
                )
            })?;
            let kind = entry.file_type().map_err(|_| {
                PlatformError::new(
                    open_compute_core::ErrorCode::PathInvalid,
                    "failed to inspect deployment staging entry type",
                )
            })?;
            if !kind.is_file() || kind.is_symlink() {
                return Err(PlatformError::new(
                    open_compute_core::ErrorCode::PathInvalid,
                    "deployment staging contains a non-regular entry",
                ));
            }
            std::fs::remove_file(entry.path()).map_err(|_| {
                PlatformError::new(
                    open_compute_core::ErrorCode::PathInvalid,
                    "failed to clear stale deployment staging file",
                )
            })?;
        }
        Ok(())
    }
}

/// Read one bounded operator receipt from an existing data root without following symlinks.
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
    let mut file = fs::open_nofollow(&path, false, false)?;
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
    let mut file = fs::open_nofollow(path, false, false).map_err(|_| do_storage_unavailable())?;
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

fn valid_scheduler_backup_name(name: &str) -> bool {
    (1..=80).contains(&name.len())
        && name.starts_with("scheduler-corrupt-")
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn append_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

fn recovery_failed() -> PlatformError {
    PlatformError::new(
        open_compute_core::ErrorCode::PathInvalid,
        "scheduler corrupt-file recovery failed",
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
    fs::create_dir_secure(&root.join(CACHE))?;
    fs::create_dir_secure(&root.join(CACHE).join(ARTIFACTS))?;
    fs::create_dir_secure(&root.join(CACHE).join(ARTIFACTS).join(SHA256))?;
    fs::create_dir_secure(&root.join(DEPLOYMENT_STAGING))?;
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

/// Paths that must not exist after a clean P0.1 bootstrap.
#[must_use]
#[cfg(any(test, feature = "test-support"))]
pub fn future_resource_paths(root: &Path) -> Vec<PathBuf> {
    FORBIDDEN_PRECREATE
        .iter()
        .map(|name| root.join(name))
        .collect()
}

/// Layout directories created for P0.1.
#[must_use]
#[cfg(any(test, feature = "test-support"))]
pub fn expected_directories(root: &Path) -> Vec<PathBuf> {
    vec![
        root.join(KEYS),
        root.join(RUNTIME),
        root.join(CACHE),
        root.join(CACHE).join(ARTIFACTS),
        root.join(CACHE).join(ARTIFACTS).join(SHA256),
        root.join(DEPLOYMENT_STAGING),
        root.join(BACKUP_STAGING),
        root.join(DIAGNOSTICS),
        root.join(DIAGNOSTICS).join(FAILED_STARTS),
    ]
}
