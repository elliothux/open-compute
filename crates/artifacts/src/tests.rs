use crate::cache::test_hooks::install_hash_pause;
use crate::error::is_not_found;
use crate::mock_s3::{Fault, MockS3};
use crate::{
    ARTIFACT_KEY_VERSION, ArtifactCache, ArtifactRef, ArtifactStore, MapEnv, ObjectBackend,
    SnapshotObjectStore, preflight_object_storage, resolve_s3_credentials,
    resolve_s3_credentials_with,
};
use bytes::Bytes;
use futures::stream;
use open_compute_core::{CacheConfig, ErrorCode, PlatformId, S3Config, StartupId};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{Error as IoError, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, SystemTime};
use tempfile::TempDir;

fn s3_config(endpoint: &str) -> S3Config {
    S3Config {
        endpoint: endpoint.to_owned(),
        region: "us-east-1".to_owned(),
        max_retries: 1,
        retry_backoff_ms: 10,
        connect_timeout_ms: 500,
        request_timeout_ms: 1_500,
        ..S3Config::default()
    }
}

fn cache_config(max_bytes: u64) -> CacheConfig {
    CacheConfig {
        max_bytes,
        high_watermark_ratio: 0.90,
        low_watermark_ratio: 0.50,
        partial_grace_ms: 50,
        max_artifact_bytes: max_bytes.max(64 * 1024),
    }
}

fn env() -> MapEnv {
    MapEnv::new()
        .with("S3_ACCESS_KEY_ID", "AKIAEXAMPLEKEYID01")
        .with(
            "S3_SECRET_ACCESS_KEY",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
        )
}

async fn client_for(mock: &MockS3) -> ObjectBackend {
    let config = s3_config(&mock.endpoint);
    let creds = resolve_s3_credentials_with(&config, &env()).expect("creds");
    ObjectBackend::connect_s3(&config, &creds, 64 * 1024).expect("client")
}

mod p1_snapshot_layout_commits_manifest_last_and_verifies_exact_bytes;

mod p1_snapshot_layout_rejects_malformed_bounds_and_remote_corruption;

async fn p1_snapshot_layout_invalid_gate() {
    let mock = MockS3::spawn("open-compute").await;
    let client = client_for(&mock).await;
    let platform = PlatformId::generate();
    let store = SnapshotObjectStore::new(client.clone(), platform);
    let snapshot_id = uuid::Uuid::now_v7().hyphenated().to_string();
    let key = format!("{}000000.bin", store.object_prefix(&snapshot_id).unwrap());
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("source.bin");
    write_mode(&source, "snapshot-bytes", 0o600);
    let payload = fs::read(&source).unwrap();
    let digest = hex::encode(Sha256::digest(&payload));

    for invalid_id in [
        "not-a-uuid".to_owned(),
        uuid::Uuid::nil().hyphenated().to_string(),
        snapshot_id.to_ascii_uppercase(),
    ] {
        assert!(store.object_prefix(&invalid_id).is_err());
        assert!(store.manifest_key(&invalid_id).is_err());
        assert!(
            SnapshotObjectStore::discover(client.clone(), &invalid_id)
                .await
                .is_err()
        );
    }
    assert!(
        SnapshotObjectStore::discover(client.clone(), &snapshot_id)
            .await
            .is_err(),
        "an uncommitted snapshot must not be discoverable"
    );

    assert!(
        store
            .put_file("system/outside.bin", &source, &digest, payload.len() as u64)
            .await
            .is_err()
    );
    assert!(
        store
            .put_file(
                &key,
                &source,
                &digest.to_ascii_uppercase(),
                payload.len() as u64
            )
            .await
            .is_err()
    );
    assert!(
        store
            .put_file(&key, &source, &digest, 64 * 1024 + 1)
            .await
            .is_err()
    );
    assert!(
        store
            .put_file(&key, &source, &digest, payload.len() as u64 + 1)
            .await
            .is_err()
    );
    assert!(
        store
            .put_file(&key, &source, &"0".repeat(64), payload.len() as u64)
            .await
            .is_err()
    );
    mock.set_fault(Fault::ServerError);
    assert!(
        store
            .put_file(&key, &source, &digest, payload.len() as u64)
            .await
            .is_err()
    );
    mock.set_fault(Fault::None);
    store
        .put_file(&key, &source, &digest, payload.len() as u64)
        .await
        .unwrap();

    assert!(store.put_manifest(&snapshot_id, b"", 1024).await.is_err());
    assert!(
        store
            .put_manifest(&snapshot_id, b"too-large", 1)
            .await
            .is_err()
    );
    let empty_manifest_id = uuid::Uuid::now_v7().hyphenated().to_string();
    mock.put_raw(&store.manifest_key(&empty_manifest_id).unwrap(), Vec::new());
    assert!(store.get_manifest(&empty_manifest_id, 1024).await.is_err());

    assert!(
        store
            .download_file(
                &key,
                &temp.path().join("too-large.bin"),
                &digest,
                64 * 1024 + 1,
            )
            .await
            .is_err()
    );
    assert!(
        store
            .download_file(
                &key,
                &temp.path().join("wrong-metadata.bin"),
                &"0".repeat(64),
                payload.len() as u64,
            )
            .await
            .is_err()
    );
    let occupied = temp.path().join("occupied.bin");
    write_mode(&occupied, "occupied", 0o600);
    assert!(
        store
            .download_file(&key, &occupied, &digest, payload.len() as u64)
            .await
            .is_err()
    );
    assert!(
        store
            .verify_file(&key, &digest, payload.len() as u64 + 1)
            .await
            .is_err()
    );

    let external_key = "system/external/reference.bin";
    mock.put_raw(external_key, payload.clone());
    assert!(
        store
            .verify_external_reference("outside/reference.bin", &digest, payload.len() as u64)
            .await
            .is_err()
    );
    assert!(
        store
            .verify_external_reference(external_key, "BAD", payload.len() as u64)
            .await
            .is_err()
    );
    assert!(
        store
            .verify_external_reference(external_key, &digest, payload.len() as u64 + 1)
            .await
            .is_err()
    );
    mock.corrupt_body(external_key);
    assert!(
        store
            .verify_external_reference(external_key, &digest, payload.len() as u64)
            .await
            .is_err()
    );
    assert!(
        store
            .delete_exact("system/not-snapshot-owned")
            .await
            .is_err()
    );

    let incomplete_id = uuid::Uuid::now_v7().hyphenated().to_string();
    let base = format!("system/snapshots/v1/{platform}/{incomplete_id}");
    mock.put_raw(&format!("{base}/unexpected"), b"retained".to_vec());
    mock.put_raw(
        &format!("system/snapshots/v1/{platform}/not-a-uuid/object"),
        b"retained".to_vec(),
    );
    let cleanup = store
        .cleanup_incomplete(SystemTime::now() + Duration::from_secs(60))
        .await
        .unwrap();
    assert_eq!(cleanup.prefixes, 1);
    assert_eq!(cleanup.objects, 1);
    assert_eq!(cleanup.bytes, payload.len() as u64);
    assert!(mock.keys().contains(&format!("{base}/unexpected")));
}

fn write_mode(path: &Path, contents: &str, mode: u32) {
    let mut f = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .open(path)
        .unwrap();
    f.write_all(contents.as_bytes()).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

fn cache_entry_path(root: &Path, digest: &str) -> PathBuf {
    root.join("sha256").join(&digest[..2]).join(&digest[2..])
}

fn list_partials(root: &Path) -> Vec<PathBuf> {
    let sha = root.join("sha256");
    let mut out = Vec::new();
    let Ok(shards) = fs::read_dir(sha) else {
        return out;
    };
    for shard in shards.flatten() {
        let Ok(ents) = fs::read_dir(shard.path()) else {
            continue;
        };
        for ent in ents.flatten() {
            let name = ent.file_name();
            if name.to_string_lossy().starts_with(".partial.") {
                out.push(ent.path());
            }
        }
    }
    out
}

mod credential_env_file_both_missing_mismatch_symlink_permissions_redaction;

mod credential_sources_reject_malformed_files_and_expose_only_explicitly;

mod production_client_rejects_insecure_or_zero_limit_configuration;

mod inspect_existing_cache_is_read_only_and_reports_integrity;

mod cached_acquire_rejects_directory_and_size_mismatch_entries;

mod eviction_stops_at_low_watermark_with_lru_entries_remaining;

mod preflight_records_signed_http_and_skips_head_bucket;

async fn expect_preflight_fail(fault: Fault) {
    let mock = MockS3::spawn("open-compute").await;
    mock.set_fault(fault);
    let client = client_for(&mock).await;
    let err = preflight_object_storage(&client, PlatformId::generate(), StartupId::generate())
        .await
        .unwrap_err();
    if fault == Fault::CorruptMetadata {
        assert_eq!(err.code(), ErrorCode::ArtifactIntegrityError);
    } else if fault == Fault::CorruptBody {
        assert_eq!(err.code(), ErrorCode::ObjectStorageIntegrityError);
    } else {
        assert_eq!(err.code(), ErrorCode::ObjectStorageUnavailable);
    }
    assert!(!format!("{err:?}").contains("AKIA"));
    assert!(!format!("{err:?}").contains("Authorization"));
    assert!(!err.message().contains("system/preflight"));
    if fault != Fault::DeleteFail {
        let expected = usize::from(matches!(fault, Fault::CorruptMetadata | Fault::CorruptBody));
        assert_eq!(mock.object_count(), expected);
    }
}

mod preflight_fails_closed_on_each_stage_and_cleans_up;

mod preflight_timeout_is_secret_safe;

mod streaming_put_head_open_and_same_digest_concurrency;

mod verified_file_upload_streams_and_rejects_post_parse_tamper;

mod remote_corruption_and_orphan_gc;

mod version_commit_reservation_fences_artifact_gc;

mod cache_same_size_corrupt_refetches_once;

mod cache_corrupt_local_and_s3_fails_once_and_cleans;

mod cache_symlink_rejected_then_valid_refetch;

mod concurrent_cold_miss_single_get;

mod verified_hit_with_s3_unavailable;

mod live_pin_blocks_eviction_until_sync_drop;

mod cancel_chunked_download_leaves_no_files;

mod failed_remove_does_not_lie_about_bytes;

mod cache_partial_cleanup_on_open;

mod artifact_store_rejects_stream_file_download_and_remote_failures;

mod artifact_store_integrity_and_existing_file_paths;

mod kv_backup_objects_are_host_scoped_immutable_and_verified;

mod d1_backup_objects_are_product_scoped_and_verified;

mod concurrent_put_precondition_races_verify_the_existing_winner;

mod artifact_listing_skips_invalid_keys_and_gc_respects_missing_time;

mod disk_write_error_rejects_file_root;

mod pin_covers_first_use_hash_against_eviction;

mod cache_open_rejects_symlink_root;

mod cache_open_rejects_symlink_ancestor_without_mutating_outside;

mod cache_open_rejects_relative_symlink_ancestor_without_mutating_outside;
