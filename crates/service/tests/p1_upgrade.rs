//! P1.4 offline N -> N+1, crash-resume, serve, and snapshot-rollback Gate.

use open_compute_artifacts::{
    MockS3, S3ArtifactClient, SnapshotObjectStore, preflight_r2, preflight_s3,
    resolve_s3_credentials,
};
use open_compute_core::{
    ErrorCode, PlatformReleaseIdentityV1, PlatformSnapshotManifestV1, SnapshotFileRole,
    SnapshotFileV1, SnapshotTotalsV1, StartupId, SystemClock,
};
use open_compute_service::capabilities::{platform_capabilities, platform_config_policy_sha256};
use open_compute_service::cli::{execute, parse_from};
use open_compute_service::config_load::{LoadedConfig, load_platform_config};
use open_compute_service::doctor::{DoctorMode, doctor_report};
use open_compute_service::upgrade_cli::upgrade_check;
use open_compute_storage::{
    ControlDb, DataDir, RestoreTarget, SchedulerStore, inspect_control_db, inspect_master_key,
    sign_snapshot_manifest,
};
use rusqlite::backup::Backup;
use rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tempfile::TempDir;
use uuid::Uuid;

struct ConfigInput<'a> {
    root: &'a Path,
    name: &'a str,
    data_dir: &'a Path,
    key: &'a Path,
    access_key: &'a Path,
    secret_key: &'a Path,
    endpoint: &'a str,
    workerd: &'a Path,
    public_port: u16,
}

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace")
        .to_path_buf()
}

fn write_mode(path: &Path, bytes: &[u8], mode: u32) {
    fs::write(path, bytes).expect("write fixture");
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("fixture mode");
}

fn write_config(input: &ConfigInput<'_>) -> PathBuf {
    let root = workspace();
    let path = input.root.join(format!("{}.toml", input.name));
    fs::write(
        &path,
        format!(
            r#"
[server]
public_bind = "127.0.0.1:{public_port}"
admin_bind = "127.0.0.1:0"

[storage]
data_dir = "{data_dir}"
master_key_file = "{key}"
free_space_soft_bytes = 1073741824
free_space_hard_bytes = 268435456

[s3]
endpoint = "{endpoint}"
region = "us-east-1"
bucket = "open-compute"
force_path_style = true
access_key_id_file = "{access_key}"
secret_access_key_file = "{secret_key}"
prefix = "system/"
r2_prefix = "tenant/r2/"
max_retries = 1
retry_backoff_ms = 10
connect_timeout_ms = 500
request_timeout_ms = 3000

[runtime]
binary = "{workerd}"
lock_file = "{lock}"
assets_dir = "{assets}"
startup_timeout_ms = 10000
shutdown_grace_ms = 3000
kill_timeout_ms = 1000

[cache]
max_bytes = 1048576
high_watermark_ratio = 0.9
low_watermark_ratio = 0.8
max_artifact_bytes = 1048576

[metrics]
enabled = true
max_label_value_bytes = 64
max_series = 512
"#,
            public_port = input.public_port,
            data_dir = input.data_dir.display(),
            key = input.key.display(),
            endpoint = input.endpoint,
            access_key = input.access_key.display(),
            secret_key = input.secret_key.display(),
            workerd = input.workerd.display(),
            lock = root.join("runtime/workerd.lock.json").display(),
            assets = root.join("runtime").display(),
        ),
    )
    .expect("write config");
    path
}

async fn run_cli_json(config: &Path, args: &[&str]) -> serde_json::Value {
    let mut argv = vec![
        "platformd".to_owned(),
        "--config".to_owned(),
        config.to_string_lossy().into_owned(),
    ];
    argv.extend(args.iter().map(|value| (*value).to_owned()));
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let status = execute(
        parse_from(argv).expect("parse upgrade CLI"),
        &mut stdout,
        &mut stderr,
    )
    .await;
    assert_eq!(
        status,
        std::process::ExitCode::SUCCESS,
        "upgrade CLI stderr: {}",
        String::from_utf8_lossy(&stderr)
    );
    assert!(stderr.is_empty());
    serde_json::from_slice(&stdout).expect("upgrade CLI JSON")
}

fn downgrade_control_to_seven(path: &Path) {
    let connection = Connection::open(path).expect("open control");
    connection
        .pragma_update(None, "foreign_keys", "OFF")
        .expect("disable fixture foreign keys");
    let queue_triggers = {
        let mut statement = connection
            .prepare(
                "SELECT name FROM sqlite_master
                 WHERE type = 'trigger' AND name LIKE '%queue%'",
            )
            .expect("list Queue triggers");
        statement
            .query_map([], |row| row.get::<_, String>(0))
            .expect("read Queue triggers")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect Queue triggers")
    };
    for trigger in queue_triggers {
        connection
            .execute_batch(&format!("DROP TRIGGER \"{trigger}\";"))
            .expect("drop Queue trigger");
    }
    connection
        .execute_batch(
            "DROP TABLE cron_activations;
             DROP TABLE deployment_cron_declarations;
             DROP TABLE deployment_cron_configs;
             DROP TABLE queue_consumers;
             DROP TABLE deployment_queue_consumers;
             DROP TABLE queue_referrers;
             DROP TABLE queue_producer_bindings;
             ALTER TABLE control_idempotency DROP COLUMN queue_id;
             DROP TABLE queues;
             DELETE FROM schema_migrations WHERE version >= 8;
             PRAGMA user_version = 7;",
        )
        .expect("restore schema-seven control fixture");
    connection
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
        .expect("checkpoint");
}

fn downgrade_scheduler_to_one(path: &Path) {
    let connection = Connection::open(path).expect("open scheduler");
    connection
        .pragma_update(None, "foreign_keys", "OFF")
        .expect("disable fixture foreign keys");
    let queue_triggers = {
        let mut statement = connection
            .prepare(
                "SELECT name FROM sqlite_master
                 WHERE type = 'trigger' AND name LIKE '%queue%'",
            )
            .expect("list scheduler Queue triggers");
        statement
            .query_map([], |row| row.get::<_, String>(0))
            .expect("read scheduler Queue triggers")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect scheduler Queue triggers")
    };
    for trigger in queue_triggers {
        connection
            .execute_batch(&format!("DROP TRIGGER \"{trigger}\";"))
            .expect("drop scheduler Queue trigger");
    }
    connection
        .execute_batch(
            "DROP TABLE cron_runs;
             DROP TABLE cron_schedules;
             DROP TABLE queue_dlq_pending;
             DROP TABLE queue_delivery_batches;
             DROP TABLE queue_consumer_state;
             DROP TABLE queue_messages;
             DROP TABLE queue_state;
             DELETE FROM scheduler_migrations WHERE version >= 2;
             UPDATE scheduler_meta SET schema_version = 1;
             PRAGMA user_version = 1;
             PRAGMA wal_checkpoint(TRUNCATE);",
        )
        .expect("restore schema-one scheduler fixture");
}

fn sqlite_backup(source: &Path, destination: &Path) {
    let source_connection = Connection::open_with_flags(
        source,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .expect("open source");
    let mut destination_connection = Connection::open(destination).expect("open destination");
    let backup = Backup::new(&source_connection, &mut destination_connection).expect("backup");
    backup
        .run_to_completion(128, Duration::from_millis(1), None)
        .expect("backup complete");
    drop(backup);
    drop(destination_connection);
    fs::set_permissions(destination, fs::Permissions::from_mode(0o600)).expect("backup mode");
}

fn file_entry(
    role: SnapshotFileRole,
    logical_id: &str,
    restore_path: &str,
    object_key: String,
    path: &Path,
) -> SnapshotFileV1 {
    let bytes = fs::read(path).expect("snapshot bytes");
    SnapshotFileV1 {
        role,
        logical_id: logical_id.to_owned(),
        restore_path: restore_path.to_owned(),
        object_key,
        size: bytes.len() as u64,
        sha256: hex::encode(Sha256::digest(&bytes)),
        mode: 0o600,
    }
}

fn regular_files(root: &Path) -> Vec<PathBuf> {
    let mut pending = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).expect("read DO fixture") {
            let entry = entry.expect("DO entry");
            let kind = entry.file_type().expect("DO type");
            assert!(!kind.is_symlink());
            if kind.is_dir() {
                pending.push(entry.path());
            } else {
                assert!(kind.is_file());
                files.push(entry.path());
            }
        }
    }
    files.sort();
    files
}

async fn create_schema_seven_snapshot(
    loaded: &LoadedConfig,
    source_release: PlatformReleaseIdentityV1,
    snapshot_id: &str,
    platform_id: open_compute_core::PlatformId,
    key: &open_compute_storage::MasterKey,
    staging: &Path,
) -> PlatformSnapshotManifestV1 {
    let credentials = resolve_s3_credentials(&loaded.config.s3).expect("credentials");
    let client = S3ArtifactClient::connect(
        &loaded.config.s3,
        &credentials,
        loaded.config.hardening.max_snapshot_file_bytes,
    )
    .expect("S3 client");
    preflight_s3(&client, platform_id, StartupId::generate())
        .await
        .expect("S3 preflight");
    preflight_r2(&client, platform_id, StartupId::generate())
        .await
        .expect("R2 preflight");
    let objects = SnapshotObjectStore::new(client, platform_id);
    let prefix = objects.object_prefix(snapshot_id).expect("snapshot prefix");
    let control = staging.join(format!("{snapshot_id}-control.sqlite"));
    let scheduler = staging.join(format!("{snapshot_id}-scheduler.sqlite"));
    sqlite_backup(
        &loaded.config.storage.data_dir.join("control.sqlite"),
        &control,
    );
    sqlite_backup(
        &loaded.config.storage.data_dir.join("scheduler.sqlite"),
        &scheduler,
    );
    fs::set_permissions(&control, fs::Permissions::from_mode(0o600)).expect("control mode");
    fs::set_permissions(&scheduler, fs::Permissions::from_mode(0o600)).expect("scheduler mode");
    let mut object_paths = vec![control.clone(), scheduler.clone()];
    let mut files = vec![
        file_entry(
            SnapshotFileRole::ControlSqlite,
            "control",
            "control.sqlite",
            format!("{prefix}000000.bin"),
            &control,
        ),
        file_entry(
            SnapshotFileRole::SchedulerSqlite,
            "scheduler",
            "scheduler.sqlite",
            format!("{prefix}000001.bin"),
            &scheduler,
        ),
    ];
    let do_root = loaded.config.storage.data_dir.join("do");
    if do_root.is_dir() {
        for path in regular_files(&do_root) {
            let restore_path = path
                .strip_prefix(&loaded.config.storage.data_dir)
                .expect("DO relative path")
                .to_string_lossy()
                .replace('\\', "/");
            let index = files.len();
            files.push(file_entry(
                SnapshotFileRole::DurableObjectFile,
                &platform_id.to_string(),
                &restore_path,
                format!("{prefix}{index:06}.bin"),
                &path,
            ));
            object_paths.push(path);
        }
    }
    let totals = SnapshotTotalsV1 {
        files: files.len() as u32,
        bytes: files.iter().map(|entry| entry.size).sum(),
    };
    let mut manifest = PlatformSnapshotManifestV1 {
        schema_version: 1,
        snapshot_id: snapshot_id.to_owned(),
        platform_id: platform_id.to_string(),
        label: "p1-before-upgrade".to_owned(),
        created_at_ms: unix_ms(),
        source_release,
        source_schemas: BTreeMap::from([
            ("control".to_owned(), 7),
            ("scheduler".to_owned(), 1),
            ("kv".to_owned(), 1),
            ("d1".to_owned(), 1),
        ]),
        master_key_fingerprint: key.fingerprint().to_owned(),
        s3_authority_fingerprint: objects.authority_fingerprint(),
        r2_prefix_fingerprint: objects.r2_prefix_fingerprint(),
        config_policy_sha256: platform_config_policy_sha256(loaded).expect("config policy"),
        immutable_references: Vec::new(),
        files,
        totals,
        manifest_mac: "0".repeat(64),
    };
    sign_snapshot_manifest(&mut manifest, key).expect("sign manifest");
    manifest
        .validate(
            loaded.config.hardening.max_snapshot_files,
            loaded.config.hardening.max_snapshot_file_bytes,
            loaded.config.hardening.max_snapshot_total_bytes,
        )
        .expect("manifest");
    for (entry, path) in manifest.files.iter().zip(object_paths) {
        objects
            .put_file(&entry.object_key, &path, &entry.sha256, entry.size)
            .await
            .expect("snapshot object");
    }
    objects
        .put_manifest(
            snapshot_id,
            &serde_json::to_vec(&manifest).expect("manifest JSON"),
            loaded.config.hardening.max_snapshot_manifest_bytes,
        )
        .await
        .expect("manifest commit");
    manifest
}

fn old_fixture_accepts_schema_seven(path: &Path) -> bool {
    let Ok(db) = ControlDb::open_readonly(path, 5_000) else {
        return false;
    };
    open_compute_storage::migrations::inspect_schema(&db).ok() == Some(7)
}

async fn restore_schema_seven(
    loaded: &LoadedConfig,
    manifest: &PlatformSnapshotManifestV1,
    key: &open_compute_storage::MasterKey,
    target: &Path,
) {
    let restore = stage_schema_seven(loaded, manifest, target).await;
    restore
        .validate_and_publish(
            manifest,
            key.fingerprint(),
            loaded.config.storage.sqlite_busy_timeout_ms,
            br#"{"schema_version":1,"result":"p1-test-rollback"}"#,
        )
        .expect("publish rollback");
}

async fn stage_schema_seven(
    loaded: &LoadedConfig,
    manifest: &PlatformSnapshotManifestV1,
    target: &Path,
) -> RestoreTarget {
    let credentials = resolve_s3_credentials(&loaded.config.s3).expect("credentials");
    let client = S3ArtifactClient::connect(
        &loaded.config.s3,
        &credentials,
        loaded.config.hardening.max_snapshot_file_bytes,
    )
    .expect("S3 client");
    let objects =
        SnapshotObjectStore::new(client, manifest.platform_id.parse().expect("platform ID"));
    let restore = RestoreTarget::acquire(target).expect("restore target");
    for file in &manifest.files {
        let destination = restore
            .destination_for(&file.restore_path)
            .expect("restore path");
        objects
            .download_file(&file.object_key, &destination, &file.sha256, file.size)
            .await
            .expect("restore object");
    }
    restore
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind free port")
        .local_addr()
        .expect("local address")
        .port()
}

fn wait_ready(child: &mut std::process::Child, port: u16, timeout: Duration) {
    let started = Instant::now();
    let mut last_response = Vec::new();
    loop {
        if let Some(status) = child.try_wait().expect("daemon status") {
            panic!("upgraded platform exited before ready: {status}");
        }
        if let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) {
            stream
                .set_read_timeout(Some(Duration::from_secs(1)))
                .expect("read timeout");
            stream
                .write_all(
                    b"GET /health/ready HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
                )
                .expect("health request");
            let mut response = Vec::new();
            let _ = stream.read_to_end(&mut response);
            if response.starts_with(b"HTTP/1.1 200") {
                return;
            }
            last_response = response;
        }
        assert!(
            started.elapsed() < timeout,
            "upgraded platform did not become ready: {}",
            String::from_utf8_lossy(&last_response)
        );
        thread::sleep(Duration::from_millis(50));
    }
}

fn unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(i64::MAX)
}

#[test]
fn p1_schema_upgrade_crash_resume_serve_and_snapshot_rollback() {
    thread::Builder::new()
        .name("p1-upgrade".to_owned())
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("runtime")
                .block_on(upgrade_gate());
        })
        .expect("P1 upgrade thread")
        .join()
        .expect("P1 upgrade result");
}

async fn upgrade_gate() {
    let temp = TempDir::new().expect("temp");
    let root = fs::canonicalize(temp.path()).expect("canonical temp");
    let mock = MockS3::spawn("open-compute").await;
    let access_key = root.join("s3-access-key");
    let secret_key = root.join("s3-secret-key");
    let master_key = root.join("recovery-master.key");
    write_mode(&access_key, b"AKIAUPGRADEEXAMPLE01", 0o600);
    write_mode(
        &secret_key,
        b"upgrade-example-secret-key-material-0001",
        0o600,
    );
    let workerd = std::env::var_os("OPEN_COMPUTE_TEST_WORKERD").map_or_else(
        || workspace().join("poc/.runtime-cache/v1.20260826.1/workerd"),
        PathBuf::from,
    );
    assert!(workerd.is_file(), "stock workerd is required");
    let source_data = root.join("source-data");
    fs::create_dir(&source_data).expect("source data");
    let public_port = free_port();
    let source_config = write_config(&ConfigInput {
        root: &root,
        name: "source",
        data_dir: &source_data,
        key: &master_key,
        access_key: &access_key,
        secret_key: &secret_key,
        endpoint: &mock.endpoint,
        workerd: &workerd,
        public_port,
    });
    let loaded = load_platform_config(&source_config).expect("source config");
    let storage =
        open_compute_storage::PlatformStorage::bootstrap(&loaded.config.storage, &SystemClock)
            .expect("source storage");
    let scheduler = storage.data_dir().ensure_scheduler_db().expect("scheduler");
    drop(SchedulerStore::open(&scheduler, 5_000, unix_ms()).expect("scheduler DB"));
    let platform_id = storage.identity().platform_id;
    let do_root = storage
        .data_dir()
        .prepare_durable_object_storage(&platform_id.to_string(), "workerd 2026-08-26")
        .expect("DO storage");
    write_mode(
        &do_root.join("upgrade-sentinel.bin"),
        b"p1-upgrade-do",
        0o600,
    );
    drop(storage);
    downgrade_control_to_seven(&source_data.join("control.sqlite"));
    downgrade_scheduler_to_one(&source_data.join("scheduler.sqlite"));
    assert!(old_fixture_accepts_schema_seven(
        &source_data.join("control.sqlite")
    ));

    let key = inspect_master_key(&loaded.config.storage).expect("master key");
    let mut source_release = platform_capabilities(&loaded)
        .expect("capabilities")
        .release;
    source_release.control_schema_version = 7;
    source_release.scheduler_schema_version = 1;
    let snapshot_id = Uuid::now_v7().hyphenated().to_string();
    let manifest = create_schema_seven_snapshot(
        &loaded,
        source_release.clone(),
        &snapshot_id,
        platform_id,
        &key,
        &root,
    )
    .await;
    let unsupported_id = Uuid::now_v7().hyphenated().to_string();
    let mut unsupported_release = source_release;
    unsupported_release.platform_version = "0.0.0-unsupported".to_owned();
    create_schema_seven_snapshot(
        &loaded,
        unsupported_release,
        &unsupported_id,
        platform_id,
        &key,
        &root,
    )
    .await;
    assert_eq!(
        upgrade_check(&loaded, &unsupported_id)
            .await
            .expect_err("unsupported release")
            .code(),
        ErrorCode::ReleaseUnsupported
    );
    let check = run_cli_json(
        &source_config,
        &[
            "upgrade",
            "check",
            "--from-snapshot",
            &snapshot_id,
            "--json",
        ],
    )
    .await;
    assert_eq!(
        (
            check["before"]["control"].as_u64(),
            check["target"]["control"].as_u64()
        ),
        (Some(7), Some(11))
    );

    let data_dir = DataDir::acquire_existing_offline(&loaded.config.storage).expect("offline");
    let control = ControlDb::open(
        &data_dir.control_db_path(),
        loaded.config.storage.sqlite_busy_timeout_ms,
    )
    .expect("control");
    assert_eq!(
        open_compute_storage::migrations::apply_with_fault(
            &control,
            &SystemClock,
            Some(open_compute_storage::MigrationFault::AfterCommit),
        )
        .expect_err("post-commit interruption")
        .code(),
        ErrorCode::MigrationFailed
    );
    drop(control);
    drop(data_dir);
    assert!(!old_fixture_accepts_schema_seven(
        &source_data.join("control.sqlite")
    ));
    let applied = run_cli_json(
        &source_config,
        &[
            "upgrade",
            "apply",
            "--from-snapshot",
            &snapshot_id,
            "--json",
        ],
    )
    .await;
    assert_eq!(
        (
            applied["before"]["control"].as_u64(),
            applied["target"]["control"].as_u64()
        ),
        (Some(8), Some(11))
    );

    let doctor = doctor_report(&loaded, DoctorMode::Full).await;
    assert!(!doctor.failed(), "doctor after upgrade: {doctor:?}");

    let mut daemon = Command::new(env!("CARGO_BIN_EXE_platformd"))
        .args([
            "--config",
            source_config.to_str().expect("config UTF-8"),
            "run",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start upgraded daemon");
    wait_ready(&mut daemon, public_port, Duration::from_secs(30));
    let status = Command::new("kill")
        .args(["-TERM", &daemon.id().to_string()])
        .status()
        .expect("SIGTERM");
    assert!(status.success());
    assert!(daemon.wait().expect("daemon output").success());

    let extra_target = root.join("rollback-extra-file");
    let extra_restore = stage_schema_seven(&loaded, &manifest, &extra_target).await;
    let extra = extra_restore
        .destination_for("do/workerd/unexpected.bin")
        .expect("unexpected restore file path");
    write_mode(&extra, b"unexpected", 0o600);
    assert_eq!(
        extra_restore
            .validate_and_publish(
                &manifest,
                key.fingerprint(),
                loaded.config.storage.sqlite_busy_timeout_ms,
                br#"{"schema_version":1}"#,
            )
            .expect_err("unexpected restore file")
            .code(),
        ErrorCode::RestoreInvalid
    );
    assert!(!extra_target.exists());

    let mode_target = root.join("rollback-broad-mode");
    let mode_restore = stage_schema_seven(&loaded, &manifest, &mode_target).await;
    fs::set_permissions(
        mode_restore.staging_root().join("control.sqlite"),
        fs::Permissions::from_mode(0o644),
    )
    .expect("broaden staged control mode");
    assert_eq!(
        mode_restore
            .validate_and_publish(
                &manifest,
                key.fingerprint(),
                loaded.config.storage.sqlite_busy_timeout_ms,
                br#"{"schema_version":1}"#,
            )
            .expect_err("broad restore file mode")
            .code(),
        ErrorCode::PathInvalid
    );
    assert!(!mode_target.exists());

    let key_target = root.join("rollback-wrong-key");
    let key_restore = stage_schema_seven(&loaded, &manifest, &key_target).await;
    assert_eq!(
        key_restore
            .validate_and_publish(
                &manifest,
                &"0".repeat(64),
                loaded.config.storage.sqlite_busy_timeout_ms,
                br#"{"schema_version":1}"#,
            )
            .expect_err("wrong restore key")
            .code(),
        ErrorCode::RestoreInvalid
    );
    assert!(!key_target.exists());

    let scheduler_target = root.join("rollback-wrong-scheduler-schema");
    let scheduler_restore = stage_schema_seven(&loaded, &manifest, &scheduler_target).await;
    let mut wrong_scheduler = manifest.clone();
    wrong_scheduler
        .source_schemas
        .insert("scheduler".to_owned(), 99);
    assert_eq!(
        scheduler_restore
            .validate_and_publish(
                &wrong_scheduler,
                key.fingerprint(),
                loaded.config.storage.sqlite_busy_timeout_ms,
                br#"{"schema_version":1}"#,
            )
            .expect_err("wrong scheduler schema")
            .code(),
        ErrorCode::RestoreInvalid
    );
    assert!(!scheduler_target.exists());

    let rollback_data = root.join("rollback-data");
    restore_schema_seven(&loaded, &manifest, &key, &rollback_data).await;
    assert!(old_fixture_accepts_schema_seven(
        &rollback_data.join("control.sqlite")
    ));
    let rollback_config = write_config(&ConfigInput {
        root: &root,
        name: "rollback",
        data_dir: &rollback_data,
        key: &master_key,
        access_key: &access_key,
        secret_key: &secret_key,
        endpoint: &mock.endpoint,
        workerd: &workerd,
        public_port: free_port(),
    });
    let refused = Command::new(env!("CARGO_BIN_EXE_platformd"))
        .args([
            "--config",
            rollback_config.to_str().expect("config UTF-8"),
            "run",
        ])
        .output()
        .expect("new release refusal");
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("UPGRADE_REQUIRED"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );

    let (_, restored_identity) = inspect_control_db(&rollback_data.join("control.sqlite"), 5_000)
        .expect("rollback identity");
    assert_eq!(restored_identity.platform_id, platform_id);
}
