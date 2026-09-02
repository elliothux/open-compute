//! P1.2/P1.3 full snapshot and fresh-host restore integration Gate.

mod common;
#[path = "p1_snapshot_restore/p5_search.rs"]
mod p5_search;
#[path = "p1_snapshot_restore/staged_validation.rs"]
mod staged_validation;

use common::load_file_only_platform_config;

use base64::Engine as _;
use open_compute_artifacts::{
    AiSearchObjectStore, MockS3, S3ArtifactClient, SnapshotObjectStore, resolve_s3_credentials,
};
use open_compute_core::{
    CronActivationId, DeploymentId, ErrorCode, PlatformSnapshotManifestV1, RequestId, SystemClock,
    WorkerId,
};
use open_compute_service::backup_cli::{
    backup_attest_restore_smoke, backup_create, backup_delete, backup_inspect, backup_list,
    backup_restore, backup_retention_plan,
};
use open_compute_service::cli::{execute, parse_from};
use open_compute_storage::{
    PlatformStorage, QueueConfig, QueueContentType, QueueEnqueueRequest, QueueMessageInput,
    QueueRepository, RestoreTarget, SchedulerStore, inspect_control_db, inspect_master_key,
    inspect_scheduler_db, sign_snapshot_manifest,
};
use open_compute_workers::{CreateQueueOutcome, CreateQueueRequest, QueueController};
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tempfile::TempDir;

fn write_mode(path: &Path, bytes: &[u8], mode: u32) {
    fs::write(path, bytes).expect("write fixture");
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("fixture mode");
}

struct ConfigInputs<'a> {
    root: &'a Path,
    name: &'a str,
    data_dir: &'a Path,
    master_key: &'a Path,
    access_key: &'a Path,
    secret_key: &'a Path,
    endpoint: &'a str,
    prefix: &'a str,
}

fn write_config(input: &ConfigInputs<'_>) -> PathBuf {
    let admin_token = input.root.join(format!("{}-admin.token", input.name));
    write_mode(&admin_token, b"p1-snapshot-admin\n", 0o600);
    let path = input.root.join(format!("{}.toml", input.name));
    fs::write(
        &path,
        format!(
            r#"
[server]
public_bind = "127.0.0.1:0"
admin_bind = "127.0.0.1:0"

[server.admin_auth]
file = "{admin_token}"

[storage]
data_dir = "{data}"
master_key_file = "{key}"

[s3]
endpoint = "{endpoint}"
region = "us-east-1"
bucket = "open-compute"
force_path_style = true
access_key_id_file = "{access_key}"
secret_access_key_file = "{secret_key}"
prefix = "{prefix}"
max_retries = 1
retry_backoff_ms = 10
connect_timeout_ms = 500
request_timeout_ms = 2000

[runtime]

[cache]
max_bytes = 1048576
high_watermark_ratio = 0.9
low_watermark_ratio = 0.8
max_artifact_bytes = 65536

[metrics]
enabled = true
max_label_value_bytes = 64
max_series = 1024
"#,
            data = input.data_dir.display(),
            key = input.master_key.display(),
            endpoint = input.endpoint,
            access_key = input.access_key.display(),
            secret_key = input.secret_key.display(),
            admin_token = admin_token.display(),
            prefix = input.prefix,
        ),
    )
    .expect("config");
    path
}

async fn run_cli_json(config: &Path, args: &[&str]) -> serde_json::Value {
    let mut argv = vec![
        "ocd".to_owned(),
        "--config".to_owned(),
        config.to_string_lossy().into_owned(),
    ];
    argv.extend(args.iter().map(|value| (*value).to_owned()));
    let cli = parse_from(argv).expect("parse CLI fixture");
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let status = execute(cli, &mut stdout, &mut stderr).await;
    assert_eq!(
        status,
        std::process::ExitCode::SUCCESS,
        "CLI stderr: {}",
        String::from_utf8_lossy(&stderr)
    );
    assert!(stderr.is_empty());
    serde_json::from_slice(&stdout).expect("CLI JSON")
}

async fn run_cli_human(config: &Path, args: &[&str]) -> String {
    let mut argv = vec![
        "ocd".to_owned(),
        "--config".to_owned(),
        config.to_string_lossy().into_owned(),
    ];
    argv.extend(args.iter().map(|value| (*value).to_owned()));
    let cli = parse_from(argv).expect("parse CLI fixture");
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let status = execute(cli, &mut stdout, &mut stderr).await;
    assert_eq!(
        status,
        std::process::ExitCode::SUCCESS,
        "CLI stderr: {}",
        String::from_utf8_lossy(&stderr)
    );
    assert!(stderr.is_empty());
    String::from_utf8(stdout).expect("CLI UTF-8")
}

#[test]
fn p1_full_snapshot_retention_and_fresh_host_restore_are_fail_closed() {
    std::thread::Builder::new()
        .name("p1-snapshot-restore".to_owned())
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime")
                .block_on(snapshot_restore_gate());
        })
        .expect("P1 Gate thread")
        .join()
        .expect("P1 Gate thread result");
}

async fn snapshot_restore_gate() {
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
    let source_config = write_config(&ConfigInputs {
        root: &root,
        name: "source",
        data_dir: &source_data,
        master_key: &master_key,
        access_key: &access_key,
        secret_key: &secret_key,
        endpoint: &mock.endpoint,
        prefix: "system/",
    });
    let source_loaded = load_file_only_platform_config(&source_config);
    let p5_credentials =
        resolve_s3_credentials(&source_loaded.config.s3).expect("P5 S3 credentials");
    let p5_client = S3ArtifactClient::connect(
        &source_loaded.config.s3,
        &p5_credentials,
        source_loaded.config.hardening.max_snapshot_file_bytes,
    )
    .expect("P5 S3 client");
    let p5_objects = AiSearchObjectStore::new(p5_client);
    let storage = PlatformStorage::bootstrap(&source_loaded.config.storage, &SystemClock)
        .expect("source bootstrap");
    let scheduler_path = storage
        .data_dir()
        .ensure_scheduler_db()
        .expect("scheduler path");
    let scheduler = Arc::new(SchedulerStore::open(&scheduler_path, 5_000, 1).expect("scheduler"));
    let account_id = storage.identity().default_account_id;
    let snapshot_queue = match QueueController::new(&storage, scheduler.clone())
        .create(&CreateQueueRequest {
            account_id,
            name: "snapshot-queue".to_owned(),
            config: QueueConfig::default(),
            idempotency_key: "snapshot-queue-create".to_owned(),
            request_id: RequestId::generate(),
            now_ms: 1_000,
        })
        .expect("create snapshot Queue")
    {
        CreateQueueOutcome::Applied(result) => result.queue.id,
        CreateQueueOutcome::Replay(_) => panic!("unexpected Queue create replay"),
    };
    scheduler
        .enqueue_queue(
            &QueueEnqueueRequest {
                queue_id: snapshot_queue,
                request_id: uuid::Uuid::now_v7(),
                output_gate: false,
                lifecycle_generation: 1,
                config_generation: 1,
                batch_delay_seconds: None,
                messages: vec![QueueMessageInput {
                    content_type: QueueContentType::Text,
                    body: b"snapshot-queue-body".to_vec(),
                    delay_seconds: Some(0),
                }],
            },
            1_001,
        )
        .expect("enqueue snapshot Queue message");
    let consumer_id = open_compute_core::QueueConsumerId::generate();
    let worker_id = WorkerId::generate();
    let deployment_id = DeploymentId::generate();
    scheduler
        .ensure_queue_consumer_projection(&open_compute_storage::QueueConsumerProjection {
            consumer_id,
            queue_id: snapshot_queue,
            consumer_generation: 1,
            deployment_id,
            worker_id,
            execution_generation: 1,
            entrypoint: None,
            config: open_compute_storage::QueueConsumerConfig {
                max_batch_size: 1,
                ..open_compute_storage::QueueConsumerConfig::default()
            },
            dead_letter_queue: None,
            descriptor_sha256: [7; 32],
            updated_at_ms: 1_002,
        })
        .expect("stage snapshot Queue consumer");
    scheduler
        .activate_queue_consumer(consumer_id, 1, 1_002)
        .expect("activate snapshot Queue consumer");
    let claimed_queue = scheduler
        .claim_queue_batches(1_002, 1_000, 250, 1, None)
        .map(|(items, _)| items)
        .expect("claim snapshot Queue batch")
        .pop()
        .expect("claimed snapshot Queue batch");

    let activation_id = CronActivationId::generate();
    scheduler
        .ensure_cron_schedule_projection(&open_compute_storage::CronScheduleProjection {
            activation_id,
            account_id,
            worker_id,
            deployment_id,
            execution_generation: 1,
            activation_generation: 1,
            expression: "* * * * *".to_owned(),
            expression_sha256: [8; 32],
            parser_version: 1,
            next_fire_at_ms: 60_000,
            updated_at_ms: 1_002,
        })
        .expect("stage snapshot Cron schedule");
    scheduler
        .activate_cron_schedule(activation_id, 1, 1_002)
        .expect("activate snapshot Cron schedule");
    assert_eq!(
        scheduler
            .project_due_cron_slots(60_000, 60_000, 1)
            .expect("project snapshot Cron slot")
            .projected,
        1
    );
    let claimed_cron = scheduler
        .claim_cron_runs(60_000, 1_000, 250, 1)
        .map(|(items, _)| items)
        .expect("claim snapshot Cron run")
        .pop()
        .expect("claimed snapshot Cron run");
    let do_root = storage
        .data_dir()
        .prepare_durable_object_storage(
            &storage.identity().platform_id.to_string(),
            &open_compute_runtime::embedded_runtime_lock()
                .expect("embedded runtime lock")
                .0
                .expected_version_output,
        )
        .expect("DO root");
    write_mode(
        &do_root.join("sentinel.bin"),
        b"durable-object-sentinel",
        0o600,
    );
    let p5_object_path = root.join("p5-ai-search-object.txt");
    let p5_fixture = p5_search::seed(&storage, &p5_objects, &p5_object_path).await;
    let platform_id = storage.identity().platform_id;
    drop(scheduler);
    drop(storage);

    for invalid_label in ["", "line\nbreak"] {
        assert_eq!(
            backup_create(&source_loaded, invalid_label)
                .await
                .expect_err("invalid snapshot label")
                .code(),
            ErrorCode::SnapshotInvalid
        );
    }
    let oversized_label = "x".repeat(129);
    assert_eq!(
        backup_create(&source_loaded, &oversized_label)
            .await
            .expect_err("oversized snapshot label")
            .code(),
        ErrorCode::SnapshotInvalid
    );

    let mut no_snapshot_headroom = source_loaded.clone();
    no_snapshot_headroom.config.storage.free_space_hard_bytes = u64::MAX;
    assert_eq!(
        backup_create(&no_snapshot_headroom, "no-headroom")
            .await
            .expect_err("snapshot reserve must be enforced")
            .code(),
        ErrorCode::StoragePressure
    );

    let first = backup_create(&source_loaded, "nightly")
        .await
        .expect("snapshot");
    assert_eq!(first.platform_id, platform_id.to_string());
    assert!(first.files >= 3);
    assert!(
        backup_inspect(&source_loaded, &first.snapshot_id, true)
            .await
            .expect("inspect")
            .verified
    );
    let second = backup_create(&source_loaded, "protected")
        .await
        .expect("second snapshot");

    let credentials = resolve_s3_credentials(&source_loaded.config.s3).expect("S3 credentials");
    let snapshot_client = S3ArtifactClient::connect(
        &source_loaded.config.s3,
        &credentials,
        source_loaded.config.hardening.max_snapshot_file_bytes,
    )
    .expect("snapshot client");
    let snapshot_objects = SnapshotObjectStore::new(snapshot_client, platform_id);
    let manifest_key = snapshot_objects
        .manifest_key(&first.snapshot_id)
        .expect("manifest key");
    let original_manifest_bytes = snapshot_objects
        .get_manifest(
            &first.snapshot_id,
            source_loaded.config.hardening.max_snapshot_manifest_bytes,
        )
        .await
        .expect("manifest bytes");
    let original_manifest: PlatformSnapshotManifestV1 =
        serde_json::from_slice(&original_manifest_bytes).expect("manifest JSON");
    assert!(original_manifest.files.iter().any(|file| {
        file.role == open_compute_core::SnapshotFileRole::VectorizeSqlite
            && file.logical_id == p5_fixture.vectorize_id.to_string()
    }));
    assert!(original_manifest.files.iter().any(|file| {
        file.role == open_compute_core::SnapshotFileRole::AiSearchSqlite
            && file.logical_id == p5_fixture.ai_search_id.to_string()
    }));
    assert!(
        original_manifest
            .immutable_references
            .iter()
            .any(|reference| {
                reference.role == "ai_search_object"
                    && reference.object_key == p5_fixture.object_key
                    && reference.sha256 == hex::encode(p5_fixture.object.sha256)
                    && reference.size == p5_fixture.object.size
            })
    );
    let recovery_key = inspect_master_key(&source_loaded.config.storage).expect("recovery key");
    staged_validation::reject_invalid_staging(
        &snapshot_objects,
        &original_manifest,
        &root,
        recovery_key.fingerprint(),
    )
    .await;

    mock.put_raw(&manifest_key, b"{".to_vec());
    assert_eq!(
        backup_inspect(&source_loaded, &first.snapshot_id, false)
            .await
            .expect_err("malformed manifest")
            .code(),
        ErrorCode::SnapshotInvalid
    );
    mock.put_raw(&manifest_key, original_manifest_bytes.clone());

    let mut wrong_identity = original_manifest.clone();
    wrong_identity.snapshot_id = second.snapshot_id.clone();
    mock.put_raw(
        &manifest_key,
        serde_json::to_vec(&wrong_identity).expect("wrong identity manifest"),
    );
    assert_eq!(
        backup_inspect(&source_loaded, &first.snapshot_id, false)
            .await
            .expect_err("manifest identity mismatch")
            .code(),
        ErrorCode::SnapshotInvalid
    );
    mock.put_raw(&manifest_key, original_manifest_bytes.clone());

    let mut wrong_layout = original_manifest.clone();
    wrong_layout.files[0].object_key = format!(
        "{}999999.bin",
        snapshot_objects
            .object_prefix(&first.snapshot_id)
            .expect("object prefix")
    );
    mock.put_raw(
        &manifest_key,
        serde_json::to_vec(&wrong_layout).expect("wrong layout manifest"),
    );
    assert_eq!(
        backup_inspect(&source_loaded, &first.snapshot_id, false)
            .await
            .expect_err("manifest object layout mismatch")
            .code(),
        ErrorCode::SnapshotInvalid
    );
    mock.put_raw(&manifest_key, original_manifest_bytes.clone());

    let mut wrong_release = original_manifest.clone();
    wrong_release.source_release.platform_version = "0.1.0-incompatible".to_owned();
    sign_snapshot_manifest(&mut wrong_release, &recovery_key).expect("sign wrong release fixture");
    mock.put_raw(
        &manifest_key,
        serde_json::to_vec(&wrong_release).expect("wrong release manifest"),
    );
    let mut wrong_release_target = source_loaded.clone();
    wrong_release_target.config.storage.data_dir = root.join("wrong-release-target");
    assert_eq!(
        backup_restore(&wrong_release_target, &first.snapshot_id)
            .await
            .expect_err("wrong snapshot release")
            .code(),
        ErrorCode::ReleaseUnsupported
    );
    mock.put_raw(&manifest_key, original_manifest_bytes.clone());

    let first_object = &original_manifest.files[0];
    let saved_object = root.join("saved-snapshot-object.bin");
    snapshot_objects
        .download_file(
            &first_object.object_key,
            &saved_object,
            &first_object.sha256,
            first_object.size,
        )
        .await
        .expect("save snapshot object");
    mock.corrupt_body(&first_object.object_key);
    assert_eq!(
        backup_inspect(&source_loaded, &first.snapshot_id, true)
            .await
            .expect_err("corrupt snapshot object")
            .code(),
        ErrorCode::SnapshotInvalid
    );
    mock.put_raw(
        &first_object.object_key,
        fs::read(saved_object).expect("saved snapshot object"),
    );

    p5_objects
        .delete_exact(&p5_fixture.object, &p5_fixture.object_key)
        .await
        .expect("remove AI Search object fixture");
    assert!(
        backup_inspect(&source_loaded, &first.snapshot_id, true)
            .await
            .is_err(),
        "snapshot verification must fail closed when an AI Search object is missing"
    );
    let mut missing_p5_object_restore = source_loaded.clone();
    missing_p5_object_restore.config.storage.data_dir = root.join("missing-p5-object-restore");
    assert!(
        backup_restore(&missing_p5_object_restore, &first.snapshot_id)
            .await
            .is_err(),
        "restore must fail closed before publication when an AI Search object is missing"
    );
    assert!(!missing_p5_object_restore.config.storage.data_dir.exists());
    p5_objects
        .put_file(&p5_fixture.object, &p5_object_path)
        .await
        .expect("restore AI Search object fixture");

    let cli_created = run_cli_json(
        &source_config,
        &["backup", "create", "--name", "cli-snapshot", "--json"],
    )
    .await;
    let cli_snapshot_id = cli_created["snapshot_id"]
        .as_str()
        .expect("CLI snapshot ID")
        .to_owned();
    let cli_list = run_cli_json(&source_config, &["backup", "list", "--json"]).await;
    assert_eq!(cli_list.as_array().expect("CLI snapshot list").len(), 3);
    assert_eq!(
        run_cli_human(&source_config, &["backup", "list"]).await,
        "SNAPSHOTS_OK 3\n"
    );
    let cli_inspect = run_cli_json(
        &source_config,
        &[
            "backup",
            "inspect",
            "--snapshot",
            &cli_snapshot_id,
            "--verify",
            "--json",
        ],
    )
    .await;
    assert_eq!(cli_inspect["verified"], true);
    let cli_plan = run_cli_json(
        &source_config,
        &[
            "backup",
            "retention-plan",
            "--keep-last",
            "1",
            "--keep-label",
            "protected",
            "--json",
        ],
    )
    .await;
    assert!(cli_plan["delete"].as_array().is_some());
    let cleanup = run_cli_json(&source_config, &["backup", "cleanup-incomplete", "--json"]).await;
    assert_eq!(cleanup["schema_version"], 1);
    let capabilities = run_cli_json(&source_config, &["capabilities", "--json"]).await;
    assert_eq!(capabilities["schema_version"], 1);
    let capabilities_human = run_cli_human(&source_config, &["capabilities"]).await;
    assert!(capabilities_human.starts_with("CAPABILITIES V1\nrelease=0.1.0 workerd="));
    assert!(capabilities_human.contains("durable_objects SupportedWithDeviation members=115"));
    let deleted = run_cli_json(
        &source_config,
        &["backup", "delete", "--snapshot", &cli_snapshot_id, "--json"],
    )
    .await;
    assert_eq!(deleted["snapshot_id"], cli_snapshot_id);
    assert_eq!(backup_list(&source_loaded).await.expect("list").len(), 2);
    let plan = backup_retention_plan(&source_loaded, 1, None, vec!["protected".to_owned()])
        .await
        .expect("retention plan");
    assert_eq!(plan.keep.len(), 1);
    assert_eq!(plan.delete.len(), 1);
    assert_eq!(plan.delete[0].snapshot_id, first.snapshot_id);
    let age_plan = backup_retention_plan(
        &source_loaded,
        0,
        Some(u64::MAX / 1_000),
        vec!["protected".to_owned(), "protected".to_owned()],
    )
    .await
    .expect("age retention plan");
    assert_eq!(age_plan.keep.len(), 2);
    assert!(age_plan.delete.is_empty());
    assert_eq!(age_plan.keep_labels, vec!["protected"]);
    for invalid_plan in [
        backup_retention_plan(&source_loaded, 10_001, None, Vec::new()).await,
        backup_retention_plan(&source_loaded, 0, Some(0), Vec::new()).await,
        backup_retention_plan(&source_loaded, 0, None, vec![String::new()]).await,
        backup_retention_plan(&source_loaded, 0, Some(u64::MAX), Vec::new()).await,
    ] {
        assert_eq!(
            invalid_plan.expect_err("invalid retention policy").code(),
            ErrorCode::SnapshotInvalid
        );
    }

    let target_data = root.join("restored-data");
    let wrong_key = root.join("wrong-recovery-master.key");
    let encoded = base64::engine::general_purpose::STANDARD.encode([9_u8; 32]);
    write_mode(&wrong_key, encoded.as_bytes(), 0o600);
    let wrong_config = write_config(&ConfigInputs {
        root: &root,
        name: "wrong-key",
        data_dir: &target_data,
        master_key: &wrong_key,
        access_key: &access_key,
        secret_key: &secret_key,
        endpoint: &mock.endpoint,
        prefix: "system/",
    });
    let wrong_loaded = load_file_only_platform_config(&wrong_config);
    assert!(
        backup_restore(&wrong_loaded, &first.snapshot_id)
            .await
            .is_err()
    );
    assert!(!target_data.exists());

    let mut wrong_source_key = source_loaded.clone();
    wrong_source_key.config.storage.master_key_file = wrong_key.clone();
    assert_eq!(
        backup_create(&wrong_source_key, "wrong-key")
            .await
            .expect_err("snapshot must bind the source recovery key")
            .code(),
        ErrorCode::MasterKeyMismatch
    );

    let restore_config = write_config(&ConfigInputs {
        root: &root,
        name: "restore",
        data_dir: &target_data,
        master_key: &master_key,
        access_key: &access_key,
        secret_key: &secret_key,
        endpoint: &mock.endpoint,
        prefix: "system/",
    });
    let restore_loaded = load_file_only_platform_config(&restore_config);
    assert!(
        backup_inspect(&restore_loaded, &first.snapshot_id, true)
            .await
            .expect("fresh-host snapshot discovery")
            .verified
    );

    let key_inside_target = root.join("key-inside-target");
    let mut key_inside_loaded = restore_loaded.clone();
    key_inside_loaded.config.storage.data_dir = key_inside_target.clone();
    key_inside_loaded.config.storage.master_key_file = key_inside_target.join("master.key");
    assert_eq!(
        backup_restore(&key_inside_loaded, &first.snapshot_id)
            .await
            .expect_err("restore key inside target must be rejected")
            .code(),
        ErrorCode::RestoreInvalid
    );

    let mut policy_mismatch = restore_loaded.clone();
    policy_mismatch.config.storage.data_dir = root.join("policy-mismatch-target");
    policy_mismatch.config.kv.namespace_quota_bytes *= 2;
    assert_eq!(
        backup_restore(&policy_mismatch, &first.snapshot_id)
            .await
            .expect_err("restore policy drift must be rejected")
            .code(),
        ErrorCode::ReleaseUnsupported
    );

    let mut missing_restore_parent = restore_loaded.clone();
    missing_restore_parent.config.storage.data_dir = root.join("missing-parent/restore-target");
    assert_eq!(
        backup_restore(&missing_restore_parent, &first.snapshot_id)
            .await
            .expect_err("restore parent space must be measurable")
            .code(),
        ErrorCode::StoragePressure
    );

    let restored = backup_restore(&restore_loaded, &first.snapshot_id)
        .await
        .expect("fresh-host restore");
    assert_eq!(restored.platform_id, platform_id.to_string());
    assert_eq!(
        fs::read(target_data.join("do/workerd/sentinel.bin")).expect("restored DO sentinel"),
        b"durable-object-sentinel"
    );
    assert_eq!(
        inspect_control_db(&target_data.join("control.sqlite"), 5_000)
            .expect("restored control")
            .1
            .platform_id,
        platform_id
    );
    let restored_scheduler = inspect_scheduler_db(&target_data.join("scheduler.sqlite"), 5_000, 1)
        .expect("restored scheduler");
    assert!(restored_scheduler.journal_mode.eq_ignore_ascii_case("wal"));
    assert_eq!(restored_scheduler.synchronous, 2);
    {
        let restored_storage =
            PlatformStorage::bootstrap(&restore_loaded.config.storage, &SystemClock)
                .expect("bootstrap restored Queue authority");
        p5_search::assert_restored(&restored_storage, &p5_fixture);
        let restored_queue = QueueRepository::new(restored_storage.db())
            .get(account_id, snapshot_queue)
            .expect("restored Queue catalog");
        let restored_scheduler =
            SchedulerStore::open(&restored_storage.data_dir().scheduler_db_path(), 5_000, 1)
                .expect("open restored Queue scheduler");
        assert_eq!(
            restored_scheduler
                .recover_expired_queue_batches(61_000, 250, 10)
                .expect("recover restored Queue lease"),
            1
        );
        let stale_queue = restored_scheduler
            .complete_queue_batch(
                &claimed_queue,
                &[open_compute_storage::QueueCompletionDecision {
                    message_id: claimed_queue.messages[0].id,
                    action: open_compute_storage::QueueCompletionAction::Ack,
                }],
                61_001,
            )
            .expect("old Queue completion is classified");
        assert!(stale_queue.stale);
        assert_eq!(
            restored_scheduler
                .recover_expired_cron_runs(61_000, 250, 10)
                .expect("recover restored Cron lease"),
            1
        );
        assert_eq!(
            restored_scheduler
                .complete_cron_run(
                    &claimed_cron,
                    open_compute_storage::CronCompletion::Success,
                    61_001,
                    2,
                )
                .expect("old Cron completion is classified"),
            open_compute_storage::CronCompletionResult::Stale
        );
        let metrics = restored_scheduler
            .queue_metrics(
                snapshot_queue,
                restored_queue.lifecycle_generation,
                restored_queue.config_generation,
            )
            .expect("restored Queue metrics");
        assert_eq!(metrics.backlog_count, 1);
        assert_eq!(metrics.backlog_bytes, 19);
        assert_eq!(metrics.oldest_message_timestamp_ms, Some(1_001));
        let cron = restored_scheduler
            .inspect_cron_runtime(activation_id, 1, 61_001)
            .expect("restored Cron authority");
        assert!(cron.projection_exists);
        assert_eq!(cron.ready_runs, 1);
    }
    p5_objects
        .verify(&p5_fixture.object, &p5_fixture.object_key)
        .await
        .expect("restored snapshot retains AI Search external object");
    assert!(
        backup_attest_restore_smoke(&restore_loaded, &first.snapshot_id, false)
            .await
            .is_err()
    );
    assert!(
        backup_attest_restore_smoke(&restore_loaded, &second.snapshot_id, true)
            .await
            .is_err()
    );
    let receipt_path = target_data.join("operations/last-restore.json");
    let original_receipt = fs::read(&receipt_path).expect("restore receipt");
    let mut tampered: serde_json::Value =
        serde_json::from_slice(&original_receipt).expect("receipt JSON");
    tampered["manifest_mac"] = serde_json::Value::String("tampered".to_owned());
    fs::write(
        &receipt_path,
        serde_json::to_vec(&tampered).expect("tampered receipt JSON"),
    )
    .expect("tamper receipt");
    assert!(
        backup_attest_restore_smoke(&restore_loaded, &first.snapshot_id, true)
            .await
            .is_err()
    );
    fs::write(&receipt_path, original_receipt).expect("restore authentic receipt");
    let smoke = backup_attest_restore_smoke(&restore_loaded, &first.snapshot_id, true)
        .await
        .expect("attest restore smoke");
    assert!(smoke.smoke_verified);
    let repeated = backup_attest_restore_smoke(&restore_loaded, &first.snapshot_id, true)
        .await
        .expect("idempotent restore smoke attestation");
    assert_eq!(repeated.attested_at_ms, smoke.attested_at_ms);

    let cli_target = root.join("cli-restored-data");
    let cli_restore_config = write_config(&ConfigInputs {
        root: &root,
        name: "cli-restore",
        data_dir: &cli_target,
        master_key: &master_key,
        access_key: &access_key,
        secret_key: &secret_key,
        endpoint: &mock.endpoint,
        prefix: "system/",
    });
    let cli_restore = run_cli_json(
        &cli_restore_config,
        &[
            "backup",
            "restore",
            "--snapshot",
            &first.snapshot_id,
            "--json",
        ],
    )
    .await;
    assert_eq!(cli_restore["snapshot_id"], first.snapshot_id);
    let cli_attestation = run_cli_json(
        &cli_restore_config,
        &[
            "backup",
            "attest-restore-smoke",
            "--snapshot",
            &first.snapshot_id,
            "--passed",
            "--json",
        ],
    )
    .await;
    assert_eq!(cli_attestation["smoke_verified"], true);
    let support_path = root.join("cli-support.tar");
    let cli_support = run_cli_json(
        &cli_restore_config,
        &[
            "support-bundle",
            "--output",
            support_path.to_str().expect("support path"),
            "--json",
        ],
    )
    .await;
    assert_eq!(
        cli_support["output"],
        support_path.to_string_lossy().as_ref()
    );

    let cleanup_target = root.join("failed-restore-data");
    let cleanup_config = write_config(&ConfigInputs {
        root: &root,
        name: "cleanup-restore",
        data_dir: &cleanup_target,
        master_key: &master_key,
        access_key: &access_key,
        secret_key: &secret_key,
        endpoint: &mock.endpoint,
        prefix: "system/",
    });
    let failed_restore = RestoreTarget::acquire(&cleanup_target).expect("failed restore target");
    let staging_name = failed_restore
        .staging_root()
        .file_name()
        .and_then(|value| value.to_str())
        .expect("restore staging name");
    let staging_id = staging_name
        .strip_prefix(".failed-restore-data.restore-")
        .expect("restore staging ID")
        .to_owned();
    let retained = failed_restore
        .destination_for("do/workerd/retained.bin")
        .expect("retained restore object");
    write_mode(&retained, b"retained-after-failure", 0o600);
    drop(failed_restore);
    let cleanup_restore = run_cli_json(
        &cleanup_config,
        &[
            "backup",
            "cleanup-restore",
            "--staging",
            &staging_id,
            "--json",
        ],
    )
    .await;
    assert_eq!(cleanup_restore["staging_id"], staging_id);
    assert_eq!(cleanup_restore["files"], 1);

    backup_delete(&source_loaded, &first.snapshot_id)
        .await
        .expect("exact delete");
    assert!(
        backup_inspect(&source_loaded, &first.snapshot_id, false)
            .await
            .is_err()
    );
    assert!(
        backup_inspect(&source_loaded, &second.snapshot_id, true)
            .await
            .is_ok()
    );
}
