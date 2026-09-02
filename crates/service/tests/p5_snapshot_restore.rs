//! Focused Vectorize and AI Search snapshot/restore regression.

#[path = "p1_snapshot_restore/p5_search.rs"]
mod p5_search;

use open_compute_artifacts::{
    AiSearchObjectStore, MockS3, S3ArtifactClient, resolve_s3_credentials,
};
use open_compute_core::SystemClock;
use open_compute_service::backup_cli::{backup_create, backup_inspect, backup_restore};
use open_compute_service::config_load::load_platform_config;
use open_compute_storage::{PlatformStorage, SchedulerStore, inspect_control_inventory};
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn write_mode(path: &Path, bytes: &[u8], mode: u32) {
    fs::write(path, bytes).expect("write fixture");
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("fixture mode");
}

fn write_config(
    root: &Path,
    name: &str,
    data_dir: &Path,
    master_key: &Path,
    access_key: &Path,
    secret_key: &Path,
    endpoint: &str,
) -> PathBuf {
    let path = root.join(format!("{name}.toml"));
    fs::write(
        &path,
        format!(
            r#"
[server]
public_bind = "127.0.0.1:0"
admin_bind = "127.0.0.1:0"

[storage]
data_dir = "{}"
master_key_file = "{}"

[s3]
endpoint = "{endpoint}"
region = "us-east-1"
bucket = "open-compute"
force_path_style = true
access_key_id_file = "{}"
secret_access_key_file = "{}"
prefix = "system/"
max_retries = 1
retry_backoff_ms = 10
connect_timeout_ms = 500
request_timeout_ms = 2000

[runtime]
"#,
            data_dir.display(),
            master_key.display(),
            access_key.display(),
            secret_key.display(),
        ),
    )
    .expect("config");
    path
}

#[test]
fn p5_vectorize_and_ai_search_snapshot_restore_is_complete_and_fail_closed() {
    std::thread::Builder::new()
        .name("p5-snapshot-restore".to_owned())
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime")
                .block_on(snapshot_restore());
        })
        .expect("P5 snapshot thread")
        .join()
        .expect("P5 snapshot thread result");
}

async fn snapshot_restore() {
    let temp = TempDir::new().expect("temp");
    let root = fs::canonicalize(temp.path()).expect("canonical temp");
    let mock = MockS3::spawn("open-compute").await;
    let access_key = root.join("s3-access-key");
    let secret_key = root.join("s3-secret-key");
    write_mode(&access_key, b"AKIAEXAMPLEKEYID01", 0o600);
    write_mode(
        &secret_key,
        b"wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
        0o600,
    );
    let master_key = root.join("recovery-master.key");
    let source_data = root.join("source-data");
    fs::create_dir(&source_data).expect("source data");
    let source_config = write_config(
        &root,
        "source",
        &source_data,
        &master_key,
        &access_key,
        &secret_key,
        &mock.endpoint,
    );
    let source = load_platform_config(&source_config).expect("source config");
    let credentials = resolve_s3_credentials(&source.config.s3).expect("S3 credentials");
    let client = S3ArtifactClient::connect(
        &source.config.s3,
        &credentials,
        source.config.hardening.max_snapshot_file_bytes,
    )
    .expect("S3 client");
    let objects = AiSearchObjectStore::new(client);
    let storage =
        PlatformStorage::bootstrap(&source.config.storage, &SystemClock).expect("source storage");
    let scheduler_path = storage
        .data_dir()
        .ensure_scheduler_db()
        .expect("scheduler path");
    drop(SchedulerStore::open(&scheduler_path, 5_000, 1).expect("scheduler"));
    let object_path = root.join("ai-search-object.txt");
    let fixture = p5_search::seed(&storage, &objects, &object_path).await;
    let inventory = inspect_control_inventory(storage.db()).expect("source inventory");
    assert_eq!(inventory.vectorize_indexes, 1);
    assert_eq!(inventory.ai_search_namespaces, 1);
    assert_eq!(inventory.ai_search_instances, 1);
    drop(storage);

    let snapshot = backup_create(&source, "p5-search-focused")
        .await
        .expect("create snapshot");
    assert!(
        backup_inspect(&source, &snapshot.snapshot_id, true)
            .await
            .expect("verify snapshot")
            .verified
    );

    objects
        .delete_exact(&fixture.object, &fixture.object_key)
        .await
        .expect("remove external object");
    let missing_target = root.join("missing-object-restore");
    let missing_config = write_config(
        &root,
        "missing",
        &missing_target,
        &master_key,
        &access_key,
        &secret_key,
        &mock.endpoint,
    );
    let missing = load_platform_config(&missing_config).expect("missing target config");
    assert!(
        backup_restore(&missing, &snapshot.snapshot_id)
            .await
            .is_err()
    );
    assert!(!missing_target.exists());
    objects
        .put_file(&fixture.object, &object_path)
        .await
        .expect("restore external object");

    let target = root.join("restored-data");
    let restore_config = write_config(
        &root,
        "restore",
        &target,
        &master_key,
        &access_key,
        &secret_key,
        &mock.endpoint,
    );
    let restore = load_platform_config(&restore_config).expect("restore config");
    backup_restore(&restore, &snapshot.snapshot_id)
        .await
        .expect("restore snapshot");
    let restored = PlatformStorage::bootstrap(&restore.config.storage, &SystemClock)
        .expect("restored storage");
    p5_search::assert_restored(&restored, &fixture);
    let inventory = inspect_control_inventory(restored.db()).expect("restored inventory");
    assert_eq!(inventory.vectorize_indexes, 1);
    assert_eq!(inventory.ai_search_namespaces, 1);
    assert_eq!(inventory.ai_search_instances, 1);
    objects
        .verify(&fixture.object, &fixture.object_key)
        .await
        .expect("retained external object");
}
