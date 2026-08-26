use crate::cache::test_hooks::install_hash_pause;
use crate::error::{
    OpKind, classify_connector, classify_http_status, classify_service_code, integrity_error,
    is_not_found, unavailable,
};
use crate::mock_s3::{Fault, MockS3};
use crate::{
    ARTIFACT_KEY_VERSION, ArtifactCache, ArtifactRef, ArtifactStore, MapEnv, S3ArtifactClient,
    S3Failure, S3Stage, SnapshotObjectStore, StaticEnv, preflight_s3, resolve_s3_credentials,
    resolve_s3_credentials_with,
};
use aws_smithy_runtime_api::client::result::ConnectorError;
use bytes::Bytes;
use futures::stream;
use open_compute_core::{CacheConfig, ErrorCode, PlatformConfig, PlatformId, S3Config, StartupId};
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
    let toml = format!(
        r#"
[s3]
endpoint = "{endpoint}"
region = "us-east-1"
bucket = "open-compute"
force_path_style = true
access_key_id_env = "S3_ACCESS_KEY_ID"
secret_access_key_env = "S3_SECRET_ACCESS_KEY"
prefix = "system/"
max_retries = 1
retry_backoff_ms = 10
connect_timeout_ms = 500
request_timeout_ms = 1500
"#
    );
    PlatformConfig::from_toml_str(&toml).expect("config").s3
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

async fn client_for(mock: &MockS3) -> S3ArtifactClient {
    let config = s3_config(&mock.endpoint);
    let creds = resolve_s3_credentials_with(&config, &env()).expect("creds");
    S3ArtifactClient::connect(&config, &creds, 64 * 1024).expect("client")
}

#[tokio::test]
async fn p1_snapshot_layout_commits_manifest_last_and_verifies_exact_bytes() {
    let mock = MockS3::spawn("open-compute").await;
    let client = client_for(&mock).await;
    let platform = PlatformId::generate();
    let store = SnapshotObjectStore::new(client.clone(), platform);
    let snapshot_id = uuid::Uuid::now_v7().hyphenated().to_string();
    let prefix = store.object_prefix(&snapshot_id).unwrap();
    let key = format!("{prefix}000000.bin");
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("source.bin");
    write_mode(&source, "snapshot-bytes", 0o600);
    let payload = fs::read(&source).unwrap();
    let digest = hex::encode(Sha256::digest(&payload));
    store
        .put_file(&key, &source, &digest, payload.len() as u64)
        .await
        .unwrap();
    assert!(store.list_committed().await.unwrap().is_empty());
    store
        .verify_file(&key, &digest, payload.len() as u64)
        .await
        .unwrap();
    let restored = temp.path().join("restored.bin");
    store
        .download_file(&key, &restored, &digest, payload.len() as u64)
        .await
        .unwrap();
    assert_eq!(fs::read(restored).unwrap(), payload);

    let manifest = br#"{"schema_version":1}"#;
    let manifest_key = store
        .put_manifest(&snapshot_id, manifest, 1024)
        .await
        .unwrap();
    assert_eq!(manifest_key, store.manifest_key(&snapshot_id).unwrap());
    assert_eq!(
        store.get_manifest(&snapshot_id, 1024).await.unwrap(),
        manifest
    );
    assert_eq!(store.list_committed().await.unwrap().len(), 1);
    let discovered = SnapshotObjectStore::discover(client, &snapshot_id)
        .await
        .unwrap();
    assert_eq!(
        discovered.get_manifest(&snapshot_id, 1024).await.unwrap(),
        manifest
    );
    assert!(
        store
            .put_manifest(&snapshot_id, b"different", 1024)
            .await
            .is_err()
    );
    let incomplete_id = uuid::Uuid::now_v7().hyphenated().to_string();
    let incomplete_key = format!("{}000000.bin", store.object_prefix(&incomplete_id).unwrap());
    store
        .put_file(&incomplete_key, &source, &digest, payload.len() as u64)
        .await
        .unwrap();
    let cleanup = store
        .cleanup_incomplete(SystemTime::now() + Duration::from_secs(60))
        .await
        .unwrap();
    assert_eq!(cleanup.prefixes, 1);
    assert_eq!(cleanup.objects, 1);
    assert_eq!(cleanup.bytes, payload.len() as u64);
    assert!(
        store
            .verify_file(&incomplete_key, &digest, payload.len() as u64)
            .await
            .is_err()
    );
    assert_eq!(store.list_committed().await.unwrap().len(), 1);
    mock.set_fault(Fault::CorruptBody);
    assert!(
        store
            .verify_file(&key, &digest, payload.len() as u64)
            .await
            .is_err()
    );
}

#[test]
fn p1_snapshot_layout_rejects_malformed_bounds_and_remote_corruption() {
    std::thread::Builder::new()
        .name("p1-snapshot-invalid".to_owned())
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(p1_snapshot_layout_invalid_gate());
        })
        .unwrap()
        .join()
        .unwrap();
}

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

#[test]
fn credential_env_file_both_missing_mismatch_symlink_permissions_redaction() {
    let dir = TempDir::new().unwrap();
    let access = dir.path().join("access");
    let secret = dir.path().join("secret");
    write_mode(&access, "AKIAEXAMPLEKEYID01\n", 0o600);
    write_mode(&secret, "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY\n", 0o600);

    let mut cfg = s3_config("http://127.0.0.1:9");
    cfg.access_key_id_file = Some(access.clone());
    cfg.secret_access_key_file = Some(secret.clone());

    let creds = resolve_s3_credentials_with(&cfg, &env()).expect("both match");
    let debug = format!("{creds:?}");
    let display = creds.to_string();
    assert!(!debug.contains("AKIA"));
    assert!(!display.contains("AKIA"));
    assert!(!debug.contains("wJalr"));

    let mismatch = env().with("S3_ACCESS_KEY_ID", "OTHER");
    assert_eq!(
        resolve_s3_credentials_with(&cfg, &mismatch)
            .unwrap_err()
            .code(),
        ErrorCode::SecretRefInvalid
    );

    let empty = MapEnv::new();
    cfg.access_key_id_file = None;
    cfg.secret_access_key_file = None;
    assert_eq!(
        resolve_s3_credentials_with(&cfg, &empty)
            .unwrap_err()
            .code(),
        ErrorCode::SecretRefInvalid
    );

    write_mode(&access, "", 0o600);
    cfg.access_key_id_file = Some(access.clone());
    cfg.access_key_id_env = None;
    cfg.secret_access_key_file = Some(secret.clone());
    cfg.secret_access_key_env = None;
    assert_eq!(
        resolve_s3_credentials_with(&cfg, &empty)
            .unwrap_err()
            .code(),
        ErrorCode::SecretRefInvalid
    );

    write_mode(&access, "AKIAEXAMPLEKEYID01", 0o644);
    cfg.access_key_id_file = Some(access.clone());
    assert_eq!(
        resolve_s3_credentials_with(&cfg, &empty)
            .unwrap_err()
            .code(),
        ErrorCode::SecretRefInvalid
    );

    let target = dir.path().join("secret-target");
    write_mode(&target, "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY", 0o600);
    let before = fs::read(&target).unwrap();
    let link = dir.path().join("access-link");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    cfg.access_key_id_file = Some(link);
    write_mode(&access, "AKIAEXAMPLEKEYID01", 0o600);
    assert_eq!(
        resolve_s3_credentials_with(&cfg, &empty)
            .unwrap_err()
            .code(),
        ErrorCode::SecretRefInvalid
    );
    assert_eq!(fs::read(&target).unwrap(), before);

    let process_err = resolve_s3_credentials(&s3_config("http://127.0.0.1:9"));
    match process_err {
        Ok(c) => {
            assert!(!format!("{c:?}").contains("AKIA"));
            assert!(!c.to_string().contains("AKIA"));
        }
        Err(err) => assert_eq!(err.code(), ErrorCode::SecretRefInvalid),
    }
}

#[test]
fn credential_sources_reject_malformed_files_and_expose_only_explicitly() {
    let dir = TempDir::new().unwrap();
    let access = dir.path().join("access");
    let secret = dir.path().join("secret");
    write_mode(&access, "AKIAEXAMPLEKEYID01\r\n", 0o600);
    write_mode(&secret, "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY\n", 0o600);

    let mut cfg = s3_config("http://127.0.0.1:9");
    cfg.access_key_id_env = None;
    cfg.secret_access_key_env = None;
    cfg.access_key_id_file = Some(access.clone());
    cfg.secret_access_key_file = Some(secret.clone());
    let creds = resolve_s3_credentials_with(&cfg, &StaticEnv::new(MapEnv::new())).unwrap();
    assert_eq!(creds.access_key_id().expose(), "AKIAEXAMPLEKEYID01");
    assert_eq!(
        creds.secret_access_key().expose(),
        "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY"
    );
    assert_eq!(creds.to_string(), "S3Credentials([REDACTED])");

    cfg.access_key_id_file = Some(PathBuf::from("relative-access"));
    assert_eq!(
        resolve_s3_credentials_with(&cfg, &MapEnv::new())
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );

    cfg.access_key_id_file = Some(dir.path().to_path_buf());
    assert_eq!(
        resolve_s3_credentials_with(&cfg, &MapEnv::new())
            .unwrap_err()
            .code(),
        ErrorCode::SecretRefInvalid
    );

    cfg.access_key_id_file = Some(access.clone());
    let write_bytes = |bytes: &[u8]| {
        let mut file = OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&access)
            .unwrap();
        file.write_all(bytes).unwrap();
    };
    write_bytes(&vec![b'x'; 16 * 1024 + 1]);
    assert_eq!(
        resolve_s3_credentials_with(&cfg, &MapEnv::new())
            .unwrap_err()
            .code(),
        ErrorCode::SecretRefInvalid
    );
    write_bytes(&[0xff, 0xfe]);
    assert_eq!(
        resolve_s3_credentials_with(&cfg, &MapEnv::new())
            .unwrap_err()
            .code(),
        ErrorCode::SecretRefInvalid
    );
    write_bytes(b"valid-prefix\0hidden");
    assert_eq!(
        resolve_s3_credentials_with(&cfg, &MapEnv::new())
            .unwrap_err()
            .code(),
        ErrorCode::SecretRefInvalid
    );

    cfg.access_key_id_file = None;
    cfg.secret_access_key_file = None;
    cfg.access_key_id_env = Some("ACCESS".to_string());
    cfg.secret_access_key_env = Some("SECRET".to_string());
    let static_env = StaticEnv::new(
        MapEnv::new()
            .with("ACCESS", "env-access")
            .with("SECRET", "env-secret"),
    );
    let creds = resolve_s3_credentials_with(&cfg, &static_env).unwrap();
    assert_eq!(creds.access_key_id().expose(), "env-access");
    assert_eq!(creds.secret_access_key().expose(), "env-secret");

    let mut empty_secret = cfg.clone();
    empty_secret.secret_access_key_env = Some("EMPTY_SECRET".into());
    assert_eq!(
        resolve_s3_credentials_with(
            &empty_secret,
            &MapEnv::new()
                .with("ACCESS", "env-access")
                .with("EMPTY_SECRET", ""),
        )
        .unwrap_err()
        .code(),
        ErrorCode::SecretRefInvalid
    );
    empty_secret.secret_access_key_env = None;
    empty_secret.secret_access_key_file = None;
    assert_eq!(
        resolve_s3_credentials_with(&empty_secret, &MapEnv::new().with("ACCESS", "env-access"))
            .unwrap_err()
            .code(),
        ErrorCode::SecretRefInvalid
    );
}

#[test]
fn production_client_rejects_insecure_or_zero_limit_configuration() {
    let creds = resolve_s3_credentials_with(&s3_config("https://s3.example.invalid"), &env())
        .expect("credentials");
    let mut insecure = s3_config("https://s3.example.invalid");
    insecure.verify_tls = false;
    assert_eq!(
        S3ArtifactClient::connect(&insecure, &creds, 1024)
            .unwrap_err()
            .code(),
        ErrorCode::ConfigInvalid
    );
    let secure = s3_config("https://s3.example.invalid");
    assert_eq!(
        S3ArtifactClient::connect(&secure, &creds, 0)
            .unwrap_err()
            .code(),
        ErrorCode::LimitInvalid
    );
}

#[test]
fn inspect_existing_cache_is_read_only_and_reports_integrity() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("cache");
    assert!(ArtifactCache::inspect_existing(root.clone()).is_err());
    fs::write(&root, b"not a directory").unwrap();
    assert!(ArtifactCache::inspect_existing(root.clone()).is_err());
    fs::remove_file(&root).unwrap();
    fs::create_dir(&root).unwrap();

    let empty = ArtifactCache::inspect_existing(root.clone()).unwrap();
    assert_eq!(empty.entry_count(), 0);
    let empty_sample = crate::sample_cache_integrity(&empty).unwrap();
    assert_eq!(empty_sample.entries, 0);
    assert_eq!(empty_sample.bytes, 0);
    assert!(!empty_sample.corrupt);
    assert!(format!("{empty_sample:?}").contains("entries"));

    let sha_root = root.join("sha256");
    fs::write(&sha_root, b"not a directory").unwrap();
    assert!(ArtifactCache::inspect_existing(root.clone()).is_err());
    fs::remove_file(&sha_root).unwrap();
    let outside = temp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, &sha_root).unwrap();
    assert!(ArtifactCache::inspect_existing(root.clone()).is_err());
    fs::remove_file(&sha_root).unwrap();

    fs::create_dir(&sha_root).unwrap();
    fs::write(sha_root.join("regular-shard"), b"ignored").unwrap();
    fs::create_dir(sha_root.join("x")).unwrap();
    let digest = hex::encode(Sha256::digest(b"cached"));
    let shard = sha_root.join(&digest[..2]);
    fs::create_dir(&shard).unwrap();
    fs::write(shard.join(&digest[2..]), b"cached").unwrap();
    fs::write(shard.join("short"), b"ignored").unwrap();
    let cache = ArtifactCache::inspect_existing(root.clone()).unwrap();
    assert_eq!(cache.entry_count(), 1);
    let sample = crate::sample_cache_integrity(&cache).unwrap();
    assert_eq!(sample.entries, 1);
    assert_eq!(sample.bytes, 6);
    assert!(!sample.corrupt);

    fs::write(shard.join(&digest[2..]), b"broken").unwrap();
    assert!(crate::sample_cache_integrity(&cache).unwrap().corrupt);
    fs::remove_file(shard.join(&digest[2..])).unwrap();
    assert!(crate::sample_cache_integrity(&cache).unwrap().corrupt);
}

#[tokio::test]
async fn cached_acquire_rejects_directory_and_size_mismatch_entries() {
    let temp = TempDir::new().unwrap();
    let cache = ArtifactCache::open(
        temp.path().join("cache"),
        cache_config(1024),
        StartupId::generate(),
    )
    .unwrap();
    let digest = "ab".repeat(32);
    let artifact = ArtifactRef::new(1, &digest, 1).unwrap();
    let path = cache_entry_path(&temp.path().join("cache"), &digest);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::create_dir(&path).unwrap();
    assert_eq!(
        cache.acquire_cached(&artifact).await.unwrap_err().code(),
        ErrorCode::CacheEntryCorrupt
    );
    fs::remove_dir(&path).unwrap();
    fs::write(&path, b"too long").unwrap();
    assert_eq!(
        cache.acquire_cached(&artifact).await.unwrap_err().code(),
        ErrorCode::CacheEntryCorrupt
    );
}

#[tokio::test]
async fn eviction_stops_at_low_watermark_with_lru_entries_remaining() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("cache");
    for byte in *b"abcd" {
        let body = vec![byte; 3];
        let digest = hex::encode(Sha256::digest(&body));
        let path = cache_entry_path(&root, &digest);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }
    let cache = ArtifactCache::open(
        root,
        CacheConfig {
            max_bytes: 10,
            high_watermark_ratio: 0.9,
            low_watermark_ratio: 0.5,
            partial_grace_ms: 0,
            max_artifact_bytes: 1024,
        },
        StartupId::generate(),
    )
    .unwrap();
    assert_eq!(cache.entry_count(), 4);
    cache.evict_if_needed().await.unwrap();
    assert_eq!(cache.entry_count(), 1);
}

#[tokio::test]
async fn preflight_records_signed_http_and_skips_head_bucket() {
    let mock = MockS3::spawn("open-compute").await;
    let client = client_for(&mock).await;
    let out = preflight_s3(&client, PlatformId::generate(), StartupId::generate())
        .await
        .expect("preflight");
    assert_eq!(out.payload_bytes(), 32);
    assert_eq!(out.puts(), 1);
    assert_eq!(out.heads(), 2);
    assert_eq!(out.gets(), 1);
    assert_eq!(out.deletes(), 1);
    assert!(format!("{out:?}").contains("payload_bytes"));
    let canary = crate::PreflightOutcome::successful_canary();
    assert_eq!(canary, out);
    let rec = mock.recorded();
    assert!(rec.iter().all(|r| r.method != "HEAD"
        || r.path.contains("/preflight/")
        || r.path.contains("/artifacts/")));
    assert!(
        !rec.iter().any(
            |r| r.method == "HEAD" && (r.path == "/open-compute" || r.path == "/open-compute/")
        )
    );
    let payload_ops: Vec<_> = rec
        .iter()
        .filter(|r| matches!(r.method.as_str(), "PUT" | "HEAD" | "GET" | "DELETE"))
        .collect();
    assert!(payload_ops.len() >= 5);
    assert!(payload_ops.iter().any(|r| r.method == "PUT"));
    assert!(payload_ops.iter().any(|r| r.method == "GET"));
    assert!(payload_ops.iter().all(|r| r.has_authorization));
    assert!(payload_ops.iter().all(|r| {
        r.authorization
            .as_deref()
            .is_some_and(|v| v.starts_with("AWS4-HMAC-SHA256 Credential="))
    }));
    assert_eq!(payload_ops[0].method, "PUT");
    assert_eq!(mock.object_count(), 0);
}

#[tokio::test]
async fn connectivity_probe_accepts_authenticated_not_found_only() {
    let mock = MockS3::spawn("open-compute").await;
    let client = client_for(&mock).await;
    client.probe_connectivity().await.unwrap();
    mock.put_raw(
        "system/__open_compute_connectivity_probe",
        b"reserved".to_vec(),
    );
    client.probe_connectivity().await.unwrap();
    mock.set_fault(Fault::Permission);
    assert_eq!(
        client.probe_connectivity().await.unwrap_err().code(),
        ErrorCode::S3Unavailable
    );
}

async fn expect_preflight_fail(fault: Fault) {
    let mock = MockS3::spawn("open-compute").await;
    mock.set_fault(fault);
    let client = client_for(&mock).await;
    let err = preflight_s3(&client, PlatformId::generate(), StartupId::generate())
        .await
        .unwrap_err();
    if matches!(fault, Fault::CorruptMetadata | Fault::CorruptBody) {
        assert_eq!(err.code(), ErrorCode::ArtifactIntegrityError);
    } else {
        assert_eq!(err.code(), ErrorCode::S3Unavailable);
    }
    assert!(!format!("{err:?}").contains("AKIA"));
    assert!(!format!("{err:?}").contains("Authorization"));
    assert!(!err.message().contains("system/preflight"));
    if fault != Fault::DeleteFail {
        assert_eq!(mock.object_count(), 0);
    }
}

#[tokio::test]
async fn preflight_fails_closed_on_each_stage_and_cleans_up() {
    expect_preflight_fail(Fault::Auth).await;
    expect_preflight_fail(Fault::Permission).await;
    expect_preflight_fail(Fault::ServerError).await;
    expect_preflight_fail(Fault::NotFound).await;
    expect_preflight_fail(Fault::DeleteFail).await;
    expect_preflight_fail(Fault::CorruptMetadata).await;
    expect_preflight_fail(Fault::CorruptBody).await;
}

#[tokio::test]
async fn preflight_timeout_is_secret_safe() {
    let mock = MockS3::spawn("open-compute").await;
    mock.set_fault(Fault::Timeout);
    let mut cfg = s3_config(&mock.endpoint);
    cfg.request_timeout_ms = 200;
    cfg.connect_timeout_ms = 100;
    cfg.max_retries = 1;
    let creds = resolve_s3_credentials_with(&cfg, &env()).unwrap();
    let client = S3ArtifactClient::connect(&cfg, &creds, 1024).unwrap();
    let err = preflight_s3(&client, PlatformId::generate(), StartupId::generate())
        .await
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::S3Unavailable);
    let json = serde_json::to_string(&err).unwrap();
    assert!(!json.contains("AKIA"));
    assert!(!json.contains("AWS4"));
}

#[tokio::test]
async fn streaming_put_head_open_and_same_digest_concurrency() {
    let mock = MockS3::spawn("open-compute").await;
    let store = ArtifactStore::new(client_for(&mock).await);
    let payload = Bytes::from_static(b"hello-artifact");
    let digest = hex::encode(Sha256::digest(&payload));
    let stream = stream::iter(vec![Ok::<Bytes, std::io::Error>(payload.clone())]);
    let r1 = store
        .put_verified(stream, &digest, payload.len() as u64)
        .await
        .unwrap();
    assert_eq!(r1.sha256_hex(), digest);
    let h = store.head(&r1).await.unwrap();
    assert_eq!(h.size(), payload.len() as u64);
    let body = store.open(&r1).await.unwrap();
    assert_eq!(body.as_ref(), payload.as_ref());

    let store2 = store.clone();
    let store3 = store.clone();
    let p1 = payload.clone();
    let p2 = payload.clone();
    let d1 = digest.clone();
    let d2 = digest.clone();
    let a = tokio::spawn(async move {
        store2
            .put_verified(stream::iter(vec![Ok::<Bytes, std::io::Error>(p1)]), &d1, 14)
            .await
    });
    let b = tokio::spawn(async move {
        store3
            .put_verified(stream::iter(vec![Ok::<Bytes, std::io::Error>(p2)]), &d2, 14)
            .await
    });
    let ra = a.await.unwrap().unwrap();
    let rb = b.await.unwrap().unwrap();
    assert_eq!(ra, rb);

    let too_big = store
        .put_verified(
            stream::iter(vec![Ok::<Bytes, std::io::Error>(Bytes::from(vec![1; 8]))]),
            &digest,
            8,
        )
        .await
        .unwrap_err();
    assert_eq!(too_big.code(), ErrorCode::ArtifactIntegrityError);
    let over = store
        .put_verified(
            stream::iter(vec![Ok::<Bytes, std::io::Error>(Bytes::from(vec![1; 16]))]),
            &"ab".repeat(32),
            1024 * 1024,
        )
        .await
        .unwrap_err();
    assert_eq!(over.code(), ErrorCode::LimitInvalid);

    let mismatch = store
        .put_verified(
            stream::iter(vec![Ok::<Bytes, std::io::Error>(Bytes::from_static(
                b"nope",
            ))]),
            &digest,
            4,
        )
        .await
        .unwrap_err();
    assert_eq!(mismatch.code(), ErrorCode::ArtifactIntegrityError);
}

#[tokio::test]
async fn verified_file_upload_streams_and_rejects_post_parse_tamper() {
    let mock = MockS3::spawn("open-compute").await;
    let store = ArtifactStore::new(client_for(&mock).await);
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("staged");
    let payload = vec![b'x'; 48 * 1024];
    fs::write(&path, &payload).unwrap();
    let digest = hex::encode(Sha256::digest(&payload));
    let artifact = store
        .put_verified_file(&path, &digest, payload.len() as u64)
        .await
        .unwrap();
    assert_eq!(store.open(&artifact).await.unwrap().as_ref(), payload);

    fs::write(&path, vec![b'y'; 48 * 1024]).unwrap();
    let error = store
        .put_verified_file(&path, &digest, payload.len() as u64)
        .await
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::ArtifactIntegrityError);
}

#[tokio::test]
async fn remote_corruption_and_orphan_gc() {
    let mock = MockS3::spawn("open-compute").await;
    let store = ArtifactStore::new(client_for(&mock).await);
    let payload = Bytes::from_static(b"gc-me");
    let digest = hex::encode(Sha256::digest(&payload));
    let r = store
        .put_verified(
            stream::iter(vec![Ok::<Bytes, std::io::Error>(payload.clone())]),
            &digest,
            5,
        )
        .await
        .unwrap();
    mock.corrupt_body(&r.physical_key("system/"));
    let err = store.open(&r).await.unwrap_err();
    assert_eq!(err.code(), ErrorCode::ArtifactIntegrityError);

    mock.put_raw("tenant/not-ours", b"x".to_vec());
    let candidates = store.list_candidates().await.unwrap();
    assert!(
        candidates
            .iter()
            .all(|c| c.artifact.sha256_hex().len() == 64)
    );
    let referenced = HashSet::new();
    let deleted = store
        .gc_unreferenced(&referenced, SystemTime::now() + Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(deleted, 1);
    assert_eq!(mock.object_count(), 1);

    let mock2 = MockS3::spawn("open-compute").await;
    let store2 = ArtifactStore::new(client_for(&mock2).await);
    let payload2 = Bytes::from_static(b"keep-me");
    let digest2 = hex::encode(Sha256::digest(&payload2));
    store2
        .put_verified(
            stream::iter(vec![Ok::<Bytes, std::io::Error>(payload2.clone())]),
            &digest2,
            7,
        )
        .await
        .unwrap();
    mock2.set_omit_last_modified(true);
    let deleted2 = store2
        .gc_unreferenced(&HashSet::new(), SystemTime::now() + Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(deleted2, 0);
    assert_eq!(mock2.object_count(), 1);
}

#[tokio::test]
async fn cache_same_size_corrupt_refetches_once() {
    let mock = MockS3::spawn("open-compute").await;
    let store = ArtifactStore::new(client_for(&mock).await);
    let payload = Bytes::from_static(b"correct-bytes!!");
    let digest = hex::encode(Sha256::digest(&payload));
    let r = store
        .put_verified(
            stream::iter(vec![Ok::<Bytes, std::io::Error>(payload.clone())]),
            &digest,
            payload.len() as u64,
        )
        .await
        .unwrap();
    let tmp = TempDir::new().unwrap();
    let cache = ArtifactCache::open(
        tmp.path().to_path_buf(),
        cache_config(4096),
        StartupId::generate(),
    )
    .unwrap();
    cache.acquire(&store, &r).await.unwrap();
    let dest = cache_entry_path(tmp.path(), &digest);
    let corrupt = vec![b'X'; payload.len()];
    fs::write(&dest, &corrupt).unwrap();
    let cache = ArtifactCache::open(
        tmp.path().to_path_buf(),
        cache_config(4096),
        StartupId::generate(),
    )
    .unwrap();
    let gets_before = mock.artifact_gets();
    let mut pin = cache.acquire(&store, &r).await.unwrap();
    assert!(pin.file().metadata().unwrap().is_file());
    assert_eq!(pin.read_all().unwrap(), payload.as_ref());
    assert_eq!(mock.artifact_gets(), gets_before + 1);
    assert_eq!(fs::read(&dest).unwrap(), payload.as_ref());
}

#[tokio::test]
async fn cache_corrupt_local_and_s3_fails_once_and_cleans() {
    let mock = MockS3::spawn("open-compute").await;
    let store = ArtifactStore::new(client_for(&mock).await);
    let payload = Bytes::from_static(b"will-be-wrong!");
    let digest = hex::encode(Sha256::digest(&payload));
    let r = store
        .put_verified(
            stream::iter(vec![Ok::<Bytes, std::io::Error>(payload.clone())]),
            &digest,
            payload.len() as u64,
        )
        .await
        .unwrap();
    let tmp = TempDir::new().unwrap();
    let cache = ArtifactCache::open(
        tmp.path().to_path_buf(),
        cache_config(4096),
        StartupId::generate(),
    )
    .unwrap();
    cache.acquire(&store, &r).await.unwrap();
    let dest = cache_entry_path(tmp.path(), &digest);
    fs::write(&dest, vec![b'Y'; payload.len()]).unwrap();
    let cache = ArtifactCache::open(
        tmp.path().to_path_buf(),
        cache_config(4096),
        StartupId::generate(),
    )
    .unwrap();
    mock.set_fault(Fault::CorruptBody);
    let gets_before = mock.artifact_gets();
    let err = cache.acquire(&store, &r).await.unwrap_err();
    assert_eq!(err.code(), ErrorCode::ArtifactIntegrityError);
    assert_eq!(mock.artifact_gets(), gets_before + 1);
    assert!(!dest.exists());
    assert!(list_partials(tmp.path()).is_empty());
}

#[tokio::test]
async fn cache_symlink_rejected_then_valid_refetch() {
    let mock = MockS3::spawn("open-compute").await;
    let store = ArtifactStore::new(client_for(&mock).await);
    let payload = Bytes::from_static(b"symlink-target-ok");
    let digest = hex::encode(Sha256::digest(&payload));
    let r = store
        .put_verified(
            stream::iter(vec![Ok::<Bytes, std::io::Error>(payload.clone())]),
            &digest,
            payload.len() as u64,
        )
        .await
        .unwrap();
    let tmp = TempDir::new().unwrap();
    let outside = tmp.path().join("outside-target");
    write_mode(&outside, "do-not-touch", 0o600);
    let dest = cache_entry_path(tmp.path(), &digest);
    fs::create_dir_all(dest.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&outside, &dest).unwrap();
    let cache = ArtifactCache::open(
        tmp.path().to_path_buf(),
        cache_config(4096),
        StartupId::generate(),
    )
    .unwrap();
    let mut pin = cache.acquire(&store, &r).await.unwrap();
    assert_eq!(pin.read_all().unwrap(), payload.as_ref());
    assert_eq!(fs::read(&outside).unwrap(), b"do-not-touch");
    assert_eq!(fs::read(&dest).unwrap(), payload.as_ref());
}

#[tokio::test]
async fn concurrent_cold_miss_single_get() {
    let mock = MockS3::spawn("open-compute").await;
    let store = ArtifactStore::new(client_for(&mock).await);
    let payload = Bytes::from_static(b"singleflight-body");
    let digest = hex::encode(Sha256::digest(&payload));
    let r = store
        .put_verified(
            stream::iter(vec![Ok::<Bytes, std::io::Error>(payload.clone())]),
            &digest,
            payload.len() as u64,
        )
        .await
        .unwrap();
    let tmp = TempDir::new().unwrap();
    let cache = Arc::new(
        ArtifactCache::open(
            tmp.path().to_path_buf(),
            cache_config(4096),
            StartupId::generate(),
        )
        .unwrap(),
    );
    let gets_before = mock.artifact_gets();
    let c1 = Arc::clone(&cache);
    let c2 = Arc::clone(&cache);
    let s1 = store.clone();
    let s2 = store.clone();
    let r1 = r.clone();
    let r2 = r.clone();
    let a = tokio::spawn(async move { c1.acquire(&s1, &r1).await });
    let b = tokio::spawn(async move { c2.acquire(&s2, &r2).await });
    let pa = a.await.unwrap().unwrap();
    let pb = b.await.unwrap().unwrap();
    drop(pa);
    drop(pb);
    assert_eq!(mock.artifact_gets(), gets_before + 1);
}

#[tokio::test]
async fn verified_hit_with_s3_unavailable() {
    let mock = MockS3::spawn("open-compute").await;
    let store = ArtifactStore::new(client_for(&mock).await);
    let payload = Bytes::from_static(b"cached-bytes");
    let digest = hex::encode(Sha256::digest(&payload));
    let r = store
        .put_verified(
            stream::iter(vec![Ok::<Bytes, std::io::Error>(payload.clone())]),
            &digest,
            payload.len() as u64,
        )
        .await
        .unwrap();
    let tmp = TempDir::new().unwrap();
    let cache = ArtifactCache::open(
        tmp.path().to_path_buf(),
        cache_config(4096),
        StartupId::generate(),
    )
    .unwrap();
    let mut pin = cache.acquire(&store, &r).await.unwrap();
    assert_eq!(pin.read_all().unwrap(), payload.as_ref());
    drop(pin);
    mock.set_fault(Fault::ServerError);
    let gets = mock.artifact_gets();
    let mut hit = cache.acquire(&store, &r).await.unwrap();
    assert_eq!(hit.read_all().unwrap(), payload.as_ref());
    assert_eq!(mock.artifact_gets(), gets);
    let mut cached = cache.acquire_cached(&r).await.unwrap();
    assert_eq!(cached.read_all().unwrap(), payload.as_ref());
    assert_eq!(mock.artifact_gets(), gets);
}

#[tokio::test]
async fn live_pin_blocks_eviction_until_sync_drop() {
    let mock = MockS3::spawn("open-compute").await;
    let store = ArtifactStore::new(client_for(&mock).await);
    let p1 = Bytes::from_static(b"aaaaaaaaaaaaaaaa");
    let p2 = Bytes::from_static(b"bbbbbbbbbbbbbbbb");
    let d1 = hex::encode(Sha256::digest(&p1));
    let d2 = hex::encode(Sha256::digest(&p2));
    let r1 = store
        .put_verified(
            stream::iter(vec![Ok::<Bytes, std::io::Error>(p1.clone())]),
            &d1,
            p1.len() as u64,
        )
        .await
        .unwrap();
    let r2 = store
        .put_verified(
            stream::iter(vec![Ok::<Bytes, std::io::Error>(p2.clone())]),
            &d2,
            p2.len() as u64,
        )
        .await
        .unwrap();
    let tmp = TempDir::new().unwrap();
    let cache = ArtifactCache::open(
        tmp.path().to_path_buf(),
        cache_config(16),
        StartupId::generate(),
    )
    .unwrap();
    let pin = cache.acquire(&store, &r1).await.unwrap();
    cache.acquire(&store, &r2).await.unwrap();
    cache.evict_if_needed().await.unwrap();
    assert!(cache_entry_path(tmp.path(), &d1).exists());
    drop(pin);
    cache.evict_if_needed().await.unwrap();
    assert!(!cache_entry_path(tmp.path(), &d1).exists());
}

#[tokio::test]
async fn cancel_chunked_download_leaves_no_files() {
    let mock = MockS3::spawn("open-compute").await;
    mock.set_get_chunking(1, Duration::from_millis(80));
    let store = ArtifactStore::new(client_for(&mock).await);
    let payload = Bytes::from_static(b"slow-download!!");
    let digest = hex::encode(Sha256::digest(&payload));
    let r = store
        .put_verified(
            stream::iter(vec![Ok::<Bytes, std::io::Error>(payload.clone())]),
            &digest,
            payload.len() as u64,
        )
        .await
        .unwrap();
    mock.set_get_chunking(1, Duration::from_millis(80));
    let tmp = TempDir::new().unwrap();
    let cache = ArtifactCache::open(
        tmp.path().to_path_buf(),
        cache_config(4096),
        StartupId::generate(),
    )
    .unwrap();
    let handle = tokio::spawn(async move { cache.acquire(&store, &r).await });
    tokio::time::sleep(Duration::from_millis(120)).await;
    handle.abort();
    let _ = handle.await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!cache_entry_path(tmp.path(), &digest).exists());
    assert!(list_partials(tmp.path()).is_empty());
}

#[tokio::test]
async fn failed_remove_does_not_lie_about_bytes() {
    let mock = MockS3::spawn("open-compute").await;
    let store = ArtifactStore::new(client_for(&mock).await);
    let payload = Bytes::from_static(b"keep-accounting");
    let digest = hex::encode(Sha256::digest(&payload));
    let r = store
        .put_verified(
            stream::iter(vec![Ok::<Bytes, std::io::Error>(payload.clone())]),
            &digest,
            payload.len() as u64,
        )
        .await
        .unwrap();
    let tmp = TempDir::new().unwrap();
    let cache = ArtifactCache::open(
        tmp.path().to_path_buf(),
        cache_config(payload.len() as u64),
        StartupId::generate(),
    )
    .unwrap();
    cache.acquire(&store, &r).await.unwrap();
    let before = cache.total_bytes().await;
    assert_eq!(before, payload.len() as u64);
    let shard = tmp.path().join("sha256").join(&digest[..2]);
    let orig = fs::metadata(&shard).unwrap().permissions();
    let mut ro = orig.clone();
    ro.set_mode(0o555);
    fs::set_permissions(&shard, ro).unwrap();
    cache.evict_if_needed().await.unwrap();
    assert_eq!(cache.total_bytes().await, before);
    assert!(cache_entry_path(tmp.path(), &digest).exists());
    fs::set_permissions(&shard, orig).unwrap();
}

#[tokio::test]
async fn cache_partial_cleanup_on_open() {
    let tmp = TempDir::new().unwrap();
    let shard = tmp.path().join("sha256").join("ab");
    fs::create_dir_all(&shard).unwrap();
    let stale = shard.join(format!(".partial.{}.dead", StartupId::generate()));
    write_mode(&stale, "partial", 0o600);
    let old = SystemTime::now() - Duration::from_secs(10);
    OpenOptions::new()
        .write(true)
        .open(&stale)
        .unwrap()
        .set_modified(old)
        .unwrap();
    ArtifactCache::open(
        tmp.path().to_path_buf(),
        cache_config(40),
        StartupId::generate(),
    )
    .unwrap();
    assert!(!stale.exists());
}

#[test]
fn s3_failure_debug_json_never_includes_secrets() {
    let fail = S3Failure::new(ErrorCode::S3Unavailable, S3Stage::Auth);
    let dbg = format!("{fail:?}");
    let disp = fail.to_string();
    assert!(dbg.contains("AUTH"));
    assert!(!dbg.contains("AKIA"));
    assert!(!disp.contains("system/"));
    let err = fail.to_platform_error();
    let json = serde_json::to_string(&err).unwrap();
    assert!(json.contains("S3_UNAVAILABLE"));
    assert!(!json.contains("Authorization"));
}

#[test]
fn s3_failure_classification_matrix_is_stable_and_secret_safe() {
    let stages = [
        (S3Stage::Dns, "DNS", "s3 dns resolution failed"),
        (S3Stage::Tls, "TLS", "s3 tls verification failed"),
        (S3Stage::Auth, "AUTH", "s3 authentication failed"),
        (
            S3Stage::Signature,
            "SIGNATURE",
            "s3 request signature was rejected",
        ),
        (S3Stage::Region, "REGION", "s3 region mismatch"),
        (S3Stage::Bucket, "BUCKET", "s3 bucket is unavailable"),
        (S3Stage::Policy, "POLICY", "s3 access was denied by policy"),
        (S3Stage::Timeout, "TIMEOUT", "s3 request timed out"),
        (S3Stage::Server, "SERVER", "s3 returned a server error"),
        (S3Stage::Delete, "DELETE", "s3 object delete failed"),
        (
            S3Stage::Integrity,
            "INTEGRITY",
            "s3 object failed integrity verification",
        ),
        (S3Stage::NotFound, "NOT_FOUND", "s3 object was not found"),
    ];
    for (stage, token, message) in stages {
        assert_eq!(stage.as_str(), token);
        assert_eq!(stage.to_string(), token);
        let code = if stage == S3Stage::Integrity {
            ErrorCode::ArtifactIntegrityError
        } else {
            ErrorCode::S3Unavailable
        };
        let failure = S3Failure::new(code, stage);
        assert_eq!(failure.code(), code);
        assert_eq!(failure.stage(), stage);
        assert_eq!(failure.to_platform_error().message(), message);
        let converted: open_compute_core::PlatformError = failure.into();
        assert_eq!(converted.code(), code);
        assert!(!format!("{failure:?}").contains("AKIA"));
        assert_eq!(failure.to_string(), format!("{}: {token}", code.as_str()));
    }

    assert_eq!(integrity_error().code(), ErrorCode::ArtifactIntegrityError);
    assert_eq!(
        unavailable(S3Stage::Integrity).code(),
        ErrorCode::ArtifactIntegrityError
    );
    assert_eq!(
        unavailable(S3Stage::Server).code(),
        ErrorCode::S3Unavailable
    );
    let missing = unavailable(S3Stage::NotFound);
    assert!(is_not_found(&missing));
    assert!(!is_not_found(&unavailable(S3Stage::Server)));

    let http_cases = [
        (404, OpKind::Put, S3Stage::NotFound),
        (404, OpKind::Delete, S3Stage::Delete),
        (301, OpKind::Head, S3Stage::Region),
        (307, OpKind::Get, S3Stage::Region),
        (400, OpKind::List, S3Stage::Region),
        (401, OpKind::Put, S3Stage::Auth),
        (403, OpKind::Put, S3Stage::Policy),
        (408, OpKind::Put, S3Stage::Timeout),
        (504, OpKind::Put, S3Stage::Timeout),
        (500, OpKind::Put, S3Stage::Server),
        (418, OpKind::Put, S3Stage::Server),
    ];
    for (status, operation, expected) in http_cases {
        assert_eq!(classify_http_status(status, operation), expected);
    }

    let service_cases = [
        ("InvalidAccessKeyId", 403, OpKind::Put, S3Stage::Auth),
        ("InvalidClientTokenId", 403, OpKind::Head, S3Stage::Auth),
        (
            "SignatureDoesNotMatch",
            403,
            OpKind::Get,
            S3Stage::Signature,
        ),
        ("AccessDenied", 403, OpKind::List, S3Stage::Policy),
        ("AllAccessDisabled", 403, OpKind::Put, S3Stage::Policy),
        ("NoSuchBucket", 404, OpKind::Put, S3Stage::Bucket),
        ("PermanentRedirect", 301, OpKind::Head, S3Stage::Bucket),
        (
            "AuthorizationHeaderMalformed",
            400,
            OpKind::Get,
            S3Stage::Region,
        ),
        ("illegal location", 400, OpKind::Put, S3Stage::Region),
        ("NoSuchKey", 404, OpKind::Get, S3Stage::NotFound),
        ("not found", 404, OpKind::Delete, S3Stage::Delete),
        ("InternalError", 500, OpKind::List, S3Stage::Server),
    ];
    for (code, status, operation, expected) in service_cases {
        assert_eq!(classify_service_code(code, status, operation), expected);
    }

    let connector_cases = [
        ("dns lookup failed", S3Stage::Dns),
        ("certificate verify failed", S3Stage::Tls),
        ("request timed out", S3Stage::Timeout),
        ("connection reset", S3Stage::Server),
    ];
    for (message, expected) in connector_cases {
        let connector = ConnectorError::other(IoError::other(message).into(), None);
        assert_eq!(classify_connector(&connector), expected);
    }
}

#[test]
fn sdk_timeout_errors_remain_timeout_at_operation_boundaries() {
    use aws_sdk_s3::error::SdkError;
    use aws_sdk_s3::operation::delete_object::DeleteObjectError;
    use aws_sdk_s3::operation::head_object::HeadObjectError;
    use aws_smithy_runtime_api::client::orchestrator::HttpResponse;

    let delete = SdkError::<DeleteObjectError, HttpResponse>::timeout_error(IoError::new(
        std::io::ErrorKind::TimedOut,
        "timeout",
    ));
    let mapped = crate::error::from_delete(&delete);
    assert_eq!(mapped.code(), ErrorCode::S3Unavailable);
    assert!(mapped.message().contains("timed out"));

    let head = SdkError::<HeadObjectError, HttpResponse>::timeout_error(IoError::new(
        std::io::ErrorKind::TimedOut,
        "timeout",
    ));
    let mapped = crate::error::from_head(&head);
    assert_eq!(mapped.code(), ErrorCode::S3Unavailable);
    assert!(mapped.message().contains("timed out"));
}

#[tokio::test]
async fn artifact_store_rejects_stream_file_download_and_remote_failures() {
    let mock = MockS3::spawn("open-compute").await;
    let store = ArtifactStore::new(client_for(&mock).await);
    let empty_digest = hex::encode(Sha256::digest([]));

    let stream_error = store
        .put_verified(
            stream::iter(vec![Err::<Bytes, IoError>(IoError::other("read failed"))]),
            &empty_digest,
            0,
        )
        .await
        .unwrap_err();
    assert_eq!(stream_error.code(), ErrorCode::S3Unavailable);

    let too_many = store
        .put_verified(
            stream::iter(vec![Ok::<Bytes, IoError>(Bytes::from_static(b"ab"))]),
            &hex::encode(Sha256::digest(b"ab")),
            1,
        )
        .await
        .unwrap_err();
    assert_eq!(too_many.code(), ErrorCode::LimitInvalid);
    let too_few = store
        .put_verified(
            stream::iter(vec![Ok::<Bytes, IoError>(Bytes::from_static(b"a"))]),
            &hex::encode(Sha256::digest(b"a")),
            2,
        )
        .await
        .unwrap_err();
    assert_eq!(too_few.code(), ErrorCode::ArtifactIntegrityError);
    assert_eq!(
        store
            .put_verified(stream::empty::<Result<Bytes, IoError>>(), "bad-digest", 0)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ConfigInvalid
    );

    let temp = TempDir::new().unwrap();
    let missing = temp.path().join("missing");
    assert_eq!(
        store
            .put_verified_file(&missing, &empty_digest, 0)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::DiskHardLimit
    );
    assert_eq!(
        store
            .put_verified_file(temp.path(), &empty_digest, 0)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ArtifactIntegrityError
    );
    let staged = temp.path().join("staged");
    fs::write(&staged, b"abc").unwrap();
    assert_eq!(
        store
            .put_verified_file(&staged, &hex::encode(Sha256::digest(b"abc")), 2)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ArtifactIntegrityError
    );
    assert_eq!(
        store
            .put_verified_file(&staged, &hex::encode(Sha256::digest(b"abc")), 65 * 1024)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::LimitInvalid
    );

    let payload = Bytes::from_static(b"writer-failure");
    let digest = hex::encode(Sha256::digest(&payload));
    let artifact = store
        .put_verified(
            stream::iter(vec![Ok::<Bytes, IoError>(payload)]),
            &digest,
            14,
        )
        .await
        .unwrap();
    struct FailingWriter;
    impl Write for FailingWriter {
        fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
            Err(IoError::other("disk full"))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    assert_eq!(
        store
            .download_verified(&artifact, &mut FailingWriter)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::DiskHardLimit
    );

    let oversized = ArtifactRef::new(1, &empty_digest, 65 * 1024).unwrap();
    assert_eq!(
        store
            .download_verified(&oversized, &mut std::io::sink())
            .await
            .unwrap_err()
            .code(),
        ErrorCode::LimitInvalid
    );
    let absent = ArtifactRef::new(1, &"11".repeat(32), 1).unwrap();
    let absent_error = store.open(&absent).await.unwrap_err();
    assert_eq!(absent_error.code(), ErrorCode::S3Unavailable);
    assert!(is_not_found(&absent_error));

    mock.set_fault(Fault::CorruptMetadata);
    assert_eq!(
        store.head(&artifact).await.unwrap_err().code(),
        ErrorCode::ArtifactIntegrityError
    );
    mock.set_fault(Fault::DeleteFail);
    assert_eq!(
        store
            .delete_unreferenced(&artifact)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::S3Unavailable
    );
    mock.set_fault(Fault::ServerError);
    assert_eq!(
        store.list_candidates().await.unwrap_err().code(),
        ErrorCode::S3Unavailable
    );
}

#[tokio::test]
async fn artifact_store_integrity_and_existing_file_paths() {
    let mock = MockS3::spawn("open-compute").await;
    let store = ArtifactStore::new(client_for(&mock).await);
    let payload = b"artifact-body";
    let digest = hex::encode(Sha256::digest(payload));
    let artifact = store
        .put_verified(
            stream::iter(vec![Ok::<Bytes, IoError>(Bytes::copy_from_slice(payload))]),
            &digest,
            payload.len() as u64,
        )
        .await
        .unwrap();

    let temp = TempDir::new().unwrap();
    let staged = temp.path().join("staged");
    fs::write(&staged, payload).unwrap();
    assert_eq!(
        store
            .put_verified_file(&staged, &digest, payload.len() as u64)
            .await
            .unwrap(),
        artifact
    );

    let wrong_size = ArtifactRef::new(1, &digest, payload.len() as u64 + 1).unwrap();
    assert_eq!(
        store.head(&wrong_size).await.unwrap_err().code(),
        ErrorCode::ArtifactIntegrityError
    );
    assert_eq!(
        store
            .download_verified(&wrong_size, &mut std::io::sink())
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ArtifactIntegrityError
    );
    let wrong_digest = ArtifactRef::new(1, &"11".repeat(32), payload.len() as u64).unwrap();
    mock.put_raw(&wrong_digest.physical_key("system/"), payload.to_vec());
    assert_eq!(
        store
            .download_verified(&wrong_digest, &mut std::io::sink())
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ArtifactIntegrityError
    );

    mock.set_fault(Fault::CorruptBody);
    assert_eq!(
        store
            .download_verified(&artifact, &mut std::io::sink())
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ArtifactIntegrityError
    );
    mock.set_fault(Fault::ServerError);
    assert_eq!(
        store
            .put_verified(
                stream::iter(vec![Ok::<Bytes, IoError>(Bytes::copy_from_slice(b"new"))]),
                &hex::encode(Sha256::digest(b"new")),
                3,
            )
            .await
            .unwrap_err()
            .code(),
        ErrorCode::S3Unavailable
    );
    assert_eq!(
        store
            .put_verified_file(&staged, &digest, payload.len() as u64)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::S3Unavailable
    );

    mock.set_fault(Fault::None);
    let maximum = 64 * 1024_u64;
    let large_digest = "22".repeat(32);
    let large = ArtifactRef::new(1, &large_digest, maximum).unwrap();
    mock.put_raw(
        &large.physical_key("system/"),
        vec![b'x'; maximum as usize + 1],
    );
    assert_eq!(
        store
            .download_verified(&large, &mut std::io::sink())
            .await
            .unwrap_err()
            .code(),
        ErrorCode::LimitInvalid
    );
}

#[tokio::test]
async fn kv_backup_objects_are_host_scoped_immutable_and_verified() {
    let mock = MockS3::spawn("open-compute").await;
    let store = ArtifactStore::new(client_for(&mock).await);
    let temp = TempDir::new().unwrap();
    let staged = temp.path().join("backup.sqlite");
    let payload = b"sqlite-backup";
    fs::write(&staged, payload).unwrap();
    let digest = hex::encode(Sha256::digest(payload));
    let relative = "backups/kv/account/resource/backup/data.sqlite";

    assert_eq!(
        store.kv_backup_key(relative).unwrap(),
        format!("system/{relative}")
    );
    for invalid in ["", "/backups/kv/x", "artifacts/x", "backups/kv/../x"] {
        assert_eq!(
            store.kv_backup_key(invalid).unwrap_err().code(),
            ErrorCode::ConfigInvalid
        );
    }
    assert_eq!(
        store
            .put_kv_backup_file(relative, temp.path(), &digest, payload.len() as u64)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ArtifactIntegrityError
    );
    assert_eq!(
        store
            .put_kv_backup_file(relative, &staged, &digest, payload.len() as u64 + 1)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ArtifactIntegrityError
    );
    assert_eq!(
        store
            .put_kv_backup_file(relative, &staged, &"11".repeat(32), payload.len() as u64)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ArtifactIntegrityError
    );

    let key = store
        .put_kv_backup_file(relative, &staged, &digest, payload.len() as u64)
        .await
        .unwrap();
    let mut restored = Vec::new();
    store
        .download_kv_backup(&key, &digest, payload.len() as u64, &mut restored)
        .await
        .unwrap();
    assert_eq!(restored, payload);
    assert_eq!(
        store
            .download_kv_backup(
                "system/artifacts/x",
                &digest,
                payload.len() as u64,
                &mut Vec::new()
            )
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ConfigInvalid
    );

    mock.set_fault(Fault::CorruptMetadata);
    assert_eq!(
        store
            .download_kv_backup(&key, &digest, payload.len() as u64, &mut Vec::new())
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ArtifactIntegrityError
    );
    mock.set_fault(Fault::CorruptBody);
    assert_eq!(
        store
            .download_kv_backup(&key, &digest, payload.len() as u64, &mut Vec::new())
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ArtifactIntegrityError
    );
    mock.set_fault(Fault::DeleteFail);
    assert_eq!(
        store.delete_kv_backup(&key).await.unwrap_err().code(),
        ErrorCode::S3Unavailable
    );
    mock.set_fault(Fault::None);
    store.delete_kv_backup(&key).await.unwrap();
    assert_eq!(
        store
            .delete_kv_backup("system/artifacts/x")
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ConfigInvalid
    );
}

#[tokio::test]
async fn d1_backup_objects_are_product_scoped_and_verified() {
    let mock = MockS3::spawn("open-compute").await;
    let store = ArtifactStore::new(client_for(&mock).await);
    let temp = TempDir::new().unwrap();
    let staged = temp.path().join("data.sqlite");
    let payload = b"d1-sqlite-backup";
    fs::write(&staged, payload).unwrap();
    let digest = hex::encode(Sha256::digest(payload));
    let relative = "backups/d1/resource/backup/data.sqlite";
    let kv_relative = "backups/kv/resource/backup/data.sqlite";

    assert_eq!(
        store.d1_backup_key(relative).unwrap(),
        format!("system/{relative}")
    );
    assert_eq!(
        store.kv_backup_key(relative).unwrap_err().code(),
        ErrorCode::ConfigInvalid
    );
    assert_eq!(
        store.d1_backup_key(kv_relative).unwrap_err().code(),
        ErrorCode::ConfigInvalid
    );

    let key = store
        .put_d1_backup_file(relative, &staged, &digest, payload.len() as u64)
        .await
        .unwrap();
    assert_eq!(
        store
            .download_kv_backup(&key, &digest, payload.len() as u64, &mut Vec::new())
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ConfigInvalid
    );
    let mut restored = Vec::new();
    store
        .download_d1_backup(&key, &digest, payload.len() as u64, &mut restored)
        .await
        .unwrap();
    assert_eq!(restored, payload);

    let manifest = Bytes::from_static(br#"{"schema":1}"#);
    let manifest_key = store
        .put_d1_backup_manifest("backups/d1/resource/backup/manifest.json", manifest.clone())
        .await
        .unwrap();
    assert_eq!(store.d1_backup_manifest_key(&key).unwrap(), manifest_key);
    assert_eq!(
        store.get_d1_backup_manifest(&manifest_key).await.unwrap(),
        manifest
    );
    assert_eq!(
        store.kv_backup_manifest_key(&key).unwrap_err().code(),
        ErrorCode::ConfigInvalid
    );

    store.delete_d1_backup(&key).await.unwrap();
    assert_eq!(
        store.delete_kv_backup(&key).await.unwrap_err().code(),
        ErrorCode::ConfigInvalid
    );
}

#[tokio::test]
async fn concurrent_put_precondition_races_verify_the_existing_winner() {
    let mock = MockS3::spawn("open-compute").await;
    let store = ArtifactStore::new(client_for(&mock).await);
    let payload = Bytes::from_static(b"concurrent-stream-race");
    let digest = hex::encode(Sha256::digest(&payload));
    mock.synchronize_next_heads(2);
    let first = store.put_verified(
        stream::iter(vec![Ok::<Bytes, IoError>(payload.clone())]),
        &digest,
        payload.len() as u64,
    );
    let second = store.put_verified(
        stream::iter(vec![Ok::<Bytes, IoError>(payload.clone())]),
        &digest,
        payload.len() as u64,
    );
    let (first, second) = tokio::join!(first, second);
    assert_eq!(first.unwrap(), second.unwrap());

    let file_payload = b"concurrent-file-race";
    let file_digest = hex::encode(Sha256::digest(file_payload));
    let dir = TempDir::new().unwrap();
    let staged = dir.path().join("staged");
    fs::write(&staged, file_payload).unwrap();
    mock.synchronize_next_heads(2);
    let first = store.put_verified_file(&staged, &file_digest, file_payload.len() as u64);
    let second = store.put_verified_file(&staged, &file_digest, file_payload.len() as u64);
    let (first, second) = tokio::join!(first, second);
    assert_eq!(first.unwrap(), second.unwrap());
}

#[tokio::test]
async fn artifact_listing_skips_invalid_keys_and_gc_respects_missing_time() {
    let mock = MockS3::spawn("open-compute").await;
    let store = ArtifactStore::new(client_for(&mock).await);
    mock.put_raw("system/artifacts/v1/sha256/not-valid", b"bad".to_vec());
    let payload = b"candidate";
    let digest = hex::encode(Sha256::digest(payload));
    let artifact = ArtifactRef::new(1, &digest, payload.len() as u64).unwrap();
    mock.put_raw(&artifact.physical_key("system/"), payload.to_vec());

    let candidates = store.list_candidates().await.unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].artifact, artifact);
    let mut referenced = HashSet::new();
    referenced.insert(artifact.clone());
    assert_eq!(
        store
            .gc_unreferenced(&referenced, SystemTime::now())
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        store
            .gc_unreferenced(&HashSet::new(), SystemTime::UNIX_EPOCH)
            .await
            .unwrap(),
        0
    );

    mock.set_omit_last_modified(true);
    assert_eq!(
        store
            .gc_unreferenced(&HashSet::new(), SystemTime::now())
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn disk_write_error_rejects_file_root() {
    let tmp = TempDir::new().unwrap();
    let file_root = tmp.path().join("not-a-dir");
    fs::write(&file_root, b"nope").unwrap();
    let err = ArtifactCache::open(file_root, cache_config(1024), StartupId::generate());
    assert_eq!(err.unwrap_err().code(), ErrorCode::PathInvalid);
}

#[tokio::test]
async fn pin_covers_first_use_hash_against_eviction() {
    let payload = b"first-use-hash-pin-body";
    let digest = hex::encode(Sha256::digest(payload));
    let artifact = ArtifactRef::new(ARTIFACT_KEY_VERSION, &digest, payload.len() as u64).unwrap();
    let tmp = TempDir::new().unwrap();
    let dest = cache_entry_path(tmp.path(), &digest);
    fs::create_dir_all(dest.parent().unwrap()).unwrap();
    fs::write(&dest, payload).unwrap();
    let cache = Arc::new(
        ArtifactCache::open(
            tmp.path().to_path_buf(),
            cache_config(payload.len() as u64),
            StartupId::generate(),
        )
        .unwrap(),
    );
    assert!(cache.is_indexed_for_test(&digest));

    struct Pause {
        in_hash: bool,
        release: bool,
    }
    let pause = Arc::new((
        Mutex::new(Pause {
            in_hash: false,
            release: false,
        }),
        Condvar::new(),
    ));
    let pause_hook = Arc::clone(&pause);
    let _guard = install_hash_pause(Arc::new(move || {
        let (lock, cv) = &*pause_hook;
        let mut g = lock.lock().unwrap();
        g.in_hash = true;
        cv.notify_all();
        while !g.release {
            g = cv.wait(g).unwrap();
        }
    }));

    let cache_hit = Arc::clone(&cache);
    let artifact_hit = artifact.clone();
    let join = std::thread::spawn(move || cache_hit.try_hit_for_test(&artifact_hit));

    {
        let (lock, cv) = &*pause;
        let mut g = lock.lock().unwrap();
        while !g.in_hash {
            g = cv.wait(g).unwrap();
        }
    }

    cache.evict_if_needed().await.unwrap();
    assert!(
        dest.exists(),
        "evictor must not unlink a hashing first-use pin"
    );
    assert!(cache.is_indexed_for_test(&digest));

    {
        let (lock, cv) = &*pause;
        let mut g = lock.lock().unwrap();
        g.release = true;
        cv.notify_all();
    }

    let mut pinned = join.join().unwrap().unwrap().unwrap();
    assert_eq!(pinned.read_all().unwrap(), payload);
    assert!(dest.exists());
    assert!(cache.is_indexed_for_test(&digest));
    drop(pinned);
}

#[test]
fn cache_open_rejects_symlink_root() {
    let tmp = TempDir::new().unwrap();
    let real = tmp.path().join("real-root");
    fs::create_dir(&real).unwrap();
    let link = tmp.path().join("link-root");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let err = ArtifactCache::open(link, cache_config(1024), StartupId::generate()).unwrap_err();
    assert_eq!(err.code(), ErrorCode::PathInvalid);
    assert!(real.exists());
    assert!(fs::read_dir(&real).unwrap().next().is_none());
}

#[test]
fn cache_open_rejects_symlink_ancestor_without_mutating_outside() {
    let tmp = TempDir::new().unwrap();
    let outside = tmp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    let sentinel = outside.join("sentinel");
    write_mode(&sentinel, "do-not-touch", 0o640);
    fs::set_permissions(&outside, fs::Permissions::from_mode(0o755)).unwrap();
    let outside_mode = fs::metadata(&outside).unwrap().permissions().mode();
    let sentinel_mode = fs::metadata(&sentinel).unwrap().permissions().mode();
    let sentinel_bytes = fs::read(&sentinel).unwrap();

    let base = tmp.path().join("base");
    fs::create_dir(&base).unwrap();
    let link = base.join("link");
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    let cache_root = link.join("cache");

    let err =
        ArtifactCache::open(cache_root, cache_config(1024), StartupId::generate()).unwrap_err();
    assert_eq!(err.code(), ErrorCode::PathInvalid);
    assert!(!outside.join("cache").exists());
    assert!(!outside.join("sha256").exists());
    assert_eq!(
        fs::metadata(&outside).unwrap().permissions().mode(),
        outside_mode
    );
    assert_eq!(fs::read(&sentinel).unwrap(), sentinel_bytes);
    assert_eq!(
        fs::metadata(&sentinel).unwrap().permissions().mode(),
        sentinel_mode
    );
}

#[test]
fn cache_open_rejects_relative_symlink_ancestor_without_mutating_outside() {
    let tmp = TempDir::new().unwrap();
    let base = tmp.path().join("base");
    fs::create_dir(&base).unwrap();
    let outside = base.join("outside");
    fs::create_dir(&outside).unwrap();
    let sentinel = outside.join("sentinel");
    write_mode(&sentinel, "do-not-touch", 0o640);
    fs::set_permissions(&outside, fs::Permissions::from_mode(0o755)).unwrap();
    let outside_mode = fs::metadata(&outside).unwrap().permissions().mode();
    let sentinel_mode = fs::metadata(&sentinel).unwrap().permissions().mode();
    let sentinel_bytes = fs::read(&sentinel).unwrap();

    let link = base.join("link");
    std::os::unix::fs::symlink(Path::new("outside"), &link).unwrap();
    let cache_root = link.join("cache");

    let err =
        ArtifactCache::open(cache_root, cache_config(1024), StartupId::generate()).unwrap_err();
    assert_eq!(err.code(), ErrorCode::PathInvalid);
    assert!(!outside.join("cache").exists());
    assert!(!outside.join("sha256").exists());
    assert_eq!(
        fs::metadata(&outside).unwrap().permissions().mode(),
        outside_mode
    );
    assert_eq!(fs::read(&sentinel).unwrap(), sentinel_bytes);
    assert_eq!(
        fs::metadata(&sentinel).unwrap().permissions().mode(),
        sentinel_mode
    );
}
