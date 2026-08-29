//! P1 offline full-platform snapshot commands.

pub use crate::backup_attestation::{BackupRestoreSmokeResult, backup_attest_restore_smoke};
pub use crate::backup_retention::{
    BackupRetentionEntry, BackupRetentionPlan, backup_retention_plan,
};
use crate::capabilities::{platform_capabilities, platform_config_policy_sha256};
use crate::config_load::LoadedConfig;
use open_compute_artifacts::{
    ArtifactRef, ArtifactStore, R2BucketIdentity, R2ObjectStore, S3ArtifactClient,
    SnapshotObjectStore, preflight_r2, preflight_s3, resolve_s3_credentials,
};
use open_compute_core::{
    ErrorCode, PlatformError, PlatformSnapshotManifestV1, ResourceState,
    SnapshotImmutableReferenceV1, StartupId,
};
use open_compute_runtime::{assert_no_live_orphan, embedded_runtime_lock};
use open_compute_storage::{
    ControlDb, DataDir, PreparePlatformSnapshotRequest, R2BucketRepository, RestoreStagingCleanup,
    cleanup_restore_staging, cleanup_stale_snapshot_staging, estimate_platform_snapshot_bytes,
    inspect_control_db, inspect_master_key, inspect_snapshot_immutable_references,
    prepare_platform_snapshot, sign_snapshot_manifest, verify_snapshot_manifest_mac,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::Write;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

/// Result of a completed snapshot create.
#[derive(Clone, Debug, Serialize)]
pub struct BackupCreateResult {
    /// Output schema version.
    pub schema_version: u32,
    /// Committed snapshot identity.
    pub snapshot_id: String,
    /// Stable source platform identity.
    pub platform_id: String,
    /// Number of locally owned objects.
    pub files: u32,
    /// Total locally owned bytes.
    pub bytes: u64,
    /// Audit timestamp.
    pub created_at_ms: i64,
    /// End-to-end create duration.
    pub duration_ms: u64,
}

/// Result of a snapshot inspection.
#[derive(Clone, Debug, Serialize)]
pub struct BackupInspectResult {
    /// Output schema version.
    pub schema_version: u32,
    /// Snapshot identity.
    pub snapshot_id: String,
    /// Platform identity.
    pub platform_id: String,
    /// Whether object bytes were fully streamed and verified.
    pub verified: bool,
    /// Local file count.
    pub files: u32,
    /// Local byte total.
    pub bytes: u64,
    /// Explicit capability boundary for tenant R2 data.
    pub r2_point_in_time_recovery: bool,
}

/// Result of a freshly installed platform snapshot.
#[derive(Clone, Debug, Serialize)]
pub struct BackupRestoreResult {
    /// Output schema version.
    pub schema_version: u32,
    /// Restored snapshot identity.
    pub snapshot_id: String,
    /// Restored platform identity.
    pub platform_id: String,
    /// Installed target data directory.
    pub data_dir: String,
    /// Restored local byte total.
    pub bytes: u64,
    /// Restore completion timestamp.
    pub restored_at_ms: i64,
    /// End-to-end restore duration.
    pub duration_ms: u64,
}

/// Aggregate result from exact-layout cleanup of old incomplete uploads.
#[derive(Clone, Debug, Serialize)]
pub struct BackupCleanupResult {
    /// Output schema version.
    pub schema_version: u32,
    /// Number of incomplete snapshot prefixes removed.
    pub prefixes: u64,
    /// Number of exact objects removed.
    pub objects: u64,
    /// Aggregate provider-reported bytes removed.
    pub bytes: u64,
    /// Number of stale local staging directories removed.
    pub local_directories: u64,
    /// Number of exact local staging files removed.
    pub local_files: u64,
    /// Filesystem-reported local staging bytes removed.
    pub local_bytes: u64,
}

/// Create one committed snapshot while holding the shared offline data-dir lock.
pub async fn backup_create(
    loaded: &LoadedConfig,
    label: &str,
) -> Result<BackupCreateResult, PlatformError> {
    let started = Instant::now();
    validate_label(label)?;
    let capabilities = platform_capabilities(&loaded.config)?;
    let data_dir = DataDir::acquire_existing_offline(&loaded.config.storage)?;
    assert_runtime_quiescent(&data_dir)?;
    let grace_deadline = incomplete_snapshot_deadline(loaded)?;
    cleanup_stale_snapshot_staging(&data_dir, grace_deadline)?;
    let key = inspect_master_key(&loaded.config.storage)?;
    let (_, identity) = inspect_control_db(
        &data_dir.control_db_path(),
        loaded.config.storage.sqlite_busy_timeout_ms,
    )?;
    if key.fingerprint() != identity.master_key_id {
        return Err(PlatformError::new(
            ErrorCode::MasterKeyMismatch,
            "snapshot key does not match platform authority",
        ));
    }
    ensure_snapshot_headroom(loaded, 0)?;
    let client = connect_snapshot_client(loaded)?;
    preflight_s3(&client, identity.platform_id, StartupId::generate()).await?;
    preflight_r2(&client, identity.platform_id, StartupId::generate()).await?;
    let objects = SnapshotObjectStore::new(client.clone(), identity.platform_id);
    objects.cleanup_incomplete(grace_deadline).await?;
    let artifact_store = ArtifactStore::new(client.clone());
    let r2_store = R2ObjectStore::new(client);
    let snapshot_id = Uuid::now_v7().hyphenated().to_string();
    let created_at_ms = unix_ms();
    let object_prefix = objects.object_prefix(&snapshot_id)?;
    let s3_authority_fingerprint = objects.authority_fingerprint();
    let r2_prefix_fingerprint = objects.r2_prefix_fingerprint();
    let config_policy_sha256 = platform_config_policy_sha256(loaded)?;
    let request = PreparePlatformSnapshotRequest {
        snapshot_id: &snapshot_id,
        label,
        created_at_ms,
        release: capabilities.release.clone(),
        master_key_fingerprint: key.fingerprint(),
        s3_authority_fingerprint: &s3_authority_fingerprint,
        r2_prefix_fingerprint: &r2_prefix_fingerprint,
        config_policy_sha256: &config_policy_sha256,
        object_prefix: &object_prefix,
        hardening: &loaded.config.hardening,
        sqlite_busy_timeout_ms: loaded.config.storage.sqlite_busy_timeout_ms,
    };
    let estimated =
        estimate_platform_snapshot_bytes(&data_dir, &request, &identity.platform_id.to_string())?;
    ensure_snapshot_headroom(loaded, estimated)?;
    let mut prepared = prepare_platform_snapshot(&data_dir, &request)?;
    // The prepared bytes are already reflected in `statvfs`; do not count them twice.
    ensure_snapshot_headroom(loaded, 0)?;
    prepared.manifest.immutable_references = collect_and_verify_external_references(
        loaded,
        &artifact_store,
        &r2_store,
        &objects,
        identity.platform_id,
    )
    .await?;
    for file in &prepared.files {
        objects
            .put_file(
                &file.entry.object_key,
                &file.staging_path,
                &file.entry.sha256,
                file.entry.size,
            )
            .await?;
    }
    sign_snapshot_manifest(&mut prepared.manifest, &key)?;
    prepared.manifest.validate(
        loaded.config.hardening.max_snapshot_files,
        loaded.config.hardening.max_snapshot_file_bytes,
        loaded.config.hardening.max_snapshot_total_bytes,
    )?;
    let manifest_bytes = serde_json::to_vec(&prepared.manifest).map_err(|_| snapshot_invalid())?;
    objects
        .put_manifest(
            &snapshot_id,
            &manifest_bytes,
            loaded.config.hardening.max_snapshot_manifest_bytes,
        )
        .await?;
    let committed = load_manifest(loaded, &objects, &snapshot_id, &key).await?;
    verify_snapshot_objects(&objects, &committed, true).await?;
    let duration_ms = elapsed_ms(started);
    write_operation_receipt(
        &data_dir,
        "last-snapshot.json",
        &serde_json::json!({
            "schema_version": 1,
            "snapshot_id": snapshot_id,
            "platform_id": identity.platform_id,
            "created_at_ms": created_at_ms,
            "files": committed.totals.files,
            "bytes": committed.totals.bytes,
            "duration_ms": duration_ms,
            "verified": true,
        }),
    )?;
    Ok(BackupCreateResult {
        schema_version: 1,
        snapshot_id,
        platform_id: identity.platform_id.to_string(),
        files: committed.totals.files,
        bytes: committed.totals.bytes,
        created_at_ms,
        duration_ms,
    })
}

/// List committed manifests for the current local platform authority.
pub async fn backup_list(loaded: &LoadedConfig) -> Result<Vec<BackupInspectResult>, PlatformError> {
    let (_, identity) = inspect_control_db(
        &loaded.config.storage.data_dir.join("control.sqlite"),
        loaded.config.storage.sqlite_busy_timeout_ms,
    )?;
    let key = inspect_master_key(&loaded.config.storage)?;
    let objects = SnapshotObjectStore::new(connect_snapshot_client(loaded)?, identity.platform_id);
    let mut results = Vec::new();
    for snapshot in objects.list_committed().await? {
        let manifest = load_manifest(loaded, &objects, &snapshot.snapshot_id, &key).await?;
        results.push(inspect_result(&manifest, false));
    }
    Ok(results)
}

/// Inspect and optionally fully verify one committed snapshot.
pub async fn backup_inspect(
    loaded: &LoadedConfig,
    snapshot_id: &str,
    verify: bool,
) -> Result<BackupInspectResult, PlatformError> {
    let key = inspect_master_key(&loaded.config.storage)?;
    let client = connect_snapshot_client(loaded)?;
    let objects = if loaded
        .config
        .storage
        .data_dir
        .join("control.sqlite")
        .exists()
    {
        let (_, identity) = inspect_control_db(
            &loaded.config.storage.data_dir.join("control.sqlite"),
            loaded.config.storage.sqlite_busy_timeout_ms,
        )?;
        SnapshotObjectStore::new(client, identity.platform_id)
    } else {
        SnapshotObjectStore::discover(client, snapshot_id).await?
    };
    let manifest = load_manifest(loaded, &objects, snapshot_id, &key).await?;
    if verify {
        verify_snapshot_objects(&objects, &manifest, true).await?;
    }
    Ok(inspect_result(&manifest, verify))
}

/// Delete exactly the authenticated objects in one committed snapshot, manifest last.
pub async fn backup_delete(
    loaded: &LoadedConfig,
    snapshot_id: &str,
) -> Result<BackupInspectResult, PlatformError> {
    let _data_dir = DataDir::acquire_existing_offline(&loaded.config.storage)?;
    assert_runtime_quiescent(&_data_dir)?;
    let key = inspect_master_key(&loaded.config.storage)?;
    let (_, identity) = inspect_control_db(
        &loaded.config.storage.data_dir.join("control.sqlite"),
        loaded.config.storage.sqlite_busy_timeout_ms,
    )?;
    let objects = SnapshotObjectStore::new(connect_snapshot_client(loaded)?, identity.platform_id);
    let manifest = load_manifest(loaded, &objects, snapshot_id, &key).await?;
    verify_snapshot_objects(&objects, &manifest, true).await?;
    for file in &manifest.files {
        objects.delete_exact(&file.object_key).await?;
    }
    objects
        .delete_exact(&objects.manifest_key(snapshot_id)?)
        .await?;
    Ok(inspect_result(&manifest, true))
}

/// Delete only exact-layout incomplete uploads older than the configured grace period.
pub async fn backup_cleanup_incomplete(
    loaded: &LoadedConfig,
) -> Result<BackupCleanupResult, PlatformError> {
    let data_dir = DataDir::acquire_existing_offline(&loaded.config.storage)?;
    assert_runtime_quiescent(&data_dir)?;
    let (_, identity) = inspect_control_db(
        &data_dir.control_db_path(),
        loaded.config.storage.sqlite_busy_timeout_ms,
    )?;
    let objects = SnapshotObjectStore::new(connect_snapshot_client(loaded)?, identity.platform_id);
    let grace_deadline = incomplete_snapshot_deadline(loaded)?;
    let local = cleanup_stale_snapshot_staging(&data_dir, grace_deadline)?;
    let result = objects.cleanup_incomplete(grace_deadline).await?;
    Ok(BackupCleanupResult {
        schema_version: 1,
        prefixes: result.prefixes,
        objects: result.objects,
        bytes: result.bytes,
        local_directories: local.directories,
        local_files: local.files,
        local_bytes: local.bytes,
    })
}

/// Remove one exact failed fresh-host restore staging tree selected by `UUIDv7`.
pub fn backup_cleanup_restore(
    loaded: &LoadedConfig,
    staging_id: &str,
) -> Result<RestoreStagingCleanup, PlatformError> {
    cleanup_restore_staging(
        &loaded.config.storage.data_dir,
        staging_id,
        loaded.config.hardening.max_snapshot_files,
        loaded.config.hardening.max_snapshot_file_bytes,
        loaded.config.hardening.max_snapshot_total_bytes,
    )
}

/// Restore one exact-release snapshot into a nonexistent or empty fresh-host data directory.
pub async fn backup_restore(
    loaded: &LoadedConfig,
    snapshot_id: &str,
) -> Result<BackupRestoreResult, PlatformError> {
    let started = Instant::now();
    let target = &loaded.config.storage.data_dir;
    if loaded.config.storage.master_key_env.is_none()
        && loaded.config.storage.master_key_file.starts_with(target)
    {
        return Err(PlatformError::new(
            ErrorCode::RestoreInvalid,
            "fresh-host restore requires a recovery master key outside data_dir or via env",
        ));
    }
    let key = inspect_master_key(&loaded.config.storage)?;
    let client = connect_snapshot_client(loaded)?;
    let objects = SnapshotObjectStore::discover(client, snapshot_id).await?;
    let manifest = load_manifest(loaded, &objects, snapshot_id, &key).await?;
    let current_release = platform_capabilities(&loaded.config)?.release;
    if manifest.source_release != current_release {
        return Err(PlatformError::new(
            ErrorCode::ReleaseUnsupported,
            "restore requires the exact source release identity",
        ));
    }
    if manifest.config_policy_sha256 != platform_config_policy_sha256(loaded)? {
        return Err(PlatformError::new(
            ErrorCode::ReleaseUnsupported,
            "restore requires the snapshot source storage and product policy",
        ));
    }
    verify_snapshot_objects(&objects, &manifest, true).await?;
    ensure_restore_headroom(loaded, manifest.totals.bytes)?;
    let restore = open_compute_storage::RestoreTarget::acquire(target)
        .map_err(|error| restore_stage(&error, "restore target acquisition failed"))?;
    for file in &manifest.files {
        let destination = restore
            .destination_for(&file.restore_path)
            .map_err(|error| restore_stage(&error, "restore destination validation failed"))?;
        objects
            .download_file(&file.object_key, &destination, &file.sha256, file.size)
            .await?;
    }
    let restored_at_ms = unix_ms();
    let duration_ms = elapsed_ms(started);
    let receipt = serde_json::to_vec(&serde_json::json!({
        "schema_version": 1,
        "snapshot_id": manifest.snapshot_id,
        "platform_id": manifest.platform_id,
        "source_release": manifest.source_release,
        "manifest_mac": manifest.manifest_mac,
        "bytes": manifest.totals.bytes,
        "restored_at_ms": restored_at_ms,
        "duration_ms": duration_ms,
        "smoke_verified": false,
        "verified": true,
    }))
    .map_err(|_| snapshot_invalid())?;
    let installed = restore.validate_and_publish(
        &manifest,
        key.fingerprint(),
        loaded.config.storage.sqlite_busy_timeout_ms,
        &receipt,
    )?;
    Ok(BackupRestoreResult {
        schema_version: 1,
        snapshot_id: manifest.snapshot_id,
        platform_id: manifest.platform_id,
        data_dir: installed.to_string_lossy().into_owned(),
        bytes: manifest.totals.bytes,
        restored_at_ms,
        duration_ms,
    })
}

/// Load, authenticate, and stream-verify a committed snapshot for another offline workflow.
pub(crate) async fn verified_snapshot(
    loaded: &LoadedConfig,
    snapshot_id: &str,
) -> Result<PlatformSnapshotManifestV1, PlatformError> {
    let key = inspect_master_key(&loaded.config.storage)?;
    let client = connect_snapshot_client(loaded)?;
    let objects = if loaded
        .config
        .storage
        .data_dir
        .join("control.sqlite")
        .exists()
    {
        let (_, identity) = inspect_control_db(
            &loaded.config.storage.data_dir.join("control.sqlite"),
            loaded.config.storage.sqlite_busy_timeout_ms,
        )?;
        SnapshotObjectStore::new(client, identity.platform_id)
    } else {
        SnapshotObjectStore::discover(client, snapshot_id).await?
    };
    let manifest = load_manifest(loaded, &objects, snapshot_id, &key).await?;
    verify_snapshot_objects(&objects, &manifest, true).await?;
    Ok(manifest)
}

pub(crate) fn assert_runtime_quiescent(data_dir: &DataDir) -> Result<(), PlatformError> {
    let (lock, _) = embedded_runtime_lock()?;
    let (_, target) = lock.current_target()?;
    assert_no_live_orphan(
        &data_dir.runtime_dir().join("child.lease"),
        &target.binary_sha256,
    )
}

/// Write one command result in deterministic human or JSON form.
pub fn write_result<T: Serialize>(
    value: &T,
    out: &mut impl Write,
    json: bool,
    human: &str,
) -> Result<(), PlatformError> {
    if json {
        serde_json::to_writer(&mut *out, value).map_err(|_| snapshot_invalid())?;
        writeln!(out).map_err(|_| snapshot_invalid())?;
    } else {
        writeln!(out, "{human}").map_err(|_| snapshot_invalid())?;
    }
    Ok(())
}

async fn collect_and_verify_external_references(
    loaded: &LoadedConfig,
    artifacts: &ArtifactStore,
    r2: &R2ObjectStore,
    snapshots: &SnapshotObjectStore,
    platform_id: open_compute_core::PlatformId,
) -> Result<Vec<SnapshotImmutableReferenceV1>, PlatformError> {
    let mut references = inspect_snapshot_immutable_references(
        &loaded.config.storage.data_dir.join("control.sqlite"),
        loaded.config.storage.sqlite_busy_timeout_ms,
        &loaded.config.s3.prefix,
    )
    .map_err(|error| snapshot_stage(&error, "snapshot external reference inventory failed"))?;
    let mut backup_manifests = Vec::new();
    for reference in &references {
        match reference.role.as_str() {
            "deployment_artifact" => {
                let artifact = ArtifactRef::new(1, &reference.sha256, reference.size)?;
                if artifact.physical_key(&loaded.config.s3.prefix) != reference.object_key {
                    return Err(PlatformError::new(
                        ErrorCode::SnapshotInvalid,
                        "snapshot deployment artifact reference is outside the configured authority",
                    ));
                }
                artifacts.head(&artifact).await?;
                artifacts
                    .download_verified(&artifact, &mut std::io::sink())
                    .await?;
            }
            "kv_backup" => {
                artifacts
                    .download_kv_backup(
                        &reference.object_key,
                        &reference.sha256,
                        reference.size,
                        &mut std::io::sink(),
                    )
                    .await?;
                let key = artifacts.kv_backup_manifest_key(&reference.object_key)?;
                let bytes = artifacts.get_kv_backup_manifest(&key).await?;
                backup_manifests.push(SnapshotImmutableReferenceV1 {
                    role: "kv_backup".to_owned(),
                    sha256: hex::encode(Sha256::digest(&bytes)),
                    object_key: key,
                    size: bytes.len() as u64,
                });
            }
            "d1_backup" => {
                artifacts
                    .download_d1_backup(
                        &reference.object_key,
                        &reference.sha256,
                        reference.size,
                        &mut std::io::sink(),
                    )
                    .await?;
                let key = artifacts.d1_backup_manifest_key(&reference.object_key)?;
                let bytes = artifacts.get_d1_backup_manifest(&key).await?;
                backup_manifests.push(SnapshotImmutableReferenceV1 {
                    role: "d1_backup".to_owned(),
                    sha256: hex::encode(Sha256::digest(&bytes)),
                    object_key: key,
                    size: bytes.len() as u64,
                });
            }
            _ => {
                return Err(PlatformError::new(
                    ErrorCode::SnapshotInvalid,
                    "snapshot external reference role is unsupported",
                ));
            }
        }
    }
    references.extend(backup_manifests);
    let control = ControlDb::open_readonly(
        &loaded.config.storage.data_dir.join("control.sqlite"),
        loaded.config.storage.sqlite_busy_timeout_ms,
    )?;
    for bucket in R2BucketRepository::new(&control).list_all()? {
        if bucket.resource.state == ResourceState::Tombstoned {
            continue;
        }
        if bucket.provider_config_sha256 != r2.authority_sha256() {
            return Err(PlatformError::new(
                ErrorCode::SnapshotInvalid,
                "snapshot R2 bucket authority does not match the configured authority",
            ));
        }
        let locator = r2.locator(bucket.resource.id, &bucket.physical_prefix)?;
        let expected = R2BucketIdentity {
            schema_version: 1,
            platform_id,
            resource_id: bucket.resource.id,
            created_at_ms: bucket.resource.created_at_ms,
        };
        let actual = r2.read_identity(&locator).await?;
        if actual.as_ref() != Some(&expected) {
            return Err(PlatformError::new(
                ErrorCode::SnapshotInvalid,
                "snapshot R2 bucket identity marker does not match local authority",
            ));
        }
        let bytes = serde_json::to_vec(&expected).map_err(|_| snapshot_invalid())?;
        references.push(SnapshotImmutableReferenceV1 {
            role: "r2_bucket_marker".to_owned(),
            sha256: hex::encode(Sha256::digest(&bytes)),
            object_key: locator.identity_marker_key(),
            size: bytes.len() as u64,
        });
    }
    references.sort_by(|left, right| left.object_key.cmp(&right.object_key));
    for reference in references
        .iter()
        .filter(|reference| reference.role == "r2_bucket_marker")
    {
        snapshots
            .verify_external_reference(&reference.object_key, &reference.sha256, reference.size)
            .await?;
    }
    Ok(references)
}

pub(crate) async fn load_manifest(
    loaded: &LoadedConfig,
    objects: &SnapshotObjectStore,
    snapshot_id: &str,
    key: &open_compute_storage::MasterKey,
) -> Result<PlatformSnapshotManifestV1, PlatformError> {
    let bytes = objects
        .get_manifest(
            snapshot_id,
            loaded.config.hardening.max_snapshot_manifest_bytes,
        )
        .await?;
    let manifest: PlatformSnapshotManifestV1 =
        serde_json::from_slice(&bytes).map_err(|_| snapshot_invalid())?;
    manifest.validate(
        loaded.config.hardening.max_snapshot_files,
        loaded.config.hardening.max_snapshot_file_bytes,
        loaded.config.hardening.max_snapshot_total_bytes,
    )?;
    if manifest.snapshot_id != snapshot_id
        || manifest.master_key_fingerprint != key.fingerprint()
        || manifest.s3_authority_fingerprint != objects.authority_fingerprint()
        || manifest.r2_prefix_fingerprint != objects.r2_prefix_fingerprint()
    {
        return Err(snapshot_invalid());
    }
    let prefix = objects.object_prefix(snapshot_id)?;
    if manifest
        .files
        .iter()
        .enumerate()
        .any(|(index, file)| file.object_key != format!("{prefix}{index:06}.bin"))
    {
        return Err(snapshot_invalid());
    }
    verify_snapshot_manifest_mac(&manifest, key)?;
    Ok(manifest)
}

async fn verify_snapshot_objects(
    objects: &SnapshotObjectStore,
    manifest: &PlatformSnapshotManifestV1,
    include_external: bool,
) -> Result<(), PlatformError> {
    for file in &manifest.files {
        objects
            .verify_file(&file.object_key, &file.sha256, file.size)
            .await?;
    }
    if include_external {
        for reference in &manifest.immutable_references {
            objects
                .verify_external_reference(&reference.object_key, &reference.sha256, reference.size)
                .await?;
        }
    }
    Ok(())
}

pub(crate) fn connect_snapshot_client(
    loaded: &LoadedConfig,
) -> Result<S3ArtifactClient, PlatformError> {
    let credentials = resolve_s3_credentials(&loaded.config.s3)?;
    S3ArtifactClient::connect(
        &loaded.config.s3,
        &credentials,
        loaded
            .config
            .hardening
            .max_snapshot_file_bytes
            .max(loaded.config.cache.max_artifact_bytes),
    )
}

fn ensure_snapshot_headroom(loaded: &LoadedConfig, staged_bytes: u64) -> Result<(), PlatformError> {
    let stat = rustix::fs::statvfs(&loaded.config.storage.data_dir).map_err(|_| {
        PlatformError::new(
            ErrorCode::StoragePressure,
            "snapshot free space could not be measured",
        )
    })?;
    let required = loaded
        .config
        .storage
        .free_space_hard_bytes
        .saturating_add(loaded.config.hardening.snapshot_staging_margin_bytes)
        .saturating_add(staged_bytes);
    let available = stat.f_bavail.saturating_mul(stat.f_frsize);
    if available < required {
        return Err(PlatformError::new(
            ErrorCode::StoragePressure,
            "snapshot staging would violate the host storage reserve",
        ));
    }
    Ok(())
}

fn ensure_restore_headroom(loaded: &LoadedConfig, restore_bytes: u64) -> Result<(), PlatformError> {
    let parent = loaded
        .config
        .storage
        .data_dir
        .parent()
        .ok_or_else(snapshot_invalid)?;
    let stat = rustix::fs::statvfs(parent).map_err(|_| {
        PlatformError::new(
            ErrorCode::StoragePressure,
            "restore free space could not be measured",
        )
    })?;
    let required = loaded
        .config
        .storage
        .free_space_hard_bytes
        .saturating_add(loaded.config.hardening.snapshot_staging_margin_bytes)
        .saturating_add(restore_bytes);
    if stat.f_bavail.saturating_mul(stat.f_frsize) < required {
        return Err(PlatformError::new(
            ErrorCode::StoragePressure,
            "restore staging would violate the host storage reserve",
        ));
    }
    Ok(())
}

fn write_operation_receipt(
    data_dir: &DataDir,
    name: &str,
    value: &serde_json::Value,
) -> Result<(), PlatformError> {
    let bytes = serde_json::to_vec(value).map_err(|_| snapshot_invalid())?;
    data_dir.write_operation_receipt(name, &bytes)
}

fn inspect_result(manifest: &PlatformSnapshotManifestV1, verified: bool) -> BackupInspectResult {
    BackupInspectResult {
        schema_version: 1,
        snapshot_id: manifest.snapshot_id.clone(),
        platform_id: manifest.platform_id.clone(),
        verified,
        files: manifest.totals.files,
        bytes: manifest.totals.bytes,
        r2_point_in_time_recovery: false,
    }
}

fn validate_label(label: &str) -> Result<(), PlatformError> {
    if label.is_empty() || label.len() > 128 || label.bytes().any(|byte| byte.is_ascii_control()) {
        return Err(snapshot_invalid());
    }
    Ok(())
}

fn incomplete_snapshot_deadline(loaded: &LoadedConfig) -> Result<SystemTime, PlatformError> {
    SystemTime::now()
        .checked_sub(Duration::from_millis(
            loaded.config.hardening.incomplete_snapshot_grace_ms,
        ))
        .ok_or_else(snapshot_invalid)
}

fn unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(i64::MAX)
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn snapshot_invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::SnapshotInvalid,
        "platform snapshot operation failed validation",
    )
}

fn snapshot_stage(error: &PlatformError, message: &'static str) -> PlatformError {
    PlatformError::new(error.code(), message)
}

fn restore_stage(error: &PlatformError, message: &'static str) -> PlatformError {
    PlatformError::new(error.code(), message)
}
