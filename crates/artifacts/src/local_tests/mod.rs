use crate::local::LocalFaultPoint;
use crate::{
    BackendError, CustomerKey, GetOptions, HeadOptions, ObjectBackend, ObjectHttpMetadata,
    ObjectKey, ObjectMetadata, ObjectRange, ObjectSource, PutMode, PutOptions, R2BucketIdentity,
    R2ChecksumAlgorithm, R2GetResult, R2HttpMetadata, R2MultipartCreateOptions, R2ObjectStore,
    R2PutOptions, R2Range, R2SsecKey, R2StorageClass, R2UploadSource, UserObjectKey, hash_bytes,
};
use bytes::Bytes;
use md5::Digest as _;
use open_compute_core::{
    ErrorCode, LocalObjectStorageConfig, ObjectStorageKind, PlatformId, ResourceId,
};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tokio::io::AsyncReadExt as _;

const LIMIT: u64 = 4 * 1024 * 1024;

struct Fixture {
    _temp: tempfile::TempDir,
    config: LocalObjectStorageConfig,
    platform_id: PlatformId,
    backend: ObjectBackend,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let config = LocalObjectStorageConfig {
            path: temp.path().join("objects"),
            free_space_soft_bytes: 1,
            free_space_hard_bytes: 1,
            partial_grace_ms: 1,
            ..LocalObjectStorageConfig::default()
        };
        let platform_id = PlatformId::generate();
        let backend = ObjectBackend::open_local(&config, platform_id, LIMIT).unwrap();
        Self {
            _temp: temp,
            config,
            platform_id,
            backend,
        }
    }

    fn object_file(&self, key: &ObjectKey) -> PathBuf {
        let mut path = self.config.path.join("objects");
        for segment in key.as_str().split('/') {
            path.push(segment);
        }
        path.join("object.ocobj")
    }
}

fn options(mode: PutMode) -> PutOptions {
    PutOptions {
        mode,
        metadata: ObjectMetadata {
            user: BTreeMap::from([("name".to_owned(), "value".to_owned())]),
            http: ObjectHttpMetadata {
                content_type: Some("text/plain".to_owned()),
                cache_control: Some("max-age=60".to_owned()),
                ..ObjectHttpMetadata::default()
            },
            ..ObjectMetadata::default()
        },
        customer_key: None,
    }
}

async fn bytes(backend: &ObjectBackend, key: &ObjectKey, options: GetOptions) -> Bytes {
    backend
        .get(key, options)
        .await
        .unwrap()
        .body
        .collect()
        .await
        .unwrap()
        .into_bytes()
}

mod physical_key_grammar_rejects_ambiguous_or_unsafe_names;

mod backend_facade_diagnostics_and_public_errors_are_complete;

mod local_contract_put_head_get_range_list_conditions_delete_and_restart;

mod local_create_only_is_atomic_under_concurrency;

mod opened_file_sources_require_exact_private_single_link_files;

mod local_object_tree_rejects_symlink_ancestors_and_special_leaves;

mod local_envelope_truncation_and_plaintext_tamper_fail_closed;

mod local_ssec_is_authenticated_chunked_and_never_persists_plaintext;

mod local_multipart_persists_encrypted_parts_and_publishes_atomically;

mod local_typed_r2_round_trips_metadata_conditions_ssec_and_multipart;

mod local_root_marker_lock_capacity_corruption_and_relocation_fail_closed;

mod local_root_rejects_symlinks_and_insecure_existing_permissions;

mod stale_owned_partial_is_recovered_only_after_grace;

mod multipart_restart_reconciliation_is_state_driven_and_retryable;

mod put_delete_and_fsync_faults_recover_without_torn_objects;

mod multipart_commit_and_abort_faults_reconcile_from_durable_intent;

mod recovery_preserves_and_rejects_unowned_symlink_evidence;

fn set_multipart_status(root: &Path, upload_id: &str, status: serde_json::Value) {
    let path = root.join("multipart").join(upload_id).join("manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest["status"] = status;
    fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

fn write_private(path: &Path, contents: &[u8]) {
    fs::write(path, contents).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

fn regular_files(root: &Path) -> Vec<PathBuf> {
    let mut output = Vec::new();
    let mut pending = vec![root.to_owned()];
    while let Some(path) = pending.pop() {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                pending.push(entry.path());
            } else {
                output.push(entry.path());
            }
        }
    }
    output
}

mod inspect_local_authority_rejects_prefix_and_schema_drift;
