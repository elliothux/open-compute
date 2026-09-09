//! Service crate tests.

use crate::auth::{bearer_matches, resolve_admin_auth};
use crate::cli::{
    BackupCommand, Cli, Command, ConfigCommand, SchedulerCommand, execute, load_checked, parse_from,
};
use crate::config_load::{MAX_CONFIG_BYTES, load_platform_config, load_platform_config_from};
use crate::doctor::{CheckStatus, DoctorMode, doctor_report};
use crate::exit::{ExitClass, emit_failure, exit_class_for};
use crate::health::{HealthCoordinator, map_supervisor};
use crate::http::{self, HttpState, REQUEST_ID_HEADER};
use crate::metrics::{
    AlarmMutation, AlarmOutcome, AlarmRepairSource, D1Lifecycle, D1LifecycleGuard, D1Operation,
    DoFacetReloadReason, DoOperation, DoReconcileState, KvGauge, KvGaugeGuard, KvLifecycle,
    KvLifecycleGuard, KvMaintenance, KvOperation, KvStagingGauge, MetricsRegistry, ObjectOp,
    ObjectResult, QueueMetricOperation, QueueReconcileOperation, R2Operation, R2ProviderError,
    R2StreamDirection, R2StreamGuard, REQUIRED_SERIES, ResourceOperation, RestartReason,
    SchedulerClaimOutcome, ServiceMetricOperation, SqliteOp, StartResult, StartStage,
    WebSocketCloseReason,
};
use crate::run::{
    FailAfter, RunOptions, gc_worker_artifacts, join_listener, join_runtime_source, join_scheduler,
    listener_plan, run_kv_maintenance, run_platform, run_platform_with,
    update_local_object_storage_health,
};
use crate::runtime_bridge::WorkerdTransport;
use crate::scheduler::SchedulerService;
use crate::workers_http::WorkerApiState;
use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::routing::post;
use clap::CommandFactory;
use open_compute_core::config::SecretReference;
use open_compute_core::{
    ComponentName, ComponentState, ErrorCode, MetricsConfig, PlatformStatus, ReadinessReason,
    SchedulerConfig, SchedulerKind, SchedulerPoolState, SecretString,
};
use open_compute_runtime::GenerationAuthRegistry;
use open_compute_runtime::supervisor::{SupervisorSnapshot, SupervisorState};
use open_compute_storage::{DataDir, SchedulerStore, SchedulerSummary, inspect_scheduler_db};
use open_compute_workers::{
    BundleLimits, CanonicalBundle, CreateVersionOutcome, CreateVersionRequest, ModuleInput,
    ModuleType, ProductPromotionCoordinator, ProductPromotionRequest, QueueConsumerInput,
    RuntimeValidator, ValidationCandidate, VersionController, VersionPins,
};
use sha2::Digest;
use std::fs::{self, File};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command as Proc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tower::ServiceExt;

#[derive(Clone)]
struct FakeCustomEventResponses {
    queue: Arc<Mutex<serde_json::Value>>,
    cron: Arc<Mutex<serde_json::Value>>,
}

async fn fake_queue_custom_event(
    State(responses): State<FakeCustomEventResponses>,
) -> Json<serde_json::Value> {
    Json(responses.queue.lock().unwrap().clone())
}

async fn fake_cron_custom_event(
    State(responses): State<FakeCustomEventResponses>,
) -> Json<serde_json::Value> {
    Json(responses.cron.lock().unwrap().clone())
}

fn write_config(dir: &Path, extra: &str) -> PathBuf {
    let data = dir.join("data");
    let key = dir.join("master.key");
    let admin_auth = dir.join("admin-auth");
    let deployer_auth = dir.join("deployer-auth");
    let read_only_auth = dir.join("read-only-auth");
    fs::create_dir_all(&data).unwrap();
    write_mode(&admin_auth, "test-admin-secret", 0o600);
    write_mode(&deployer_auth, "test-deployer-secret", 0o600);
    write_mode(&read_only_auth, "test-read-only-secret", 0o600);
    let object_storage =
        if extra.contains("backend = \"s3\"") || extra.contains("backend = \"local\"") {
            String::new()
        } else {
            format!(
                r#"
[storage]
backend = "local"
path = "{}"
prefix = "system/"
"#,
                dir.join("objects").display()
            )
        };
    let toml = format!(
        r#"
[server]
public_bind = "127.0.0.1:0"
admin_bind = "127.0.0.1:0"
admin_auth = {{ file = "{admin_auth}" }}
deployer_auth = {{ file = "{deployer_auth}" }}
read_only_auth = {{ file = "{read_only_auth}" }}

[data]
path = "{data_dir}"
master_key_file = "{master_key_file}"
{object_storage}
[cache]
max_bytes = 1048576
high_watermark_ratio = 0.9
low_watermark_ratio = 0.8
max_artifact_bytes = 65536

[metrics]
enabled = true
max_label_value_bytes = 64
max_series = 1024
{extra}
"#,
        data_dir = data.display(),
        master_key_file = key.display(),
        admin_auth = admin_auth.display(),
        deployer_auth = deployer_auth.display(),
        read_only_auth = read_only_auth.display(),
    );
    let path = dir.join("config.toml");
    fs::write(&path, toml).unwrap();
    path
}

mod package_and_cli_shape;

mod cli_execute_covers_success_failure_and_output_modes;

mod bound_local_authority_mismatch_does_not_initialize_a_new_root;

mod config_path_boundary_helpers_reject_ambiguous_roots;

mod listener_plan_and_task_join_errors_are_stable;

mod config_path_rejections_do_not_echo_secrets;

mod config_check_has_no_side_effects;

fn snapshot(root: &Path) -> Vec<(String, u64, Option<SystemTime>)> {
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, u64, Option<SystemTime>)>) {
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    for ent in rd.flatten() {
        let p = ent.path();
        let rel = p.strip_prefix(root).unwrap().to_string_lossy().into_owned();
        let meta = fs::symlink_metadata(&p).unwrap();
        out.push((rel, meta.len(), meta.modified().ok()));
        if meta.file_type().is_dir() {
            walk(root, &p, out);
        }
    }
}

mod component_and_supervisor_mapping;

mod exit_classes_and_failure_output_are_stable;

mod coalesced_watch_transitions_and_draining_terminal;

fn runtime_state(coord: &HealthCoordinator) -> ComponentState {
    coord
        .snapshot()
        .components
        .iter()
        .find(|c| c.name == ComponentName::Runtime)
        .unwrap()
        .state
}

trait SnapInit {
    fn initial_for_test(now: SystemTime) -> Self;
}

impl SnapInit for SupervisorSnapshot {
    fn initial_for_test(now: SystemTime) -> Self {
        SupervisorSnapshot {
            state: SupervisorState::Stopped,
            reason: ReadinessReason::Starting,
            last_transition_at: now,
            attempt: 0,
            last_exit: None,
            next_retry_at: None,
            pid: None,
            pgid: None,
            binary_digest: "ab".into(),
            config_digest: String::new(),
            startup_id: None,
            token_fingerprint: None,
            listen_port: None,
        }
    }
}

fn test_state(health: HealthCoordinator, secret: Option<&str>) -> HttpState {
    let metrics = Arc::new(
        MetricsRegistry::new(&MetricsConfig::default(), "0.1.0", "workerd 2026-08-26").unwrap(),
    );
    HttpState::for_test(health, metrics, true, secret.map(SecretString::new))
}

mod local_object_storage_health_tracks_current_filesystem_capacity;

mod liveness_ready_status_and_bounds;

mod admin_auth_and_separate_routers;

mod metrics_fixed_and_limits;

mod queue_metrics_cover_fixed_operations_outcomes_and_backlog;

mod observe_supervisor_counts_one_logical_restart;

mod metrics_workerd_version_and_preflight_counters;

mod default_doctor_does_not_mutate;

fn check<'a>(
    report: &'a crate::doctor::DoctorReport,
    name: &str,
) -> &'a crate::doctor::DoctorCheck {
    report.checks.iter().find(|c| c.name == name).expect(name)
}

fn encode_master_key(bytes: &[u8; 32]) -> String {
    use base64::Engine;
    format!(
        "ocmk1:{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    )
}

fn write_mode(path: &Path, contents: &str, mode: u32) {
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .open(path)
        .unwrap();
    f.write_all(contents.as_bytes()).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

fn admin_reference(file: Option<&Path>) -> SecretReference {
    SecretReference {
        env: None,
        file: file.map(Path::to_path_buf),
    }
}

mod admin_auth_files_and_bearer_matching_fail_closed;

mod admin_auth_environment_modes_are_covered_in_isolated_processes;

mod metrics_mutation_surfaces_and_label_bounds_are_complete;

fn content_snapshot(root: &Path) -> Vec<(String, u64, Option<SystemTime>, String)> {
    let mut out = Vec::new();
    fn rec(root: &Path, dir: &Path, out: &mut Vec<(String, u64, Option<SystemTime>, String)>) {
        let Ok(rd) = fs::read_dir(dir) else {
            return;
        };
        for ent in rd.flatten() {
            let p = ent.path();
            let rel = p.strip_prefix(root).unwrap().to_string_lossy().into_owned();
            let meta = fs::symlink_metadata(&p).unwrap();
            let digest = if meta.file_type().is_file() {
                hex::encode(sha2::Sha256::digest(fs::read(&p).unwrap_or_default()))
            } else {
                String::new()
            };
            out.push((rel, meta.len(), meta.modified().ok(), digest));
            if meta.file_type().is_dir() {
                rec(root, &p, out);
            }
        }
    }
    rec(root, root, &mut out);
    out.sort();
    out
}

const FIXTURE_S3_ACCESS_KEY_ID: &str = "AKIAEXAMPLEKEYID01";
const FIXTURE_S3_SECRET_ACCESS_KEY: &str = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";

fn clear_fixture_s3_env_defaults(config: &mut open_compute_core::PlatformConfig) {
    if let Some(s3) = config.object_storage.as_s3_mut() {
        s3.normalize_implicit_env_defaults();
    }
}

fn load_fixture_platform_config(path: &Path) -> crate::config_load::LoadedConfig {
    let mut loaded = load_platform_config(path).unwrap();
    clear_fixture_s3_env_defaults(&mut loaded.config);
    loaded
}

fn resolve_fixture_s3_credentials(
    config: &open_compute_core::S3Config,
) -> open_compute_artifacts::S3Credentials {
    open_compute_artifacts::resolve_s3_credentials(config).unwrap()
}

async fn initialized_doctor_fixture() -> (TempDir, PathBuf, open_compute_artifacts::MockS3) {
    let dir = TempDir::new().unwrap();
    let mock = open_compute_artifacts::MockS3::spawn("open-compute").await;
    let ak = dir.path().join("ak");
    let sk = dir.path().join("sk");
    write_mode(&ak, FIXTURE_S3_ACCESS_KEY_ID, 0o600);
    write_mode(&sk, FIXTURE_S3_SECRET_ACCESS_KEY, 0o600);
    let extra = format!(
        r#"
[storage]
backend = "s3"
endpoint = "{endpoint}"
region = "us-east-1"
bucket = "open-compute"
force_path_style = true
access_key_id_file = "{ak}"
secret_access_key_file = "{sk}"
verify_tls = true
prefix = "system/"
max_retries = 1
retry_backoff_ms = 10
connect_timeout_ms = 500
request_timeout_ms = 2000
"#,
        endpoint = mock.endpoint,
        ak = ak.display(),
        sk = sk.display(),
    );
    let path = write_config(dir.path(), &extra);
    let loaded = load_fixture_platform_config(&path);
    let storage = open_compute_storage::PlatformStorage::bootstrap(
        &loaded.config.data,
        &open_compute_core::SystemClock,
    )
    .unwrap();
    let connected =
        crate::object_storage::connect_object_backend(&loaded.config, storage.identity()).unwrap();
    storage
        .bind_object_authority(
            connected.backend.kind(),
            &connected.backend.authority_sha256(),
        )
        .unwrap();
    open_compute_artifacts::preflight_object_storage(
        &connected.backend,
        storage.identity().platform_id,
        open_compute_core::StartupId::generate(),
    )
    .await
    .unwrap();
    storage
        .data_dir()
        .prepare_durable_object_storage(
            &storage.identity().platform_id.to_string(),
            &open_compute_runtime::embedded_runtime_lock()
                .unwrap()
                .0
                .expected_version_output,
        )
        .unwrap();
    mock.clear_recorded();
    (dir, path, mock)
}

mod p1_startup_receipts_health_and_inventory_metrics_cover_real_authority;

mod p1_capability_release_support_bundle_and_metrics_contract_is_bounded;

pub(crate) async fn initialized_worker_http_fixture() -> (
    TempDir,
    open_compute_artifacts::MockS3,
    HttpState,
    open_compute_core::AccountId,
    Arc<open_compute_storage::PlatformStorage>,
) {
    let (dir, path, mock) = initialized_doctor_fixture().await;
    let loaded = load_fixture_platform_config(&path);
    let storage = Arc::new(
        open_compute_storage::PlatformStorage::bootstrap(
            &loaded.config.data,
            &open_compute_core::SystemClock,
        )
        .unwrap(),
    );
    let account = storage.identity().default_account_id;
    let s3 = loaded.config.object_storage.as_s3().expect("S3 config");
    let credentials = resolve_fixture_s3_credentials(s3);
    let client = open_compute_artifacts::ObjectBackend::connect_s3(
        s3,
        &credentials,
        loaded.config.cache.max_artifact_bytes,
    )
    .unwrap();
    let transport =
        WorkerdTransport::new(GenerationAuthRegistry::new(), Arc::new(Mutex::new(None)));
    let observability_store = Arc::new(
        open_compute_storage::ObservabilityStore::open(
            &storage.data_dir().ensure_observability_db().unwrap(),
            loaded.config.data.sqlite_busy_timeout_ms,
            loaded.config.observability.retention_ms,
            loaded.config.observability.max_database_bytes,
        )
        .unwrap(),
    );
    let observability = crate::observability::ObservabilityService::new(
        storage.clone(),
        Some(observability_store),
        loaded.config.observability.clone(),
        Arc::new(MetricsRegistry::new(&loaded.config.metrics, "test", "workerd").unwrap()),
    );
    let api = WorkerApiState::new(
        storage.clone(),
        open_compute_artifacts::ArtifactStore::new(client),
        transport,
        VersionPins::new(),
        BundleLimits::default(),
        Duration::from_millis(10),
    )
    .with_observability(observability);
    assert!(format!("{api:?}").contains("WorkerApiState"));
    assert_eq!(
        api.pins().count(open_compute_core::VersionId::generate()),
        0
    );
    (
        dir,
        mock,
        test_state(HealthCoordinator::new(), Some("admin-token")).with_worker_api(api),
        account,
        storage,
    )
}

mod v4_account_subdomain_is_a_stable_read_only_unroutable_prerequisite;

mod p7_script_tails_and_empty_telemetry_follow_the_fixed_v4_contract;

mod v4_asset_upload_auth_integrity_and_failed_script_creation_are_closed;

mod p2_3_promotion_is_idempotent_preserves_pause_and_resumes_an_interrupted_update;

mod initialized_basic_doctor_is_read_only_and_head_only;

mod initialized_local_doctor_reports_backend_specific_checks;

mod doctor_reports_key_mismatch_and_env_only_key;

mod doctor_rejects_future_schema_and_sha256_symlink_and_corrupt_cache;

mod fail_after_stages_release_lock_and_ports;

mod full_doctor_uses_embedded_workerd;

mod doctor_skips_db_when_platform_lock_is_held;

mod doctor_reports_limits_space_and_full_prerequisite_failures;

mod full_doctor_reports_object_storage_canary_failure_without_leaking_objects;

mod run_startup_failure_matrix_releases_owned_resources;

mod run_real_workerd_with_separate_admin_listener_and_maintenance_tick;

mod worker_artifact_gc_skips_when_final_reference_snapshot_fails;

mod reused_old_artifact_commit_precedes_gc_reference_snapshot;

mod kv_maintenance_gc_skip_checkpoint_and_corruption_isolation;

mod run_real_workerd_on_merged_listener_serves_status_and_shuts_down;

#[path = "p2_3_route_epoch_tests.rs"]
mod p2_3_route_epoch_tests;

#[path = "p2_3_cron_generation_tests.rs"]
mod p2_3_cron_generation_tests;
