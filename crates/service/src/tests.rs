//! Service crate tests.

use crate::auth::{bearer_matches, resolve_admin_auth};
use crate::cli::{
    BackupCommand, Cli, Command, ConfigCommand, SchedulerCommand, execute, load_checked, parse_from,
};
use crate::config_load::{MAX_CONFIG_BYTES, load_platform_config};
use crate::doctor::{CheckStatus, DoctorMode, doctor_report};
use crate::exit::{ExitClass, emit_failure, exit_class_for, exit_code};
use crate::health::{HealthCoordinator, map_supervisor};
use crate::http::{self, HttpState, REQUEST_ID_HEADER};
use crate::metrics::{
    AlarmMutation, AlarmOutcome, AlarmRepairSource, D1Lifecycle, D1LifecycleGuard, D1Operation,
    DoFacetReloadReason, DoOperation, DoReconcileState, KvGauge, KvGaugeGuard, KvLifecycle,
    KvLifecycleGuard, KvMaintenance, KvOperation, KvStagingGauge, MetricsRegistry,
    QueueMetricOperation, QueueReconcileOperation, R2Operation, R2ProviderError, R2StreamDirection,
    R2StreamGuard, REQUIRED_SERIES, ResourceOperation, RestartReason, S3Op, S3Result,
    SchedulerClaimOutcome, ServiceMetricOperation, SqliteOp, StartResult, StartStage,
    WebSocketCloseReason,
};
use crate::run::{
    FailAfter, RunOptions, gc_worker_artifacts, join_listener, join_runtime_source, listener_plan,
    run_kv_maintenance, run_platform, run_platform_with,
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
    SchedulerConfig, SchedulerKind, SchedulerPoolState, SecretString, SystemSchedulerClock,
};
use open_compute_runtime::GenerationAuthRegistry;
use open_compute_runtime::supervisor::{SupervisorSnapshot, SupervisorState};
use open_compute_storage::{DataDir, SchedulerStore, SchedulerSummary, inspect_scheduler_db};
use open_compute_workers::{
    BundleLimits, CanonicalBundle, CreateDeploymentOutcome, CreateDeploymentRequest,
    DeploymentController, DeploymentPins, ModuleInput, ModuleType, ProductPromotionCoordinator,
    ProductPromotionRequest, QueueConsumerInput, RuntimeValidator, ValidationCandidate,
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
    fs::create_dir_all(&data).unwrap();
    let s3 = if extra.contains("[s3]") {
        String::new()
    } else {
        r#"
[s3]
endpoint = "http://127.0.0.1:9"
region = "auto"
bucket = "open-compute"
force_path_style = true
access_key_id_env = "S3_ACCESS_KEY_ID"
secret_access_key_env = "S3_SECRET_ACCESS_KEY"
prefix = "system/"
"#
        .to_string()
    };
    let toml = format!(
        r#"
[server]
public_bind = "127.0.0.1:0"
admin_bind = "127.0.0.1:0"

[storage]
data_dir = "{}"
master_key_file = "{}"
{s3}
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
        data.display(),
        key.display(),
    );
    let path = dir.join("config.toml");
    fs::write(&path, toml).unwrap();
    path
}

#[test]
fn package_and_cli_shape() {
    assert_eq!(env!("CARGO_PKG_NAME"), "open-compute-service");
    let help = Cli::command().render_help().to_string();
    assert!(help.contains("ocd"));
    assert!(help.contains("run"));
    assert!(help.contains("config"));
    assert!(help.contains("doctor"));
    assert!(help.contains("scheduler"));
    assert!(help.contains("capabilities"));
    assert!(help.contains("backup"));
    assert!(!help.contains("upgrade"));
    assert!(help.contains("support-bundle"));
    let parsed = parse_from(["ocd", "run", "--config", "/tmp/config.toml"]).unwrap();
    assert!(matches!(parsed.command, Command::Run));
    let parsed = parse_from([
        "ocd",
        "config",
        "check",
        "--config",
        "/tmp/config.toml",
        "--json",
    ])
    .unwrap();
    assert!(matches!(
        parsed.command,
        Command::Config {
            command: ConfigCommand::Check { json: true }
        }
    ));
    let parsed = parse_from([
        "ocd",
        "backup",
        "create",
        "--name",
        "before-snapshot",
        "--config",
        "/tmp/config.toml",
        "--json",
    ])
    .unwrap();
    assert!(matches!(
        parsed.command,
        Command::Backup {
            command: BackupCommand::Create { json: true, .. }
        }
    ));
    let parsed = parse_from([
        "ocd",
        "backup",
        "cleanup-restore",
        "--staging",
        "01900000-0000-7000-8000-000000000000",
        "--config",
        "/tmp/config.toml",
        "--json",
    ])
    .unwrap();
    assert!(matches!(
        parsed.command,
        Command::Backup {
            command: BackupCommand::CleanupRestore { json: true, .. }
        }
    ));
    let parsed = parse_from([
        "ocd",
        "backup",
        "attest-restore-smoke",
        "--snapshot",
        "01900000-0000-7000-8000-000000000000",
        "--passed",
        "--config",
        "/tmp/config.toml",
        "--json",
    ])
    .unwrap();
    assert!(matches!(
        parsed.command,
        Command::Backup {
            command: BackupCommand::AttestRestoreSmoke {
                passed: true,
                json: true,
                ..
            }
        }
    ));
    assert!(parse_from(["ocd", "upgrade"]).is_err());
    let parsed = parse_from([
        "ocd",
        "doctor",
        "--config",
        "/tmp/config.toml",
        "--full",
        "--json",
    ])
    .unwrap();
    assert!(matches!(
        parsed.command,
        Command::Doctor {
            full: true,
            json: true
        }
    ));
    let parsed = parse_from([
        "ocd",
        "scheduler",
        "recover-corrupt",
        "--backup-name",
        "scheduler-corrupt-test",
        "--config",
        "/tmp/config.toml",
    ])
    .unwrap();
    assert!(matches!(
        parsed.command,
        Command::Scheduler {
            command: SchedulerCommand::RecoverCorrupt { .. }
        }
    ));
}

#[tokio::test]
async fn cli_execute_covers_success_failure_and_output_modes() {
    let dir = TempDir::new().unwrap();
    let path = write_config(dir.path(), "");
    assert!(load_checked(&path).is_ok());

    for json in [false, true] {
        let mut args = vec![
            "ocd".to_owned(),
            "--config".to_owned(),
            path.display().to_string(),
            "config".to_owned(),
            "check".to_owned(),
        ];
        if json {
            args.push("--json".to_owned());
        }
        let cli = parse_from(args).unwrap();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = execute(cli, &mut stdout, &mut stderr).await;
        assert_eq!(code, std::process::ExitCode::SUCCESS);
        assert!(stderr.is_empty());
        let text = String::from_utf8(stdout).unwrap();
        assert!(text.contains(if json { "config_check" } else { "CONFIG_OK" }));
    }

    let loaded = load_platform_config(&path).unwrap();
    let storage = open_compute_storage::PlatformStorage::bootstrap(
        &loaded.config.storage,
        &open_compute_core::SystemClock,
    )
    .unwrap();
    let scheduler_path = storage.data_dir().ensure_scheduler_db().unwrap();
    fs::write(&scheduler_path, b"corrupt scheduler").unwrap();
    drop(storage);
    let recovery = parse_from([
        "ocd",
        "--config",
        path.to_str().unwrap(),
        "scheduler",
        "recover-corrupt",
        "--backup-name",
        "scheduler-corrupt-cli-test",
    ])
    .unwrap();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    assert_eq!(
        execute(recovery, &mut stdout, &mut stderr).await,
        std::process::ExitCode::SUCCESS
    );
    assert!(stderr.is_empty());
    assert!(
        String::from_utf8(stdout)
            .unwrap()
            .contains("SCHEDULER_RECOVERED")
    );
    assert!(inspect_scheduler_db(&scheduler_path, 100, 10).is_ok());
    assert_eq!(
        fs::read(
            loaded
                .config
                .storage
                .data_dir
                .join("diagnostics/scheduler-recovery/scheduler-corrupt-cli-test/scheduler.sqlite")
        )
        .unwrap(),
        b"corrupt scheduler"
    );

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let missing_config = parse_from(["ocd", "config", "check"]).unwrap();
    let code = execute(missing_config, &mut stdout, &mut stderr).await;
    assert_ne!(code, std::process::ExitCode::SUCCESS);
    assert!(
        String::from_utf8(stderr)
            .unwrap()
            .contains("CONFIG_PATH_INVALID")
    );

    assert!(parse_from(["ocd", "package-release"]).is_err());

    for json in [false, true] {
        let mut args = vec![
            "ocd".to_owned(),
            "--config".to_owned(),
            path.display().to_string(),
            "doctor".to_owned(),
        ];
        if json {
            args.push("--json".to_owned());
        }
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = execute(parse_from(args).unwrap(), &mut stdout, &mut stderr).await;
        assert_eq!(code, std::process::ExitCode::from(ExitClass::Doctor.code()));
        assert!(stderr.is_empty());
        let output = String::from_utf8(stdout).unwrap();
        assert!(output.contains(if json {
            "\"command\":\"doctor\""
        } else {
            "DOCTOR"
        }));
    }

    struct RejectWrites;
    impl Write for RejectWrites {
        fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("rejected"))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let cli = parse_from(["ocd", "--config", path.to_str().unwrap(), "config", "check"]).unwrap();
    let mut stdout = RejectWrites;
    let mut stderr = Vec::new();
    assert_ne!(
        execute(cli, &mut stdout, &mut stderr).await,
        std::process::ExitCode::SUCCESS
    );
    assert!(
        String::from_utf8(stderr)
            .unwrap()
            .contains("CONFIG_INVALID")
    );
}

#[tokio::test]
async fn listener_plan_and_task_join_errors_are_stable() {
    let mut server = open_compute_core::ServerConfig {
        public_bind: "127.0.0.1:8080".to_owned(),
        admin_bind: Some("127.0.0.1:8080".to_owned()),
        ..open_compute_core::ServerConfig::default()
    };
    assert_eq!(listener_plan(&server).unwrap().1, None);
    server.admin_bind = Some("127.0.0.1:8081".to_owned());
    assert_eq!(listener_plan(&server).unwrap().1.unwrap().port(), 8081);
    server.public_bind = "not-an-address".to_owned();
    assert!(listener_plan(&server).is_err());

    assert_eq!(join_listener(Ok(Ok(()))).code(), ErrorCode::ConfigInvalid);
    assert_eq!(
        join_listener(Ok(Err(open_compute_core::PlatformError::new(
            ErrorCode::PathInvalid,
            "listener",
        ))))
        .code(),
        ErrorCode::PathInvalid
    );
    assert_eq!(
        join_runtime_source(Ok(Ok(()))).code(),
        ErrorCode::RuntimeUnavailable
    );
    assert_eq!(
        join_runtime_source(Ok(Err(open_compute_core::PlatformError::new(
            ErrorCode::Internal,
            "runtime source",
        ))))
        .code(),
        ErrorCode::Internal
    );

    let listener_panic = tokio::spawn(async { panic!("listener test panic") })
        .await
        .unwrap_err();
    assert_eq!(
        join_listener(Err(listener_panic)).code(),
        ErrorCode::ConfigInvalid
    );
    let source_panic = tokio::spawn(async { panic!("source test panic") })
        .await
        .unwrap_err();
    assert_eq!(
        join_runtime_source(Err(source_panic)).code(),
        ErrorCode::RuntimeUnavailable
    );
}

#[test]
fn config_path_rejections_do_not_echo_secrets() {
    let dir = TempDir::new().unwrap();
    let rel = parse_from(["ocd", "run", "--config", "relative.toml"]).unwrap();
    let err = load_platform_config(rel.config.as_ref().unwrap()).unwrap_err();
    assert_eq!(err.code(), ErrorCode::ConfigPathInvalid);
    assert!(!err.to_string().contains("AKIA"));

    let dotted = dir.path().join("a/../config.toml");
    // even if absolute with ..
    let abs_dot = PathBuf::from("/tmp/../etc/passwd");
    let err = load_platform_config(&abs_dot).unwrap_err();
    assert_eq!(err.code(), ErrorCode::ConfigPathInvalid);

    let link = dir.path().join("link.toml");
    let target = dir.path().join("real.toml");
    fs::write(&target, "not toml secret=AKIA123").unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let err = load_platform_config(&link).unwrap_err();
    assert_eq!(err.code(), ErrorCode::ConfigPathInvalid);
    assert!(!format!("{err:?}").contains("AKIA"));

    let fifo = dir.path().join("fifo.toml");
    let _ = Proc::new("mkfifo").arg(&fifo).status();
    if fifo.exists() {
        let err = load_platform_config(&fifo).unwrap_err();
        assert_eq!(err.code(), ErrorCode::ConfigPathInvalid);
    }

    let big = dir.path().join("big.toml");
    let mut f = File::create(&big).unwrap();
    let chunk = vec![b'a'; 1024];
    for _ in 0..(MAX_CONFIG_BYTES / 1024 + 2) {
        f.write_all(&chunk).unwrap();
    }
    drop(f);
    let err = load_platform_config(&big).unwrap_err();
    assert_eq!(err.code(), ErrorCode::ConfigPathInvalid);

    let non_utf = dir.path().join("bad.toml");
    fs::write(&non_utf, [0xff, 0xfe]).unwrap();
    let err = load_platform_config(&non_utf).unwrap_err();
    assert_eq!(err.code(), ErrorCode::ConfigParseFailed);

    let unknown = dir.path().join("unknown.toml");
    let path = write_config(dir.path(), "");
    let mut toml = fs::read_to_string(&path).unwrap();
    toml.push_str("\nnope = 1\n");
    fs::write(&unknown, toml).unwrap();
    let err = load_platform_config(&unknown).unwrap_err();
    assert_eq!(err.code(), ErrorCode::ConfigParseFailed);
    let _ = dotted;
}

#[tokio::test]
async fn config_check_has_no_side_effects() {
    let dir = TempDir::new().unwrap();
    let path = write_config(dir.path(), "");
    let before = snapshot(dir.path());
    let loaded = load_platform_config(&path).unwrap();
    MetricsRegistry::validate_limits(&loaded.config.metrics).unwrap();
    let after = snapshot(dir.path());
    assert_eq!(before, after);
}

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

#[test]
fn component_and_supervisor_mapping() {
    let mut status = PlatformStatus::starting();
    for c in &mut status.components {
        c.transition(ComponentState::Healthy, Some(ReadinessReason::Ready))
            .unwrap();
    }
    status.recompute();
    assert_eq!(status.readiness, ReadinessReason::Ready);

    let coord = HealthCoordinator::new();
    let now = SystemTime::UNIX_EPOCH;
    let mut snap = SupervisorSnapshot::initial_for_test(now);
    snap.state = SupervisorState::Starting;
    snap.reason = ReadinessReason::RuntimeStarting;
    coord.apply_supervisor(&snap).unwrap();
    assert_eq!(coord.readiness(), ReadinessReason::Starting);

    snap.state = SupervisorState::Running;
    snap.reason = ReadinessReason::Ready;
    coord.apply_supervisor(&snap).unwrap();
    assert_eq!(
        coord
            .snapshot()
            .components
            .iter()
            .find(|c| c.name == ComponentName::Runtime)
            .unwrap()
            .state,
        ComponentState::Healthy
    );

    snap.state = SupervisorState::BackingOff;
    snap.reason = ReadinessReason::RuntimeRestartBackoff;
    coord.apply_supervisor(&snap).unwrap();
    assert_eq!(coord.readiness(), ReadinessReason::RuntimeRestartBackoff);

    snap.state = SupervisorState::Failed;
    snap.reason = ReadinessReason::RuntimeInvalid;
    coord.apply_supervisor(&snap).unwrap();
    assert_eq!(coord.readiness(), ReadinessReason::RuntimeInvalid);

    snap.state = SupervisorState::Starting;
    snap.reason = ReadinessReason::RuntimeStarting;
    coord.apply_supervisor(&snap).unwrap();

    snap.state = SupervisorState::Draining;
    coord.apply_supervisor(&snap).unwrap();
    assert_eq!(coord.readiness(), ReadinessReason::Draining);

    let stopped = SupervisorSnapshot::initial_for_test(now);
    assert_eq!(
        map_supervisor(&stopped),
        (ComponentState::Starting, ReadinessReason::Starting)
    );
    snap.state = SupervisorState::Failed;
    snap.reason = ReadinessReason::S3Unavailable;
    assert_eq!(
        map_supervisor(&snap),
        (ComponentState::Failed, ReadinessReason::S3Unavailable)
    );

    let degraded = HealthCoordinator::default();
    degraded
        .set_component(
            ComponentName::Runtime,
            ComponentState::Degraded,
            Some(ReadinessReason::RuntimeRestartBackoff),
        )
        .unwrap();
    snap.state = SupervisorState::Starting;
    snap.reason = ReadinessReason::RuntimeStarting;
    degraded.apply_supervisor(&snap).unwrap();
    assert_eq!(runtime_state(&degraded), ComponentState::Starting);
    degraded.begin_drain().unwrap();
    degraded.begin_drain().unwrap();
}

#[test]
fn exit_classes_and_failure_output_are_stable() {
    for code in [
        ErrorCode::ConfigPathInvalid,
        ErrorCode::ConfigParseFailed,
        ErrorCode::AdminAuthRequired,
        ErrorCode::SecretRefInvalid,
        ErrorCode::PathInvalid,
        ErrorCode::S3PrefixInvalid,
        ErrorCode::CacheBoundsInvalid,
        ErrorCode::LimitInvalid,
    ] {
        assert_eq!(exit_class_for(code), ExitClass::Config);
    }
    assert_eq!(exit_class_for(ErrorCode::Internal), ExitClass::Run);
    assert_eq!(exit_code(ExitClass::Cli), std::process::ExitCode::from(2));
    let mut output = Vec::new();
    emit_failure(
        &open_compute_core::PlatformError::new(ErrorCode::Internal, "safe"),
        &mut output,
    )
    .unwrap();
    assert_eq!(output, b"INTERNAL: safe\n");
}

#[test]
fn coalesced_watch_transitions_and_draining_terminal() {
    let now = SystemTime::UNIX_EPOCH;
    let coord = HealthCoordinator::new();
    let mut snap = SupervisorSnapshot::initial_for_test(now);
    snap.state = SupervisorState::Failed;
    snap.reason = ReadinessReason::RuntimeInvalid;
    coord.apply_supervisor(&snap).unwrap();
    snap.state = SupervisorState::Running;
    snap.reason = ReadinessReason::Ready;
    coord.apply_supervisor(&snap).unwrap();
    assert_eq!(
        runtime_state(&coord),
        ComponentState::Healthy,
        "Failed -> Running must bridge through Starting"
    );
    assert_eq!(coord.readiness(), ReadinessReason::Starting); // other components still starting
    snap.state = SupervisorState::Running;
    coord.apply_supervisor(&snap).unwrap();
    assert_eq!(runtime_state(&coord), ComponentState::Healthy);

    let coord = HealthCoordinator::new();
    snap.state = SupervisorState::Running;
    coord.apply_supervisor(&snap).unwrap();
    snap.state = SupervisorState::Starting;
    snap.reason = ReadinessReason::RuntimeStarting;
    coord.apply_supervisor(&snap).unwrap();
    assert_eq!(runtime_state(&coord), ComponentState::Starting);
    snap.state = SupervisorState::Running;
    snap.reason = ReadinessReason::Ready;
    coord.apply_supervisor(&snap).unwrap();
    assert_eq!(runtime_state(&coord), ComponentState::Healthy);

    let coord = HealthCoordinator::new();
    snap.state = SupervisorState::BackingOff;
    snap.reason = ReadinessReason::RuntimeRestartBackoff;
    coord.apply_supervisor(&snap).unwrap();
    snap.state = SupervisorState::Running;
    snap.reason = ReadinessReason::Ready;
    coord.apply_supervisor(&snap).unwrap();
    assert_eq!(runtime_state(&coord), ComponentState::Healthy);

    let coord = HealthCoordinator::new();
    snap.state = SupervisorState::Draining;
    coord.apply_supervisor(&snap).unwrap();
    snap.state = SupervisorState::Running;
    let _ = coord.apply_supervisor(&snap);
    assert_eq!(runtime_state(&coord), ComponentState::Draining);
    assert_eq!(coord.readiness(), ReadinessReason::Draining);

    let (st, reason) = map_supervisor(&SupervisorSnapshot {
        state: SupervisorState::Stopping,
        reason: ReadinessReason::Draining,
        last_transition_at: now,
        attempt: 1,
        last_exit: None,
        next_retry_at: None,
        pid: Some(1),
        pgid: Some(1),
        binary_digest: "x".into(),
        config_digest: "y".into(),
        startup_id: None,
        token_fingerprint: None,
        listen_port: Some(1),
    });
    assert_eq!(st, ComponentState::Draining);
    assert_eq!(reason, ReadinessReason::Draining);
}

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

#[tokio::test]
async fn liveness_ready_status_and_bounds() {
    let health = HealthCoordinator::new();
    let state = test_state(health.clone(), None);
    let debug = format!("{state:?}");
    assert!(debug.contains("HttpState"));
    assert!(debug.contains("worker_api: false"));
    let app = http::merged_router(state.clone());
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health/live")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(res.headers().get(REQUEST_ID_HEADER).is_some());

    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = axum::body::to_bytes(res.into_body(), 4096).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["code"], "STARTING");

    health
        .set_component(
            ComponentName::Process,
            ComponentState::Healthy,
            Some(ReadinessReason::Ready),
        )
        .unwrap();
    for name in [
        ComponentName::DataDir,
        ComponentName::ControlDb,
        ComponentName::MasterKey,
        ComponentName::S3,
        ComponentName::Cache,
        ComponentName::Runtime,
        ComponentName::Scheduler,
        ComponentName::Operations,
    ] {
        health
            .set_component(name, ComponentState::Healthy, Some(ReadinessReason::Ready))
            .unwrap();
    }
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    health.begin_drain().unwrap();
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health/live")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);

    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/health/live")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::METHOD_NOT_ALLOWED);

    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/workers")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);

    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health/live")
                .header("x-pad", "x".repeat(9000))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let deployment_path = "/v1/accounts/acct_test/workers/wrk_test/deployments";
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(deployment_path)
                .header("content-length", 18 * 1024 * 1024)
                .header(
                    "x-open-compute-deployment-metadata",
                    format!("{{\"padding\":\"{}\"}}", "x".repeat(9000)),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(deployment_path)
                .header("content-length", 64 * 1024 * 1024 + 1)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let staged_upload_path =
        "/v1/accounts/acct_test/workers/wrk_test/deployment-uploads/upload_test/objects/digest";
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(staged_upload_path)
                .header("content-length", 18 * 1024 * 1024)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(staged_upload_path)
                .header("content-length", 64 * 1024 * 1024 + 1)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/accounts/acct_test/workers/wrk_test/deployment-uploads")
                .header("content-length", 16 * 1024)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/accounts/acct_test/workers/wrk_test/deployment-uploads-extra")
                .header("content-length", 16 * 1024)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("HEAD")
                .uri("/health/live")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    for method in ["PUT", "OPTIONS"] {
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri("/unknown")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/unknown")
                .header("x-one", "x".repeat(6000))
                .header("x-two", "x".repeat(6000))
                .header("x-three", "x".repeat(6000))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let metrics_disabled = test_state(HealthCoordinator::new(), None);
    let admin_without_metrics = http::admin_router(HttpState::for_test(
        metrics_disabled.health().clone(),
        Arc::new(MetricsRegistry::new(&MetricsConfig::default(), "0.1.0", "unknown").unwrap()),
        false,
        None,
    ));
    let res = admin_without_metrics
        .clone()
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    let res = admin_without_metrics
        .oneshot(
            Request::builder()
                .method("OPTIONS")
                .uri("/unknown")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::METHOD_NOT_ALLOWED);

    let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let occupied_addr = occupied.local_addr().unwrap();
    assert_eq!(
        http::bind(occupied_addr).await.unwrap_err().code(),
        ErrorCode::ConfigInvalid
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    http::serve_until(listener, http::merged_router(state), async {})
        .await
        .unwrap();
}

#[tokio::test]
async fn admin_auth_and_separate_routers() {
    let health = HealthCoordinator::new();
    let state = test_state(health, Some("s3cret-token"));
    let public = http::public_router(state.clone());
    let admin = http::admin_router(state);

    let res = public
        .oneshot(
            Request::builder()
                .uri("/health/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);

    let res = admin
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    let res = admin
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health/status")
                .header("Authorization", "Bearer s3cret-token")
                .header("x-forwarded-for", "1.1.1.1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), 16_384).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(!text.contains("pid"));
    assert!(!text.contains("pgid"));
    assert!(!text.contains("token"));

    let res = admin
        .oneshot(
            Request::builder()
                .uri("/health/status")
                .header("Authorization", "Bearer wrong-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn scheduler_operator_routes_are_authenticated_bounded_and_stateful() {
    let (_dir, path, _mock) = initialized_doctor_fixture().await;
    let loaded = load_platform_config(&path).unwrap();
    let storage = Arc::new(
        open_compute_storage::PlatformStorage::bootstrap(
            &loaded.config.storage,
            &open_compute_core::SystemClock,
        )
        .unwrap(),
    );
    let scheduler_path = storage.data_dir().ensure_scheduler_db().unwrap();
    let store = Arc::new(SchedulerStore::open(&scheduler_path, 100, 10).unwrap());
    let transport =
        WorkerdTransport::new(GenerationAuthRegistry::new(), Arc::new(Mutex::new(None)));
    let scheduler = Arc::new(SchedulerService::new(
        store.clone(),
        storage,
        transport,
        SchedulerConfig::default(),
        open_compute_core::WorkflowsConfig::default(),
        Arc::new(SystemSchedulerClock),
    ));
    let state = test_state(HealthCoordinator::new(), Some("scheduler-admin"))
        .with_scheduler(Some(scheduler.clone()));
    let app = http::admin_router(state);

    let unauthorized = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/scheduler")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let request = |method: &str, uri: &str| {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("authorization", "Bearer scheduler-admin")
            .body(Body::empty())
            .unwrap()
    };
    let inspect = app
        .clone()
        .oneshot(request("GET", "/v1/scheduler"))
        .await
        .unwrap();
    assert_eq!(inspect.status(), StatusCode::OK);
    let body = axum::body::to_bytes(inspect.into_body(), 4096)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["version"], 1);
    assert_eq!(body["paused"], false);
    assert_eq!(body["global"]["inFlight"], 0);
    assert_eq!(body["pools"].as_array().unwrap().len(), 4);
    assert_eq!(body["pools"][0]["kind"], "do_alarm");
    assert_eq!(body["pools"][0]["ready"], 0);
    assert_eq!(body["pools"][1]["kind"], "queue");
    assert_eq!(body["pools"][1]["ready"], 0);
    assert_eq!(body["pools"][2]["kind"], "cron");
    assert_eq!(body["pools"][2]["ready"], 0);
    assert_eq!(body["pools"][3]["kind"], "workflow");
    assert_eq!(body["pools"][3]["ready"], 0);
    assert_eq!(body["queueConsumers"], serde_json::json!([]));
    assert_eq!(body["cronActivations"], serde_json::json!([]));

    let operator_inspect = app
        .clone()
        .oneshot(request("GET", "/v1/operator/queue-consumers"))
        .await
        .unwrap();
    assert_eq!(operator_inspect.status(), StatusCode::OK);
    let invalid_consumer = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/operator/queue-consumers/not-an-id/pause")
                .header("authorization", "Bearer scheduler-admin")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"consumerGeneration":1}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(invalid_consumer.status(), StatusCode::BAD_REQUEST);

    let queue_id = open_compute_core::QueueId::generate();
    store
        .create_queue_projection(&open_compute_storage::QueueProjection {
            queue_id,
            account_id: open_compute_core::AccountId::generate(),
            lifecycle_generation: 1,
            config_generation: 1,
            config: open_compute_storage::QueueConfig::default(),
            created_at_ms: 1,
            updated_at_ms: 1,
        })
        .unwrap();
    store
        .enqueue_queue(
            &open_compute_storage::QueueEnqueueRequest {
                queue_id,
                request_id: uuid::Uuid::now_v7(),
                output_gate: false,
                lifecycle_generation: 1,
                config_generation: 1,
                batch_delay_seconds: None,
                messages: vec![open_compute_storage::QueueMessageInput {
                    content_type: open_compute_storage::QueueContentType::Text,
                    body: b"expired".to_vec(),
                    delay_seconds: Some(0),
                }],
            },
            1,
        )
        .unwrap();
    assert_eq!(scheduler.poll_once().await.unwrap(), 1);
    assert_eq!(
        store.queue_metrics(queue_id, 1, 1).unwrap().backlog_count,
        0
    );

    assert_eq!(
        app.clone()
            .oneshot(request("POST", "/v1/scheduler/pause"))
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
    assert!(scheduler.is_paused());
    assert_eq!(
        app.clone()
            .oneshot(request("POST", "/v1/scheduler/resume"))
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
    assert!(!scheduler.is_paused());
    assert_eq!(
        app.clone()
            .oneshot(request(
                "POST",
                "/v1/operator/scheduler/pause?kind=do_alarm"
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
    assert!(scheduler.is_kind_paused(SchedulerKind::Alarm).unwrap());
    assert_eq!(
        app.clone()
            .oneshot(request(
                "POST",
                "/v1/operator/scheduler/resume?kind=do_alarm"
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.clone()
            .oneshot(request("POST", "/v1/operator/scheduler/pause?kind=queue"))
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
    assert!(scheduler.is_kind_paused(SchedulerKind::Queue).unwrap());
    assert_eq!(
        app.clone()
            .oneshot(request("POST", "/v1/operator/scheduler/resume?kind=queue"))
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
    let repair = app
        .clone()
        .oneshot(request("POST", "/v1/scheduler/repair"))
        .await
        .unwrap();
    assert_eq!(repair.status(), StatusCode::OK);
    let body = axum::body::to_bytes(repair.into_body(), 4096)
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&body).unwrap()["repaired"],
        0
    );

    let unavailable = http::admin_router(test_state(HealthCoordinator::new(), None))
        .oneshot(
            Request::builder()
                .uri("/v1/scheduler")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unavailable.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[test]
fn metrics_fixed_and_limits() {
    let cfg = MetricsConfig {
        max_series: 4,
        ..MetricsConfig::default()
    };
    assert_eq!(
        MetricsRegistry::validate_limits(&cfg).unwrap_err().code(),
        ErrorCode::LimitInvalid
    );
    let cfg = MetricsConfig {
        max_series: REQUIRED_SERIES,
        max_label_value_bytes: 8,
        ..MetricsConfig::default()
    };
    assert_eq!(
        MetricsRegistry::validate_limits(&cfg).unwrap_err().code(),
        ErrorCode::LimitInvalid
    );
    let reg =
        MetricsRegistry::new(&MetricsConfig::default(), "0.1.0", "workerd 2026-08-26").unwrap();
    reg.inc_start(StartResult::Success, StartStage::Config);
    reg.observe_service_invocation(
        ServiceMetricOperation::DefaultFetch,
        true,
        Duration::from_millis(7),
    );
    reg.set_service_invocation_counts(2, 3, 5);
    let text = reg.render(&PlatformStatus::starting());
    assert!(text.contains("platform_info"));
    assert!(text.contains("platform_ready"));
    assert!(
        text.contains(
            "service_invocations_total{operation=\"default_fetch\",outcome=\"success\"} 1"
        )
    );
    assert!(
        text.contains("service_invocation_duration_seconds{operation=\"default_fetch\"} 0.007")
    );
    assert!(text.contains("service_invocation_roots 2"));
    assert!(text.contains("service_invocation_operations 3"));
    assert!(text.contains("service_capability_retentions 5"));
    assert!(text.contains("response_cache_operations_total"));
    assert!(text.contains("response_cache_s3_duration_seconds_bucket"));
    assert!(text.contains("images_operations_total"));
    assert!(text.contains("images_limit_rejections_total"));
    let series = text
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .count();
    assert_eq!(series as u64, REQUIRED_SERIES);
    let again = reg.render(&PlatformStatus::starting());
    assert_eq!(text, again);
    assert!(text.contains("content") || text.contains("platform_info"));
    assert!(!text.contains("AKIA"));
}

#[test]
fn queue_metrics_cover_fixed_operations_outcomes_and_backlog() {
    let metrics = MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd").unwrap();
    for operation in [
        QueueMetricOperation::Send,
        QueueMetricOperation::Batch,
        QueueMetricOperation::Metrics,
    ] {
        metrics.observe_queue_producer(operation, false, 2, 3, Duration::from_millis(4));
        metrics.observe_queue_producer(operation, true, 5, 7, Duration::from_millis(8));
        metrics.inc_queue_result_unknown(operation);
    }
    metrics.set_queue_backlog(11, 13);
    metrics.observe_queue_retention(false, 17, 19);
    metrics.observe_queue_retention(true, 23, 29);
    for operation in [
        QueueReconcileOperation::Create,
        QueueReconcileOperation::Config,
        QueueReconcileOperation::Delete,
    ] {
        metrics.observe_queue_reconcile(operation, false, Duration::from_millis(31));
        metrics.observe_queue_reconcile(operation, true, Duration::from_millis(37));
    }
    let rendered = metrics.render(&PlatformStatus::starting());
    for expected in [
        "queue_producer_requests_total{operation=\"send\",outcome=\"error\"} 1",
        "queue_producer_requests_total{operation=\"batch\",outcome=\"success\"} 1",
        "queue_producer_messages_total{operation=\"metrics\",outcome=\"success\"} 5",
        "queue_producer_body_bytes_total{operation=\"send\",outcome=\"success\"} 7",
        "queue_backlog_messages 11",
        "queue_backlog_bytes 13",
        "queue_retention_deleted_total{outcome=\"error\"} 17",
        "queue_retention_deleted_bytes_total{outcome=\"success\"} 29",
        "queue_reconcile_total{operation=\"delete\",outcome=\"success\"} 1",
        "queue_projection_lag_seconds 0.037",
        "queue_result_unknown_total{operation=\"send\"} 1",
        "queue_result_unknown_total{operation=\"batch\"} 1",
    ] {
        assert!(rendered.contains(expected), "missing {expected}");
    }
    assert!(!rendered.contains("queue_result_unknown_total{operation=\"metrics\"}"));
}

#[test]
fn observe_supervisor_counts_one_logical_restart() {
    let reg = MetricsRegistry::new(&MetricsConfig::default(), "0.1.0", "unknown").unwrap();
    let mut snap = SupervisorSnapshot::initial_for_test(SystemTime::UNIX_EPOCH);
    assert_eq!(snap.state, SupervisorState::Stopped);
    assert_eq!(snap.attempt, 0);
    reg.observe_supervisor(&snap);
    snap.state = SupervisorState::Starting;
    snap.attempt = 1;
    reg.observe_supervisor(&snap);
    snap.state = SupervisorState::Running;
    snap.attempt = 1;
    reg.observe_supervisor(&snap);
    snap.state = SupervisorState::Running;
    reg.observe_supervisor(&snap);
    assert_eq!(reg.restart_total(RestartReason::UnexpectedExit), 0);
    assert_eq!(reg.restart_total(RestartReason::ProbeFailed), 0);

    snap.state = SupervisorState::BackingOff;
    snap.attempt = 1;
    reg.observe_supervisor(&snap);
    snap.state = SupervisorState::Starting;
    snap.attempt = 2;
    reg.observe_supervisor(&snap);
    snap.state = SupervisorState::Running;
    snap.attempt = 2;
    reg.observe_supervisor(&snap);
    assert_eq!(reg.restart_total(RestartReason::UnexpectedExit), 1);

    let reg = MetricsRegistry::new(&MetricsConfig::default(), "0.1.0", "unknown").unwrap();
    snap.state = SupervisorState::Running;
    snap.attempt = 1;
    reg.observe_supervisor(&snap);
    snap.state = SupervisorState::Running;
    snap.attempt = 2;
    reg.observe_supervisor(&snap);
    assert_eq!(
        reg.restart_total(RestartReason::UnexpectedExit),
        1,
        "coalesced Running -> BackingOff -> Running"
    );

    let reg = MetricsRegistry::new(&MetricsConfig::default(), "0.1.0", "unknown").unwrap();
    snap.state = SupervisorState::Starting;
    snap.attempt = 1;
    reg.observe_supervisor(&snap);
    snap.state = SupervisorState::Failed;
    snap.attempt = 1;
    reg.observe_supervisor(&snap);
    snap.state = SupervisorState::Failed;
    reg.observe_supervisor(&snap);
    assert_eq!(reg.restart_total(RestartReason::ProbeFailed), 1);
    assert_eq!(reg.restart_total(RestartReason::UnexpectedExit), 0);

    snap.state = SupervisorState::Draining;
    reg.observe_supervisor(&snap);
    snap.state = SupervisorState::Stopping;
    reg.observe_supervisor(&snap);
    assert_eq!(reg.restart_total(RestartReason::ProbeFailed), 1);
    assert_eq!(reg.restart_total(RestartReason::UnexpectedExit), 0);

    let reg = MetricsRegistry::new(&MetricsConfig::default(), "0.1.0", "unknown").unwrap();
    snap.state = SupervisorState::Starting;
    snap.attempt = 2;
    reg.observe_supervisor(&snap);
    snap.state = SupervisorState::Running;
    snap.attempt = 2;
    reg.observe_supervisor(&snap);
    assert_eq!(
        reg.restart_total(RestartReason::UnexpectedExit),
        1,
        "first observed snapshot already at attempt 2 is one restart"
    );
    snap.state = SupervisorState::Running;
    snap.attempt = 2;
    reg.observe_supervisor(&snap);
    assert_eq!(reg.restart_total(RestartReason::UnexpectedExit), 1);

    let reg = MetricsRegistry::new(&MetricsConfig::default(), "0.1.0", "unknown").unwrap();
    snap.state = SupervisorState::Starting;
    snap.attempt = 1;
    reg.observe_supervisor(&snap);
    snap.state = SupervisorState::Starting;
    snap.attempt = 3;
    reg.observe_supervisor(&snap);
    assert_eq!(
        reg.restart_total(RestartReason::UnexpectedExit),
        2,
        "coalesced attempt 1 -> 3 is two logical restarts"
    );

    let reg = MetricsRegistry::new(&MetricsConfig::default(), "0.1.0", "unknown").unwrap();
    snap.state = SupervisorState::Running;
    snap.attempt = 3;
    reg.observe_supervisor(&snap);
    assert_eq!(
        reg.restart_total(RestartReason::UnexpectedExit),
        2,
        "first observed snapshot at attempt 3 is two logical restarts"
    );
    snap.state = SupervisorState::Running;
    snap.attempt = 3;
    reg.observe_supervisor(&snap);
    assert_eq!(reg.restart_total(RestartReason::UnexpectedExit), 2);
}

#[test]
fn metrics_workerd_version_and_preflight_counters() {
    let reg = MetricsRegistry::new(&MetricsConfig::default(), "0.1.0", "unknown").unwrap();
    reg.set_workerd_version("workerd 2026-08-26").unwrap();
    let text = reg.render(&PlatformStatus::starting());
    assert!(text.contains("workerd_version=\"workerd 2026-08-26\""));
    assert!(!text.contains("workerd_version=\"unknown\""));

    let outcome = open_compute_artifacts::PreflightOutcome::successful_canary();
    reg.observe_preflight_success(&outcome);
    assert_eq!(reg.s3_total(S3Op::Put, S3Result::Success), 1);
    assert_eq!(reg.s3_total(S3Op::Head, S3Result::Success), 2);
    assert_eq!(reg.s3_total(S3Op::Get, S3Result::Success), 1);
    assert_eq!(reg.s3_total(S3Op::Delete, S3Result::Success), 1);
    assert_eq!(reg.s3_total(S3Op::Put, S3Result::Failure), 0);
}

#[tokio::test]
async fn default_doctor_does_not_mutate() {
    let dir = TempDir::new().unwrap();
    let path = write_config(dir.path(), "");
    let data = dir.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let mut perms = fs::metadata(&data).unwrap().permissions();
    perms.set_mode(0o700);
    fs::set_permissions(&data, perms).unwrap();
    let before = snapshot(dir.path());
    let wal = data.join("control.sqlite-wal");
    let shm = data.join("control.sqlite-shm");
    assert!(!wal.exists());
    let loaded = load_platform_config(&path).unwrap();
    let report = doctor_report(&loaded, DoctorMode::Basic).await;
    assert!(!wal.exists());
    assert!(!shm.exists());
    let after = snapshot(dir.path());
    assert_eq!(before, after);
    assert!(report.checks.iter().any(|c| c.name == "data_dir"));
    let human = {
        let mut buf = Vec::new();
        report.write(&mut buf, false).unwrap();
        String::from_utf8(buf).unwrap()
    };
    let json = {
        let mut buf = Vec::new();
        report.write(&mut buf, true).unwrap();
        String::from_utf8(buf).unwrap()
    };
    assert!(!human.contains("AKIA"));
    assert!(json.contains("schema_version"));
}

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

#[test]
fn admin_auth_files_and_bearer_matching_fail_closed() {
    let dir = TempDir::new().unwrap();
    let valid = dir.path().join("admin-token");
    write_mode(&valid, "secret-value\r\n", 0o600);
    let secret = resolve_admin_auth(&admin_reference(Some(&valid))).unwrap();
    assert_eq!(secret.expose(), "secret-value");
    assert!(bearer_matches(Some("Bearer secret-value"), &secret));
    assert!(!bearer_matches(Some("Bearer Secret-value"), &secret));
    assert!(!bearer_matches(Some("secret-value"), &secret));
    assert!(!bearer_matches(None, &secret));

    assert_eq!(
        resolve_admin_auth(&admin_reference(Some(Path::new("relative"))))
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
    let loose = dir.path().join("loose");
    write_mode(&loose, "secret", 0o644);
    assert_eq!(
        resolve_admin_auth(&admin_reference(Some(&loose)))
            .unwrap_err()
            .code(),
        ErrorCode::SecretRefInvalid
    );
    for (name, bytes) in [
        ("empty", Vec::new()),
        ("newline-only", b"\r\n".to_vec()),
        ("large", vec![b'x'; 257]),
        ("invalid-utf8", vec![0xff]),
    ] {
        let path = dir.path().join(name);
        fs::write(&path, bytes).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            resolve_admin_auth(&admin_reference(Some(&path)))
                .unwrap_err()
                .code(),
            ErrorCode::SecretRefInvalid
        );
    }
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(&valid, &link).unwrap();
    assert_eq!(
        resolve_admin_auth(&admin_reference(Some(&link)))
            .unwrap_err()
            .code(),
        ErrorCode::SecretRefInvalid
    );
    assert_eq!(
        resolve_admin_auth(&admin_reference(Some(dir.path())))
            .unwrap_err()
            .code(),
        ErrorCode::SecretRefInvalid
    );
    assert_eq!(
        resolve_admin_auth(&admin_reference(None))
            .unwrap_err()
            .code(),
        ErrorCode::SecretRefInvalid
    );

    let missing = format!("OPEN_COMPUTE_TEST_MISSING_ADMIN_{}", std::process::id());
    let fallback = resolve_admin_auth(&SecretReference {
        env: Some(missing.clone()),
        file: Some(valid),
    })
    .unwrap();
    assert_eq!(fallback.expose(), "secret-value");
    assert_eq!(
        resolve_admin_auth(&SecretReference {
            env: Some(missing),
            file: None,
        })
        .unwrap_err()
        .code(),
        ErrorCode::SecretRefInvalid
    );
}

#[test]
fn admin_auth_environment_modes_are_covered_in_isolated_processes() {
    const MARKER: &str = "OPEN_COMPUTE_ADMIN_AUTH_CHILD_MODE";
    const SECRET_ENV: &str = "OPEN_COMPUTE_ADMIN_AUTH_CHILD_SECRET";
    if let Ok(mode) = std::env::var(MARKER) {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("admin-token");
        write_mode(
            &file,
            if mode == "mismatch" {
                "other"
            } else {
                "secret"
            },
            0o600,
        );
        let reference = SecretReference {
            env: Some(SECRET_ENV.to_owned()),
            file: if mode == "env-only" || mode == "empty" || mode == "large" {
                None
            } else {
                Some(file)
            },
        };
        let result = resolve_admin_auth(&reference);
        match mode.as_str() {
            "env-only" | "match" => assert_eq!(result.unwrap().expose(), "secret"),
            "mismatch" | "empty" | "large" => {
                assert_eq!(result.unwrap_err().code(), ErrorCode::SecretRefInvalid);
            }
            _ => panic!("unexpected child mode"),
        }
        return;
    }

    let current = std::env::current_exe().unwrap();
    for (mode, value) in [
        ("env-only", "secret".to_owned()),
        ("match", "secret".to_owned()),
        ("mismatch", "secret".to_owned()),
        ("empty", String::new()),
        ("large", "x".repeat(257)),
    ] {
        let status = Proc::new(&current)
            .args([
                "--exact",
                "tests::admin_auth_environment_modes_are_covered_in_isolated_processes",
                "--test-threads=1",
            ])
            .env(MARKER, mode)
            .env(SECRET_ENV, value)
            .status()
            .unwrap();
        assert!(status.success(), "child mode {mode} failed");
    }
}

#[test]
fn metrics_mutation_surfaces_and_label_bounds_are_complete() {
    let cfg = MetricsConfig {
        max_label_value_bytes: 64,
        ..MetricsConfig::default()
    };
    assert_eq!(
        MetricsRegistry::new(&cfg, &"v".repeat(65), "workerd")
            .unwrap_err()
            .code(),
        ErrorCode::LimitInvalid
    );
    let reg = Arc::new(MetricsRegistry::new(&cfg, "v1", "workerd").unwrap());
    assert_eq!(
        reg.set_workerd_version(&"w".repeat(65)).unwrap_err().code(),
        ErrorCode::LimitInvalid
    );
    reg.set_process_up(true);
    reg.observe_start_duration(Duration::from_millis(250));
    reg.inc_restart(RestartReason::UnexpectedExit);
    reg.inc_restart(RestartReason::ProbeFailed);
    for op in [
        SqliteOp::Open,
        SqliteOp::Migrate,
        SqliteOp::Query,
        SqliteOp::Checkpoint,
    ] {
        reg.observe_sqlite(op, Duration::from_millis(1));
    }
    for op in [S3Op::Head, S3Op::Put, S3Op::Get, S3Op::Delete, S3Op::List] {
        reg.observe_s3(op, S3Result::Failure, Duration::from_millis(2));
        assert_eq!(reg.s3_total(op, S3Result::Failure), 1);
    }
    for op in [
        KvOperation::Get,
        KvOperation::GetWithMetadata,
        KvOperation::GetMany,
        KvOperation::Put,
        KvOperation::Delete,
        KvOperation::List,
    ] {
        reg.observe_kv_operation(op, true, 3, 5, Duration::from_millis(4));
    }
    let successful = KvLifecycleGuard::new(reg.clone(), KvLifecycle::Backup);
    successful.success();
    drop(KvLifecycleGuard::new(reg.clone(), KvLifecycle::Restore));
    reg.inc_kv_maintenance(KvMaintenance::Gc, true);
    reg.inc_kv_maintenance(KvMaintenance::Checkpoint, false);
    reg.inc_kv_corruption(usize::MAX);
    reg.observe_kv_wal_bytes(2 * 1024 * 1024);
    reg.observe_r2_operation(R2Operation::Get, true, Duration::from_millis(6));
    reg.inc_r2_provider_error(R2Operation::Put, R2ProviderError::ResultUnknown);
    reg.inc_r2_result_unknown(false);
    reg.inc_r2_condition_failure(true);
    reg.add_r2_list_head_fanout(3);
    reg.add_r2_bytes(R2StreamDirection::Upload, 7);
    reg.add_r2_bytes(R2StreamDirection::Download, 5);
    reg.observe_d1_operation(
        D1Operation::Query,
        true,
        true,
        Duration::from_millis(3),
        2,
        0,
        17,
    );
    reg.observe_d1_queue_depth(2);
    reg.set_d1_open_databases(3);
    reg.observe_d1_wal_bytes(2 * 1024 * 1024);
    reg.inc_d1_error(D1Operation::Exec, ErrorCode::D1ResultUnknown);
    let d1_backup = D1LifecycleGuard::new(reg.clone(), D1Lifecycle::Backup);
    d1_backup.success();
    drop(D1LifecycleGuard::new(reg.clone(), D1Lifecycle::Migration));
    reg.observe_do_dispatch(DoOperation::Fetch, true, Duration::from_millis(7));
    reg.observe_do_dispatch(DoOperation::Rpc, false, Duration::from_millis(8));
    reg.observe_do_dispatch(DoOperation::Connect, true, Duration::from_millis(9));
    reg.set_do_active_hosts(4);
    for reason in [
        DoFacetReloadReason::Promotion,
        DoFacetReloadReason::Restart,
        DoFacetReloadReason::Delete,
    ] {
        reg.inc_do_facet_reload(reason);
    }
    reg.inc_do_reconcile(DoReconcileState::Creating, true);
    reg.inc_do_reconcile(DoReconcileState::Deleting, false);
    reg.set_do_storage_watermark(usize::MAX);
    reg.observe_scheduler_summary(
        SchedulerSummary {
            scheduled: 3,
            claimed: 2,
            discarding: 1,
            oldest_due_at_ms: Some(1_000),
            expired_claims: 0,
        },
        4_000,
    );
    for kind in SchedulerKind::ALL {
        reg.inc_scheduler_claim(kind, SchedulerClaimOutcome::Claimed);
        reg.inc_scheduler_claim_expired(kind, 2);
        reg.set_scheduler_in_flight(kind, 1);
    }
    for kind in SchedulerKind::ALL {
        reg.observe_scheduler_workload(
            kind,
            open_compute_core::WorkloadSummary {
                ready: 3 + kind.index() as u64,
                claimed: 2,
                expired: 0,
                oldest_due_at_ms: Some(1_000),
                next_due_at_ms: Some(1_000),
            },
            4_000,
        );
    }
    for kind in SchedulerKind::ALL {
        reg.observe_scheduler_claim_duration(kind, Duration::from_millis(4));
    }
    for (kind, state) in SchedulerKind::ALL.into_iter().zip([
        SchedulerPoolState::Ready,
        SchedulerPoolState::Paused,
        SchedulerPoolState::Backoff,
        SchedulerPoolState::CircuitOpen,
    ]) {
        reg.inc_scheduler_stale_completion(kind);
        reg.set_scheduler_pool_state(kind, state);
    }
    reg.inc_scheduler_wake("notification");
    reg.observe_alarm_delivery(AlarmOutcome::Retry, 2, Duration::from_millis(9));
    reg.inc_alarm_mutation(AlarmMutation::Set, true);
    reg.inc_alarm_repair(AlarmRepairSource::Scan, false);
    {
        let _reader = KvGaugeGuard::new(&reg, KvGauge::ReaderConnection);
        let _writer = KvGaugeGuard::new(&reg, KvGauge::WriterConnection);
        let mut staging = KvStagingGauge::new(Some(&reg));
        staging.add(7);
        let _upload = R2StreamGuard::new(&reg, R2StreamDirection::Upload);
        let _download = R2StreamGuard::new(&reg, R2StreamDirection::Download);
        reg.adjust_r2_staging_bytes(11, true);
        let active = reg.render(&PlatformStatus::starting());
        assert!(active.contains("kv_open_connections{role=\"reader\"} 1"));
        assert!(active.contains("kv_open_connections{role=\"writer\"} 1"));
        assert!(active.contains("kv_active_streams 1"));
        assert!(active.contains("kv_staging_bytes 7"));
        assert!(active.contains("r2_active_streams{direction=\"upload\"} 1"));
        assert!(active.contains("r2_active_streams{direction=\"download\"} 1"));
        assert!(active.contains("r2_staging_bytes 11"));
        reg.adjust_r2_staging_bytes(11, false);
    }
    let rendered = reg.render(&PlatformStatus::starting());
    assert!(rendered.contains("workerd_process_up 1"));
    assert!(rendered.contains(
        "kv_operations_total{operation=\"get_with_metadata\",outcome=\"success\",type=\"raw\"} 1"
    ));
    assert!(rendered.contains("kv_backup_total{outcome=\"success\"} 1"));
    assert!(rendered.contains("kv_restore_total{outcome=\"failure\"} 1"));
    assert!(rendered.contains("kv_gc_entries_total{outcome=\"success\"} 1"));
    assert!(rendered.contains("kv_checkpoint_total{outcome=\"failure\"} 1"));
    assert!(rendered.contains("kv_corruption_total{class=\"sqlite\"} 1"));
    assert!(rendered.contains("kv_open_connections{role=\"reader\"} 0"));
    assert!(rendered.contains("kv_active_streams 0"));
    assert!(rendered.contains("kv_staging_bytes 0"));
    assert!(rendered.contains("kv_wal_bytes_bucket{le=\"4194304\"} 1"));
    assert!(rendered.contains("r2_operations_total{operation=\"get\",outcome=\"success\"} 1"));
    assert!(
        rendered.contains("r2_provider_errors_total{stage=\"put\",category=\"result_unknown\"} 1")
    );
    assert!(rendered.contains("r2_result_unknown_total{operation=\"put\"} 1"));
    assert!(rendered.contains("r2_condition_failures_total{operation=\"put\"} 1"));
    assert!(rendered.contains("r2_list_head_fanout_total 3"));
    assert!(rendered.contains("r2_bytes_total{direction=\"ingress\"} 7"));
    assert!(rendered.contains("r2_bytes_total{direction=\"egress\"} 5"));
    assert!(rendered.contains("r2_active_streams{direction=\"upload\"} 0"));
    assert!(rendered.contains("r2_staging_bytes 0"));
    assert!(rendered.contains(
        "d1_operations_total{operation=\"query\",outcome=\"success\",readonly=\"true\"} 1"
    ));
    assert!(rendered.contains("d1_operation_queue_depth_bucket{le=\"4\"} 1"));
    assert!(rendered.contains("d1_open_databases 3"));
    assert!(rendered.contains("d1_wal_bytes_bucket{le=\"4194304\"} 1"));
    assert!(rendered.contains("d1_result_unknown_total{operation=\"exec\"} 1"));
    assert!(rendered.contains("d1_backup_total{outcome=\"success\"} 1"));
    assert!(rendered.contains("d1_migration_total{outcome=\"failure\"} 1"));
    assert!(rendered.contains("oc_do_dispatch_total{operation=\"fetch\",outcome=\"success\"} 1"));
    assert!(rendered.contains("oc_do_dispatch_total{operation=\"rpc\",outcome=\"failure\"} 1"));
    assert!(rendered.contains("oc_do_dispatch_total{operation=\"connect\",outcome=\"success\"} 1"));
    assert!(rendered.contains("oc_do_active_host_actors 4"));
    assert!(rendered.contains("oc_do_facet_reload_total{reason=\"promotion\"} 1"));
    assert!(
        rendered.contains("oc_do_object_reconcile_total{state=\"deleting\",outcome=\"failure\"} 1")
    );
    assert!(!rendered.contains("oc_do_websocket_active"));
    assert!(!rendered.contains("oc_do_storage_bytes"));
    assert!(rendered.contains("oc_do_storage_watermark{state=\"stop\"} 1"));
    assert!(rendered.contains("oc_do_alarm_jobs{state=\"scheduled\"} 3"));
    assert!(!rendered.contains("oc_scheduler_"));
    assert!(rendered.contains("open_compute_scheduler_ready{kind=\"do_alarm\"} 3"));
    assert!(
        rendered.contains(
            "open_compute_scheduler_claim_total{kind=\"do_alarm\",outcome=\"claimed\"} 1"
        )
    );
    assert!(
        rendered.contains("open_compute_scheduler_stale_completion_total{kind=\"do_alarm\"} 1")
    );
    assert!(rendered.contains("open_compute_scheduler_ready{kind=\"queue\"} 4"));
    assert!(rendered.contains("open_compute_scheduler_ready{kind=\"cron\"} 5"));
    assert!(rendered.contains("open_compute_scheduler_ready{kind=\"workflow\"} 6"));
    assert!(rendered.contains("open_compute_scheduler_in_flight{kind=\"workflow\"} 1"));
    assert!(rendered.contains("open_compute_scheduler_lease_recovery_total{kind=\"queue\"} 2"));
    assert!(
        rendered.contains("open_compute_scheduler_stale_completion_total{kind=\"workflow\"} 1")
    );
    assert!(
        rendered.contains("open_compute_scheduler_pool_state{kind=\"do_alarm\",state=\"ready\"} 1")
    );
    assert!(
        rendered.contains("open_compute_scheduler_pool_state{kind=\"queue\",state=\"paused\"} 1")
    );
    assert!(
        rendered.contains("open_compute_scheduler_pool_state{kind=\"cron\",state=\"backoff\"} 1")
    );
    assert!(rendered.contains("open_compute_scheduler_wake_total{reason=\"notification\"} 1"));
    assert!(
        rendered.contains("oc_do_alarm_mutation_total{operation=\"set\",outcome=\"success\"} 1")
    );
    assert!(
        rendered.contains("oc_do_alarm_delivery_total{outcome=\"retry\",retry_bucket=\"2\"} 1")
    );
    assert!(rendered.contains("oc_do_alarm_repair_total{source=\"scan\",outcome=\"failure\"} 1"));
    assert!(rendered.contains("oc_do_alarm_lag_seconds 3"));
}

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

async fn initialized_doctor_fixture() -> (TempDir, PathBuf, open_compute_artifacts::MockS3) {
    let dir = TempDir::new().unwrap();
    let mock = open_compute_artifacts::MockS3::spawn("open-compute").await;
    let ak = dir.path().join("ak");
    let sk = dir.path().join("sk");
    write_mode(&ak, "AKIAEXAMPLEKEYID01", 0o600);
    write_mode(&sk, "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY", 0o600);
    let extra = format!(
        r#"
[s3]
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
    let loaded = load_platform_config(&path).unwrap();
    let storage = open_compute_storage::PlatformStorage::bootstrap(
        &loaded.config.storage,
        &open_compute_core::SystemClock,
    )
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
    (dir, path, mock)
}

#[tokio::test]
async fn p1_startup_receipts_health_and_inventory_metrics_cover_real_authority() {
    let (dir, path, _mock) = initialized_doctor_fixture().await;
    let loaded = load_platform_config(&path).unwrap();
    let fresh_root = dir.path().join("fresh-schema-root");
    fs::create_dir(&fresh_root).unwrap();
    let mut fresh = loaded.clone();
    fresh.config.storage.data_dir = fresh_root.clone();
    assert!(crate::run::p1::require_current_serving_schema(&fresh).is_ok());
    fs::write(fresh_root.join("control.sqlite"), b"").unwrap();
    assert!(crate::run::p1::require_current_serving_schema(&fresh).is_ok());
    drop(
        open_compute_storage::ControlDb::open(
            &fresh_root.join("control.sqlite"),
            loaded.config.storage.sqlite_busy_timeout_ms,
        )
        .unwrap(),
    );
    assert!(
        crate::run::p1::require_current_serving_schema(&fresh).is_ok(),
        "unmigrated control.sqlite must still first-start"
    );
    assert!(crate::run::p1::require_current_serving_schema(&loaded).is_ok());

    let data_dir = DataDir::acquire_existing_offline(&loaded.config.storage).unwrap();
    let now_ms = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    data_dir
        .write_operation_receipt(
            "last-snapshot.json",
            serde_json::to_vec(&serde_json::json!({
                "bytes": 321,
                "created_at_ms": now_ms,
                "duration_ms": 12,
                "verified": true
            }))
            .unwrap()
            .as_slice(),
        )
        .unwrap();
    data_dir
        .write_operation_receipt(
            "last-restore.json",
            serde_json::to_vec(&serde_json::json!({
                "restored_at_ms": now_ms,
                "duration_ms": 34,
                "smoke_verified": true
            }))
            .unwrap()
            .as_slice(),
        )
        .unwrap();

    let metrics = MetricsRegistry::new(&loaded.config.metrics, "test", "workerd").unwrap();
    crate::run::p1::load_offline_metrics_receipts(&data_dir, &metrics);
    let health = HealthCoordinator::new();
    crate::run::p1::update_operations_health(&data_dir, 60_000, &health).unwrap();
    let operations = health
        .snapshot()
        .components
        .into_iter()
        .find(|component| component.name == ComponentName::Operations)
        .unwrap();
    assert_eq!(operations.state, ComponentState::Healthy);

    let storage = open_compute_storage::PlatformStorage::bootstrap(
        &loaded.config.storage,
        &open_compute_core::SystemClock,
    );
    assert_eq!(storage.unwrap_err().code(), ErrorCode::DataDirInUse);
    drop(data_dir);
    let storage = open_compute_storage::PlatformStorage::bootstrap(
        &loaded.config.storage,
        &open_compute_core::SystemClock,
    )
    .unwrap();
    crate::run::p1::refresh_metrics(
        &storage,
        &metrics,
        loaded.config.hardening.emergency_reserve_bytes,
    )
    .unwrap();
    let rendered = metrics.render(&health.snapshot());
    assert!(rendered.contains("platform_snapshot_last_bytes 321"));
    assert!(rendered.contains("platform_restore_last_smoke_verified 1"));
    assert!(rendered.contains("platform_resource_count{resource=\"accounts\"} 1"));
    drop(storage);

    let data_dir = DataDir::acquire_existing_offline(&loaded.config.storage).unwrap();
    data_dir
        .write_operation_receipt("last-snapshot.json", br#"{"verified":false}"#)
        .unwrap();
    crate::run::p1::load_offline_metrics_receipts(&data_dir, &metrics);
    crate::run::p1::update_operations_health(&data_dir, 0, &health).unwrap();
    let operations = health
        .snapshot()
        .components
        .into_iter()
        .find(|component| component.name == ComponentName::Operations)
        .unwrap();
    assert_eq!(operations.state, ComponentState::Degraded);
    assert_eq!(operations.reason, Some(ReadinessReason::SnapshotStale));
}

#[tokio::test]
async fn p1_capability_release_support_bundle_and_metrics_contract_is_bounded() {
    assert_eq!(
        crate::snapshot_pins::SnapshotPins::Unavailable
            .ensure_unpinned("system/artifacts/v1/sha256/untrusted")
            .unwrap_err()
            .code(),
        ErrorCode::ResourceReferenced
    );
    let (dir, path, _mock) = initialized_doctor_fixture().await;
    let mut loaded = load_platform_config(&path).unwrap();
    let capabilities = crate::capabilities::platform_capabilities(&loaded.config).unwrap();
    assert!(capabilities.validate());
    assert!(
        capabilities.products["durable_objects"]
            .members
            .iter()
            .any(|member| member.member == "get"
                && member.status != open_compute_core::CapabilityStatus::Blocked)
    );
    assert_eq!(
        capabilities.products["queues"].status,
        open_compute_core::CapabilityStatus::SupportedWithDeviation
    );
    assert_eq!(
        capabilities.products["queues"].deviations,
        vec!["OC-QUEUE-001"]
    );
    assert_eq!(
        capabilities.products["cron"].status,
        open_compute_core::CapabilityStatus::SupportedWithDeviation
    );
    assert_eq!(
        capabilities.products["cron"].deviations,
        vec!["OC-CRON-001"]
    );
    assert_eq!(
        capabilities.products["workflows"].status,
        open_compute_core::CapabilityStatus::SupportedWithDeviation
    );
    assert_eq!(
        capabilities.products["workflows"].deviations,
        vec!["OC-WORKFLOW-001"]
    );
    assert_eq!(
        capabilities.products["websocket_hibernation"].status,
        open_compute_core::CapabilityStatus::Supported
    );
    assert!(!capabilities.runtime.workers_types_version.is_empty());
    assert_eq!(capabilities.runtime.workers_types_ast_sha256.len(), 64);
    let metadata = crate::capabilities::platform_release_metadata(&loaded).unwrap();
    assert!(metadata.validate());
    assert_eq!(metadata.release, capabilities.release);
    assert_eq!(
        metadata.schema_definitions.last().unwrap().version,
        metadata.release.control_schema_version
    );
    let policy = crate::capabilities::platform_config_policy_sha256(&loaded).unwrap();
    let original_data_dir = loaded.config.storage.data_dir.clone();
    let original_master_key_file = loaded.config.storage.master_key_file.clone();
    let original_public_bind = loaded.config.server.public_bind;
    let original_admin_bind = loaded.config.server.admin_bind;
    loaded.config.storage.data_dir = dir.path().join("relocated-data");
    loaded.config.storage.master_key_file = dir.path().join("relocated-recovery-key");
    loaded.config.server.public_bind = "127.0.0.1:65001".parse().unwrap();
    loaded.config.server.admin_bind = Some("127.0.0.1:65002".to_owned());
    assert_eq!(
        crate::capabilities::platform_config_policy_sha256(&loaded).unwrap(),
        policy,
        "host paths and listener ports are intentionally outside restore policy"
    );
    loaded.config.kv.namespace_quota_bytes += 4096;
    assert_ne!(
        crate::capabilities::platform_config_policy_sha256(&loaded).unwrap(),
        policy,
        "product semantics must change the authenticated restore policy"
    );
    loaded.config.kv.namespace_quota_bytes -= 4096;
    loaded.config.storage.data_dir = original_data_dir;
    loaded.config.storage.master_key_file = original_master_key_file;
    loaded.config.server.public_bind = original_public_bind;
    loaded.config.server.admin_bind = original_admin_bind;

    let operations = loaded.config.storage.data_dir.join("operations");
    fs::create_dir(&operations).unwrap();
    fs::set_permissions(&operations, fs::Permissions::from_mode(0o700)).unwrap();
    let snapshot_receipt = operations.join("last-snapshot.json");
    write_mode(
        &snapshot_receipt,
        r#"{"schema_version":1,"created_at_ms":1,"verified":true}"#,
        0o600,
    );
    let outside_receipt = dir.path().join("outside-receipt.json");
    write_mode(&outside_receipt, r#"{"secret":"outside"}"#, 0o600);
    std::os::unix::fs::symlink(&outside_receipt, operations.join("last-restore.json")).unwrap();

    let output = fs::canonicalize(dir.path())
        .unwrap()
        .join("open-compute-support.tar");
    let result = crate::support_bundle::create_support_bundle(&loaded, &output)
        .await
        .unwrap();
    assert_eq!(result.entries, 8);
    assert_eq!(
        fs::metadata(&output).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let archive = fs::read(output).unwrap();
    assert!(!archive.windows(4).any(|window| window == b"AKIA"));
    assert!(
        !archive
            .windows(b"wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".len())
            .any(|window| window == b"wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY")
    );
    for name in [
        b"config-policy.json".as_slice(),
        b"doctor.json".as_slice(),
        b"metrics.prom".as_slice(),
        b"receipts/last-snapshot.json".as_slice(),
        b"release.json".as_slice(),
    ] {
        assert!(archive.windows(name.len()).any(|window| window == name));
    }
    assert_eq!(
        crate::support_bundle::create_support_bundle(&loaded, Path::new("relative.tar"))
            .await
            .unwrap_err()
            .code(),
        ErrorCode::SupportBundleInvalid
    );
    let existing = fs::canonicalize(dir.path())
        .unwrap()
        .join("existing-support.tar");
    fs::write(&existing, b"existing").unwrap();
    assert_eq!(
        crate::support_bundle::create_support_bundle(&loaded, &existing)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::SupportBundleInvalid
    );
    loaded.config.hardening.max_support_bundle_bytes = 1;
    assert_eq!(
        crate::support_bundle::create_support_bundle(
            &loaded,
            &fs::canonicalize(dir.path())
                .unwrap()
                .join("bounded-support.tar"),
        )
        .await
        .unwrap_err()
        .code(),
        ErrorCode::SupportBundleInvalid
    );
    loaded.config.hardening.max_support_bundle_bytes = 32 * 1024 * 1024;

    fs::remove_file(operations.join("last-restore.json")).unwrap();
    write_mode(&operations.join("last-restore.json"), "not-json", 0o600);
    assert_eq!(
        crate::support_bundle::create_support_bundle(
            &loaded,
            &fs::canonicalize(dir.path())
                .unwrap()
                .join("invalid-receipt-support.tar"),
        )
        .await
        .unwrap_err()
        .code(),
        ErrorCode::SupportBundleInvalid
    );
    fs::remove_file(operations.join("last-restore.json")).unwrap();

    let admin_secret = dir.path().join("admin-auth-secret");
    write_mode(&admin_secret, "p1-support-admin-secret", 0o600);
    loaded.config.server.admin_auth = Some(SecretReference {
        env: None,
        file: Some(admin_secret),
    });
    let admin_output = fs::canonicalize(dir.path())
        .unwrap()
        .join("admin-support.tar");
    crate::support_bundle::create_support_bundle(&loaded, &admin_output)
        .await
        .unwrap();
    assert!(
        !fs::read(admin_output)
            .unwrap()
            .windows(b"p1-support-admin-secret".len())
            .any(|window| window == b"p1-support-admin-secret")
    );

    let metrics = MetricsRegistry::new(&loaded.config.metrics, "test", "workerd").unwrap();
    metrics.observe_admission(open_compute_core::OperationClass::Kv, None);
    metrics.observe_admission(
        open_compute_core::OperationClass::Workers,
        Some(ErrorCode::QuotaExceeded),
    );
    metrics.observe_admission(
        open_compute_core::OperationClass::D1,
        Some(ErrorCode::AdmissionBusy),
    );
    metrics.observe_admission(
        open_compute_core::OperationClass::Restore,
        Some(ErrorCode::StoragePressure),
    );
    metrics.observe_admission(
        open_compute_core::OperationClass::Snapshot,
        Some(ErrorCode::PlatformUnavailable),
    );
    metrics.set_disk_admission(
        &open_compute_core::AdmissionSnapshotV1 {
            schema_version: 1,
            filesystem_free_bytes: 100,
            soft_reserve_bytes: 80,
            hard_reserve_bytes: 60,
            emergency_reserve_bytes: 10,
            reserved_bytes: 7,
            owned_staging_bytes: 3,
            mode: open_compute_core::PlatformMode::Serving,
        },
        10,
    );
    metrics.set_schema_version(8);
    metrics.set_schema_failed_resources(2);
    metrics.set_resource_counts([1, 2, 3, 4, 5, 6, 7, 8]);
    metrics.observe_product_error(
        open_compute_core::OperationClass::DurableObjects,
        ErrorCode::QuotaExceeded,
    );
    metrics.inc_sqlite_busy();
    metrics.inc_sqlite_check_failure();
    metrics.inc_websocket_close(WebSocketCloseReason::DeploymentRestart);
    for reason in [
        WebSocketCloseReason::Normal,
        WebSocketCloseReason::Shutdown,
        WebSocketCloseReason::Error,
        WebSocketCloseReason::Disconnected,
    ] {
        metrics.inc_websocket_close(reason);
    }
    metrics.record_snapshot_receipt(122, Duration::from_millis(11));
    metrics.record_snapshot_receipt_at(123, Duration::from_millis(12), 1);
    metrics.record_restore_receipt(1, Duration::from_millis(34), true);
    metrics
        .set_release_identity(&"a".repeat(64), "p1.0-capabilities-v1")
        .unwrap();
    assert!(metrics.set_release_identity("BAD", "p1").is_err());
    metrics.inc_quota_reject("unsupported-product");
    metrics.observe_product_error(
        open_compute_core::OperationClass::Scheduler,
        ErrorCode::QuotaExceeded,
    );
    metrics.observe_product_error(open_compute_core::OperationClass::Kv, ErrorCode::KvBusy);
    metrics.observe_product_error(
        open_compute_core::OperationClass::D1,
        ErrorCode::D1Overloaded,
    );
    let mut staging_gauge = KvStagingGauge::new(None);
    staging_gauge.add(5);
    assert!(format!("{staging_gauge:?}").contains("bytes: 5"));
    for operation in [
        ResourceOperation::Create,
        ResourceOperation::Get,
        ResourceOperation::List,
        ResourceOperation::Rename,
        ResourceOperation::Delete,
    ] {
        metrics.observe_resource_operation(operation, false, Duration::from_millis(1));
        metrics.observe_resource_operation(operation, true, Duration::from_millis(2));
    }
    metrics.set_resource_open_handles(7);
    metrics.observe_resource_pin_wait(Duration::from_millis(3));
    for deleting in [false, true] {
        for success in [false, true] {
            metrics.inc_resource_reconcile(deleting, success);
        }
    }
    let rendered = metrics.render(&PlatformStatus::starting());
    assert!(rendered.contains("platform_admission_total{operation=\"kv\",outcome=\"accepted\"} 1"));
    assert!(rendered.contains(
        "platform_admission_total{operation=\"restore\",outcome=\"storage_pressure\"} 1"
    ));
    assert!(rendered.contains("platform_schema_current 8"));
    assert!(rendered.contains("platform_schema_failed_resources 2"));
    assert!(rendered.contains("platform_resource_count{resource=\"d1_databases\"} 7"));
    assert!(rendered.contains("platform_quota_reject_total{product=\"durable_objects\"} 1"));
    assert!(rendered.contains("sqlite_busy_total 3"));
    assert!(rendered.contains("sqlite_check_failure_total 1"));
    assert!(rendered.contains("oc_do_websocket_close_total{reason=\"deployment_restart\"} 1"));
    assert!(rendered.contains("platform_restore_last_smoke_verified 1"));
    assert!(rendered.contains("conformance_result=\"p1.0-capabilities-v1\""));
    assert!(rendered.contains(
        "resource_operations_total{kind=\"kv_namespace\",operation=\"create\",outcome=\"success\"} 1"
    ));
    assert!(rendered.contains("resource_open_handles{kind=\"kv_namespace\"} 7"));
}

async fn initialized_worker_http_fixture() -> (
    TempDir,
    open_compute_artifacts::MockS3,
    HttpState,
    open_compute_core::AccountId,
) {
    let (dir, path, mock) = initialized_doctor_fixture().await;
    let loaded = load_platform_config(&path).unwrap();
    let storage = Arc::new(
        open_compute_storage::PlatformStorage::bootstrap(
            &loaded.config.storage,
            &open_compute_core::SystemClock,
        )
        .unwrap(),
    );
    let account = storage.identity().default_account_id;
    let credentials = open_compute_artifacts::resolve_s3_credentials(&loaded.config.s3).unwrap();
    let client = open_compute_artifacts::S3ArtifactClient::connect(
        &loaded.config.s3,
        &credentials,
        loaded.config.cache.max_artifact_bytes,
    )
    .unwrap();
    let transport =
        WorkerdTransport::new(GenerationAuthRegistry::new(), Arc::new(Mutex::new(None)));
    let api = WorkerApiState::new(
        storage,
        open_compute_artifacts::ArtifactStore::new(client),
        transport,
        DeploymentPins::new(),
        BundleLimits::default(),
        Duration::from_millis(10),
    );
    assert!(format!("{api:?}").contains("WorkerApiState"));
    assert_eq!(
        api.pins()
            .count(open_compute_core::DeploymentId::generate()),
        0
    );
    (
        dir,
        mock,
        test_state(HealthCoordinator::new(), Some("admin-token")).with_worker_api(api),
        account,
    )
}

#[tokio::test]
async fn p2_3_promotion_is_idempotent_preserves_pause_and_resumes_an_interrupted_update() {
    let (_dir, path, _mock) = initialized_doctor_fixture().await;
    let loaded = load_platform_config(&path).unwrap();
    let storage = Arc::new(
        open_compute_storage::PlatformStorage::bootstrap(
            &loaded.config.storage,
            &open_compute_core::SystemClock,
        )
        .unwrap(),
    );
    let scheduler_path = storage.data_dir().ensure_scheduler_db().unwrap();
    let scheduler_store = Arc::new(SchedulerStore::open(&scheduler_path, 100, 1).unwrap());
    let account = storage.identity().default_account_id;
    let queue_id = open_compute_core::QueueId::generate();
    let queue_config = open_compute_storage::QueueConfig::default();
    let queues = open_compute_storage::QueueRepository::new(storage.db());
    queues
        .insert_creating(account, queue_id, "promotion-queue", queue_config, 1)
        .unwrap();
    scheduler_store
        .create_queue_projection(&open_compute_storage::QueueProjection {
            queue_id,
            account_id: account,
            lifecycle_generation: 1,
            config_generation: 1,
            config: queue_config,
            created_at_ms: 1,
            updated_at_ms: 1,
        })
        .unwrap();
    queues.mark_ready(account, queue_id, 2).unwrap();

    let workers = open_compute_storage::WorkerRepository::new(storage.db());
    let (worker, _) = workers
        .create_worker(
            account,
            "p2-3-promotion",
            open_compute_core::RequestId::generate(),
            2,
            1_000_000,
        )
        .unwrap();
    let credentials = open_compute_artifacts::resolve_s3_credentials(&loaded.config.s3).unwrap();
    let client = open_compute_artifacts::S3ArtifactClient::connect(
        &loaded.config.s3,
        &credentials,
        loaded.config.cache.max_artifact_bytes,
    )
    .unwrap();
    let validator: Arc<dyn RuntimeValidator> = Arc::new(|_: ValidationCandidate| async { Ok(()) });
    let promoter = Arc::new(crate::p2_3_promotion::P23PromotionCoordinator::new(
        storage.clone(),
        scheduler_store.clone(),
        Duration::from_millis(100),
    ));
    let controller = DeploymentController::new(
        &storage,
        open_compute_artifacts::ArtifactStore::new(client),
        validator,
        BundleLimits::default(),
    )
    .with_product_promoter(promoter.clone());

    let request = |key: &str, label: &str, promote: bool, cron: &str, batch_size: u32| {
        let source = format!(
            "export default {{ fetch() {{ return new Response('{label}'); }}, queue() {{}}, scheduled() {{}} }};"
        );
        let bundle = CanonicalBundle::build(
            "index.js",
            vec![ModuleInput {
                name: "index.js".to_owned(),
                module_type: ModuleType::EsModule,
                bytes: source.into_bytes(),
            }],
            BundleLimits::default(),
        )
        .unwrap();
        CreateDeploymentRequest {
            account_id: account,
            worker_id: worker.id,
            idempotency_key: key.to_owned(),
            content: open_compute_workers::DeploymentContent::Worker {
                bundle: bundle.into_bytes().into(),
                assets: None,
            },
            vars: std::collections::BTreeMap::new(),
            secrets: std::collections::BTreeMap::new(),
            bindings: std::collections::BTreeMap::new(),
            services: std::collections::BTreeMap::new(),
            runtime_features: Default::default(),
            queue_consumers: vec![QueueConsumerInput {
                queue: queue_id,
                entrypoint: None,
                config: open_compute_storage::QueueConsumerConfig {
                    max_batch_size: batch_size,
                    ..open_compute_storage::QueueConsumerConfig::default()
                },
                dead_letter_queue: None,
            }],
            crons: vec![cron.to_owned()],
            promote,
            request_id: open_compute_core::RequestId::generate(),
            now_ms: 60_000,
        }
    };

    let first = controller
        .create_deployment(request("p23-first", "first", true, "*/5 * * * *", 10))
        .await
        .unwrap();
    let first_id = match first {
        CreateDeploymentOutcome::Applied(result) => result.deployment.id,
        CreateDeploymentOutcome::Replay(_) => panic!("first P2.3 deployment replayed"),
    };
    let consumer_repo = open_compute_storage::QueueConsumerRepository::new(storage.db());
    let first_consumer = consumer_repo.live_for_queue(queue_id).unwrap().unwrap();
    assert_eq!(
        first_consumer.state,
        open_compute_storage::QueueConsumerState::Active
    );
    assert_eq!(first_consumer.deployment_id, first_id);
    assert!(
        scheduler_store
            .inspect_queue_consumer_runtime(queue_id, first_consumer.id, 1)
            .unwrap()
            .projection_exists
    );
    let first_crons = open_compute_storage::CronRepository::new(storage.db())
        .live_for_worker(worker.id)
        .unwrap();
    assert_eq!(first_crons.len(), 1);
    assert_eq!(
        first_crons[0].state,
        open_compute_storage::CronActivationState::Active
    );

    promoter
        .promote(ProductPromotionRequest {
            account_id: account,
            worker_id: worker.id,
            deployment_id: first_id,
            request_id: open_compute_core::RequestId::generate(),
            now_ms: 60_001,
        })
        .await
        .unwrap();
    assert_eq!(
        consumer_repo
            .live_for_queue(queue_id)
            .unwrap()
            .unwrap()
            .consumer_generation,
        1
    );

    let responses = FakeCustomEventResponses {
        queue: Arc::new(Mutex::new(serde_json::json!({
            "outcome": "ok",
            "ackAll": true,
            "retryBatch": {"retry": false},
            "explicitAcks": [],
            "retryMessages": []
        }))),
        cron: Arc::new(Mutex::new(serde_json::json!({
            "outcome": "ok",
            "noRetry": false
        }))),
    };
    let custom_event_listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let custom_event_port = custom_event_listener.local_addr().unwrap().port();
    let server_responses = responses.clone();
    let custom_event_task = tokio::spawn(async move {
        axum::serve(
            custom_event_listener,
            Router::new()
                .route("/internal/queue", post(fake_queue_custom_event))
                .route("/internal/scheduled", post(fake_cron_custom_event))
                .with_state(server_responses)
                .into_make_service(),
        )
        .await
        .unwrap();
    });
    let auth = GenerationAuthRegistry::new();
    auth.activate_for_test(SecretString::new("11".repeat(32)));
    let transport = WorkerdTransport::for_test_endpoint(auth, custom_event_port);
    let clock = Arc::new(open_compute_core::DeterministicSchedulerClock::new(300_000));
    let metrics =
        Arc::new(MetricsRegistry::new(&MetricsConfig::default(), "test", "test").unwrap());
    let scheduler = Arc::new(
        SchedulerService::new(
            scheduler_store.clone(),
            storage.clone(),
            transport,
            SchedulerConfig::default(),
            open_compute_core::WorkflowsConfig::default(),
            clock.clone(),
        )
        .with_metrics(metrics),
    );
    scheduler_store
        .enqueue_queue(
            &open_compute_storage::QueueEnqueueRequest {
                queue_id,
                request_id: uuid::Uuid::now_v7(),
                output_gate: false,
                lifecycle_generation: 1,
                config_generation: 1,
                batch_delay_seconds: None,
                messages: vec![open_compute_storage::QueueMessageInput {
                    content_type: open_compute_storage::QueueContentType::Json,
                    body: br#"{"event":"first"}"#.to_vec(),
                    delay_seconds: None,
                }],
            },
            300_000,
        )
        .unwrap();
    clock.set_wall_time_ms(305_000);
    let before_dispatch = scheduler.inspect().unwrap();
    assert_eq!(before_dispatch.queue_consumers.len(), 1);
    assert_eq!(before_dispatch.cron_activations.len(), 1);
    for kind in [
        SchedulerKind::Alarm,
        SchedulerKind::Queue,
        SchedulerKind::Cron,
        SchedulerKind::Workflow,
    ] {
        scheduler.pause_kind(kind).unwrap();
        assert!(scheduler.is_kind_paused(kind).unwrap());
        scheduler.resume_kind(kind).unwrap();
        assert!(!scheduler.is_kind_paused(kind).unwrap());
    }
    assert!(!scheduler.is_kind_paused(SchedulerKind::Workflow).unwrap());
    assert_eq!(
        scheduler.repair_products(0).unwrap_err().code(),
        ErrorCode::SchedulerUnavailable
    );
    assert!(scheduler.repair_products(1_000).unwrap() >= 2);

    let (kernel_shutdown, kernel_shutdown_rx) = tokio::sync::watch::channel(false);
    let kernel = tokio::spawn(scheduler.clone().run(kernel_shutdown_rx));
    for _ in 0..10_000 {
        let queue_empty = scheduler_store.queue_backlog_totals().unwrap().0 == 0;
        let cron_complete = scheduler
            .inspect()
            .unwrap()
            .cron_activations
            .first()
            .and_then(|activation| activation.last_outcome.as_deref())
            == Some("complete");
        if queue_empty && cron_complete {
            break;
        }
        tokio::task::yield_now().await;
    }
    let dispatched = scheduler.inspect().unwrap();
    assert_eq!(
        scheduler_store.queue_backlog_totals().unwrap(),
        (0, 0),
        "{dispatched:?}"
    );
    assert_eq!(
        scheduler
            .inspect()
            .unwrap()
            .cron_activations
            .first()
            .and_then(|activation| activation.last_outcome.as_deref()),
        Some("complete")
    );
    kernel_shutdown.send(true).unwrap();
    kernel.await.unwrap().unwrap();

    *responses.queue.lock().unwrap() = serde_json::json!({
        "outcome": "exception",
        "ackAll": false,
        "retryBatch": {"retry": false},
        "explicitAcks": [],
        "retryMessages": []
    });
    *responses.cron.lock().unwrap() = serde_json::json!({
        "outcome": "exception",
        "noRetry": false
    });
    clock.set_wall_time_ms(600_000);
    scheduler_store
        .enqueue_queue(
            &open_compute_storage::QueueEnqueueRequest {
                queue_id,
                request_id: uuid::Uuid::now_v7(),
                output_gate: false,
                lifecycle_generation: 1,
                config_generation: 1,
                batch_delay_seconds: None,
                messages: vec![open_compute_storage::QueueMessageInput {
                    content_type: open_compute_storage::QueueContentType::Text,
                    body: b"retry".to_vec(),
                    delay_seconds: None,
                }],
            },
            600_000,
        )
        .unwrap();
    clock.set_wall_time_ms(605_000);
    let [retry_batch] = scheduler
        .claim_queue_consumers(1)
        .await
        .unwrap()
        .try_into()
        .unwrap();
    let [retry_run] = scheduler.claim_cron(1).await.unwrap().try_into().unwrap();
    scheduler
        .clone()
        .dispatch_queue_batch(retry_batch.clone())
        .await;
    scheduler.clone().dispatch_cron_run(retry_run.clone()).await;
    assert_eq!(scheduler_store.queue_backlog_totals().unwrap().0, 1);

    *responses.queue.lock().unwrap() = serde_json::json!({
        "outcome": "ok",
        "ackAll": true,
        "retryBatch": {"retry": false},
        "explicitAcks": [],
        "retryMessages": []
    });
    *responses.cron.lock().unwrap() = serde_json::json!({
        "outcome": "ok",
        "noRetry": false
    });
    scheduler
        .clone()
        .dispatch_queue_batch(retry_batch.clone())
        .await;
    scheduler.clone().dispatch_cron_run(retry_run.clone()).await;

    let mut missing_queue_authority = retry_batch.clone();
    missing_queue_authority.worker_id = open_compute_core::WorkerId::generate();
    scheduler
        .clone()
        .dispatch_queue_batch(missing_queue_authority)
        .await;
    let mut missing_queue_deployment = retry_batch.clone();
    missing_queue_deployment.deployment_id = open_compute_core::DeploymentId::generate();
    scheduler
        .clone()
        .dispatch_queue_batch(missing_queue_deployment)
        .await;
    let mut invalid_queue_generation = retry_batch.clone();
    invalid_queue_generation.execution_generation = u64::MAX;
    scheduler
        .clone()
        .dispatch_queue_batch(invalid_queue_generation)
        .await;
    let mut missing_cron_authority = retry_run.clone();
    missing_cron_authority.worker_id = open_compute_core::WorkerId::generate();
    scheduler
        .clone()
        .dispatch_cron_run(missing_cron_authority)
        .await;
    let mut invalid_cron_generation = retry_run.clone();
    invalid_cron_generation.execution_generation = u64::MAX;
    scheduler
        .clone()
        .dispatch_cron_run(invalid_cron_generation)
        .await;

    clock.set_wall_time_ms(610_000);
    let [unknown_batch] = scheduler
        .claim_queue_consumers(1)
        .await
        .unwrap()
        .try_into()
        .unwrap();
    let [unknown_run] = scheduler.claim_cron(1).await.unwrap().try_into().unwrap();
    *responses.queue.lock().unwrap() = serde_json::json!({
        "outcome": "ok",
        "ackAll": false,
        "retryBatch": {"retry": false},
        "explicitAcks": [open_compute_core::QueueMessageId::generate().to_string()],
        "retryMessages": []
    });
    *responses.cron.lock().unwrap() = serde_json::json!({
        "outcome": "aborted",
        "noRetry": false
    });
    scheduler
        .clone()
        .dispatch_queue_batch(unknown_batch.clone())
        .await;
    scheduler
        .clone()
        .dispatch_cron_run(unknown_run.clone())
        .await;
    *responses.queue.lock().unwrap() = serde_json::json!({
        "outcome": "aborted",
        "ackAll": false,
        "retryBatch": {"retry": false},
        "explicitAcks": [],
        "retryMessages": []
    });
    scheduler
        .clone()
        .dispatch_queue_batch(unknown_batch.clone())
        .await;
    *responses.queue.lock().unwrap() = serde_json::json!({"outcome": "forged"});
    *responses.cron.lock().unwrap() = serde_json::json!({"outcome": "ok"});
    scheduler.clone().dispatch_queue_batch(unknown_batch).await;
    scheduler.clone().dispatch_cron_run(unknown_run).await;

    *responses.queue.lock().unwrap() = serde_json::json!({
        "outcome": "ok",
        "ackAll": true,
        "retryBatch": {"retry": false},
        "explicitAcks": [],
        "retryMessages": []
    });
    *responses.cron.lock().unwrap() = serde_json::json!({
        "outcome": "ok",
        "noRetry": false
    });
    clock.set_wall_time_ms(700_000);
    assert_eq!(scheduler.poll_once().await.unwrap(), 0);
    clock.set_wall_time_ms(701_000);
    assert!(scheduler.poll_once().await.unwrap() >= 1);
    clock.set_wall_time_ms(706_000);
    for batch in scheduler_store
        .claim_queue_batches(706_000, 60_000, 250, 1, None)
        .map(|(items, _)| items)
        .unwrap()
    {
        scheduler.clone().dispatch_queue_batch(batch).await;
    }
    for run in scheduler_store
        .claim_cron_runs(706_000, 60_000, 250, 1)
        .map(|(items, _)| items)
        .unwrap()
    {
        scheduler.clone().dispatch_cron_run(run).await;
    }
    assert_eq!(scheduler_store.queue_backlog_totals().unwrap(), (0, 0));
    *responses.cron.lock().unwrap() = serde_json::json!({
        "outcome": "exception",
        "noRetry": true
    });
    clock.set_wall_time_ms(900_000);
    let [terminal_run] = scheduler.claim_cron(1).await.unwrap().try_into().unwrap();
    scheduler.clone().dispatch_cron_run(terminal_run).await;
    assert_eq!(
        scheduler
            .inspect()
            .unwrap()
            .cron_activations
            .first()
            .and_then(|activation| activation.last_outcome.as_deref()),
        Some("failed")
    );
    *responses.cron.lock().unwrap() = serde_json::json!({
        "outcome": "ok",
        "noRetry": false
    });
    let audit_count = || {
        let connection = rusqlite::Connection::open(storage.data_dir().control_db_path()).unwrap();
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM control_audit_events", [], |row| {
                row.get(0)
            })
            .unwrap();
        u64::try_from(count).unwrap()
    };
    let audit_before = audit_count();
    for result in [
        scheduler.pause_queue_consumer_operator(
            first_consumer.id,
            2,
            open_compute_core::RequestId::generate(),
        ),
        scheduler.resume_queue_consumer_operator(
            first_consumer.id,
            2,
            open_compute_core::RequestId::generate(),
        ),
    ] {
        assert_eq!(
            result.unwrap_err().code(),
            ErrorCode::QueueConsumerGenerationStale
        );
    }
    scheduler
        .pause_queue_consumer_operator(
            first_consumer.id,
            1,
            open_compute_core::RequestId::generate(),
        )
        .unwrap();
    scheduler
        .pause_queue_consumer_operator(
            first_consumer.id,
            1,
            open_compute_core::RequestId::generate(),
        )
        .unwrap();
    assert_eq!(audit_count(), audit_before + 1);
    scheduler
        .resume_queue_consumer_operator(
            first_consumer.id,
            1,
            open_compute_core::RequestId::generate(),
        )
        .unwrap();
    scheduler
        .resume_queue_consumer_operator(
            first_consumer.id,
            1,
            open_compute_core::RequestId::generate(),
        )
        .unwrap();
    scheduler
        .pause_queue_consumer_operator(
            first_consumer.id,
            1,
            open_compute_core::RequestId::generate(),
        )
        .unwrap();
    assert_eq!(audit_count(), audit_before + 3);
    assert!(scheduler.repair_products(1_000).unwrap() >= 2);

    let second = controller
        .create_deployment(request("p23-second", "second", true, "0 * * * *", 20))
        .await
        .unwrap();
    let second_id = match second {
        CreateDeploymentOutcome::Applied(result) => result.deployment.id,
        CreateDeploymentOutcome::Replay(_) => panic!("second P2.3 deployment replayed"),
    };
    let second_consumer = consumer_repo.live_for_queue(queue_id).unwrap().unwrap();
    assert_eq!(second_consumer.consumer_generation, 2);
    assert_eq!(second_consumer.deployment_id, second_id);
    assert_eq!(
        second_consumer.state,
        open_compute_storage::QueueConsumerState::Paused
    );

    let third = controller
        .create_deployment(request("p23-third", "third", false, "30 * * * *", 30))
        .await
        .unwrap();
    let third_id = match third {
        CreateDeploymentOutcome::Applied(result) => result.deployment.id,
        CreateDeploymentOutcome::Replay(_) => panic!("third P2.3 deployment replayed"),
    };
    let third_declaration = consumer_repo
        .deployment_declarations(third_id)
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    assert!(
        consumer_repo
            .begin_update(second_consumer.id, 2, &third_declaration, 60_002)
            .unwrap()
    );
    for result in [
        scheduler.pause_queue_consumer_operator(
            second_consumer.id,
            3,
            open_compute_core::RequestId::generate(),
        ),
        scheduler.resume_queue_consumer_operator(
            second_consumer.id,
            3,
            open_compute_core::RequestId::generate(),
        ),
    ] {
        assert_eq!(
            result.unwrap_err().code(),
            ErrorCode::QueueConsumerGenerationStale
        );
    }
    assert!(scheduler.repair_products(1_000).unwrap() > 0);
    let reconciled = consumer_repo.live_for_queue(queue_id).unwrap().unwrap();
    assert_eq!(
        reconciled.state,
        open_compute_storage::QueueConsumerState::Updating
    );
    assert_eq!(reconciled.deployment_id, third_id);
    assert_eq!(reconciled.pending_deployment_id, None);
    let pre_promote_crons = open_compute_storage::CronRepository::new(storage.db())
        .live_for_worker(worker.id)
        .unwrap();
    assert_eq!(pre_promote_crons.len(), 1);
    assert_eq!(
        open_compute_storage::CronRepository::new(storage.db())
            .retire_before(
                worker.id,
                pre_promote_crons[0].activation_generation + 1,
                60_003,
            )
            .unwrap(),
        1
    );
    assert!(scheduler.repair_products(1_000).unwrap() > 0);
    workers
        .promote(
            account,
            worker.id,
            third_id,
            Some(second_id),
            open_compute_core::RequestId::generate(),
            60_003,
        )
        .unwrap();
    assert!(scheduler.repair_products(1_000).unwrap() > 0);
    assert_eq!(
        consumer_repo
            .live_for_queue(queue_id)
            .unwrap()
            .unwrap()
            .state,
        open_compute_storage::QueueConsumerState::Paused
    );
    promoter
        .promote(ProductPromotionRequest {
            account_id: account,
            worker_id: worker.id,
            deployment_id: third_id,
            request_id: open_compute_core::RequestId::generate(),
            now_ms: 60_003,
        })
        .await
        .unwrap();
    let recovered = consumer_repo.live_for_queue(queue_id).unwrap().unwrap();
    assert_eq!(recovered.consumer_generation, 3);
    assert_eq!(recovered.deployment_id, third_id);
    assert_eq!(
        recovered.state,
        open_compute_storage::QueueConsumerState::Paused
    );
    assert_eq!(
        workers
            .get_worker(account, worker.id)
            .unwrap()
            .active_deployment_id,
        Some(third_id)
    );
    let live_crons = open_compute_storage::CronRepository::new(storage.db())
        .live_for_worker(worker.id)
        .unwrap();
    assert_eq!(live_crons.len(), 1);
    assert_eq!(live_crons[0].expression, "30 * * * *");
    assert_eq!(
        live_crons[0].state,
        open_compute_storage::CronActivationState::Active
    );
    assert_eq!(
        open_compute_storage::inspect_p23_cross_database(
            &storage.data_dir().control_db_path(),
            &scheduler_path,
            100,
        )
        .unwrap(),
        open_compute_storage::P23CrossDatabaseInspection::default()
    );

    let mut retarget = request("p23-retarget", "retarget", true, "ignored", 40);
    retarget.crons = vec!["30 * * * *".to_owned()];
    let retargeted = controller.create_deployment(retarget).await.unwrap();
    let retargeted_id = match retargeted {
        CreateDeploymentOutcome::Applied(result) => result.deployment.id,
        CreateDeploymentOutcome::Replay(_) => panic!("retargeted P2.3 deployment replayed"),
    };
    let retargeted_consumer = consumer_repo.live_for_queue(queue_id).unwrap().unwrap();
    assert_eq!(retargeted_consumer.consumer_generation, 4);
    assert_eq!(retargeted_consumer.deployment_id, retargeted_id);
    let retargeted_crons = open_compute_storage::CronRepository::new(storage.db())
        .live_for_worker(worker.id)
        .unwrap();
    assert_eq!(retargeted_crons.len(), 1);
    assert_eq!(retargeted_crons[0].expression, "30 * * * *");
    assert_eq!(retargeted_crons[0].deployment_id, retargeted_id);
    let retargeted_declaration = consumer_repo
        .deployment_declarations(retargeted_id)
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let retargeted_cron_declarations = vec![open_compute_storage::CronDeclaration {
        id: open_compute_core::CronActivationId::generate(),
        deployment_id: retargeted_id,
        expression: retargeted_crons[0].expression.clone(),
        expression_sha256: retargeted_crons[0].expression_sha256,
        parser_version: retargeted_crons[0].parser_version,
        scheduled_handler: true,
        workflow_bindings: Vec::new(),
        created_at_ms: 706_002,
    }];
    assert!(
        consumer_repo
            .begin_delete(
                retargeted_consumer.id,
                retargeted_consumer.consumer_generation,
                706_001,
            )
            .unwrap()
    );
    assert_eq!(
        open_compute_storage::CronRepository::new(storage.db())
            .retire_before(
                worker.id,
                retargeted_crons[0].activation_generation + 1,
                706_001,
            )
            .unwrap(),
        1
    );
    assert!(scheduler.repair_products(1_000).unwrap() >= 2);
    assert!(consumer_repo.live_for_queue(queue_id).unwrap().is_none());
    assert!(
        open_compute_storage::CronRepository::new(storage.db())
            .live_for_worker(worker.id)
            .unwrap()
            .is_empty()
    );
    let reactivated = consumer_repo
        .create_attachment(account, worker.id, &retargeted_declaration, 706_002)
        .unwrap();
    let restaged = open_compute_storage::CronRepository::new(storage.db())
        .stage_activations(
            account,
            worker.id,
            retargeted_id,
            retargeted_crons[0].activation_generation + 1,
            &retargeted_cron_declarations,
            706_002,
        )
        .unwrap();
    assert_eq!(restaged.len(), 1);
    assert!(scheduler.repair_products(1_000).unwrap() >= 2);
    assert_eq!(
        consumer_repo.get(reactivated.id).unwrap().state,
        open_compute_storage::QueueConsumerState::Active
    );
    assert_eq!(
        open_compute_storage::CronRepository::new(storage.db())
            .live_for_worker(worker.id)
            .unwrap()[0]
            .state,
        open_compute_storage::CronActivationState::Active
    );
    assert!(
        consumer_repo
            .begin_update(
                reactivated.id,
                reactivated.consumer_generation,
                &retargeted_declaration,
                706_003,
            )
            .unwrap()
    );
    assert!(scheduler.repair_products(1_000).unwrap() >= 2);
    let reactivated = consumer_repo.get(reactivated.id).unwrap();
    assert_eq!(reactivated.consumer_generation, 2);
    assert_eq!(
        reactivated.state,
        open_compute_storage::QueueConsumerState::Active
    );
    assert!(
        consumer_repo
            .begin_delete(reactivated.id, reactivated.consumer_generation, 706_004)
            .unwrap()
    );
    assert_eq!(
        open_compute_storage::CronRepository::new(storage.db())
            .retire_before(worker.id, restaged[0].activation_generation + 1, 706_004,)
            .unwrap(),
        1
    );
    assert!(scheduler.repair_products(1_000).unwrap() >= 2);

    let mut empty = request("p23-empty", "empty", true, "ignored", 10);
    empty.queue_consumers.clear();
    empty.crons = Vec::new();
    let emptied = controller.create_deployment(empty).await.unwrap();
    let emptied_id = match emptied {
        CreateDeploymentOutcome::Applied(result) => result.deployment.id,
        CreateDeploymentOutcome::Replay(_) => panic!("empty P2.3 deployment replayed"),
    };
    assert_eq!(
        workers
            .get_worker(account, worker.id)
            .unwrap()
            .active_deployment_id,
        Some(emptied_id)
    );
    assert!(consumer_repo.live_for_queue(queue_id).unwrap().is_none());
    assert!(
        open_compute_storage::CronRepository::new(storage.db())
            .live_for_worker(worker.id)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        open_compute_storage::inspect_p23_cross_database(
            &storage.data_dir().control_db_path(),
            &scheduler_path,
            100,
        )
        .unwrap(),
        open_compute_storage::P23CrossDatabaseInspection::default()
    );
    custom_event_task.abort();
    let _ = custom_event_task.await;
}

#[tokio::test]
async fn worker_http_boundaries_reject_malformed_ids_keys_and_bodies() {
    let (dir, mock, state, account) = initialized_worker_http_fixture().await;
    let app = http::admin_router(state.clone());
    let worker = open_compute_core::WorkerId::generate();
    let deployment = open_compute_core::DeploymentId::generate();
    let malformed = [
        ("POST", "/v1/accounts/bad/workers".to_owned()),
        ("GET", "/v1/accounts/bad/workers".to_owned()),
        ("GET", format!("/v1/accounts/{account}/workers/bad")),
        ("DELETE", format!("/v1/accounts/{account}/workers/bad")),
        (
            "POST",
            format!("/v1/accounts/{account}/workers/bad/deployments"),
        ),
        (
            "GET",
            format!("/v1/accounts/{account}/workers/bad/deployments"),
        ),
        (
            "GET",
            format!("/v1/accounts/{account}/workers/{worker}/deployments/bad"),
        ),
        (
            "DELETE",
            format!("/v1/accounts/{account}/workers/{worker}/deployments/bad"),
        ),
        (
            "POST",
            format!("/v1/accounts/{account}/workers/bad/promotions"),
        ),
        (
            "POST",
            format!("/v1/accounts/{account}/workers/bad/rollbacks"),
        ),
        ("POST", format!("/v1/accounts/{account}/workers/bad/routes")),
        ("GET", format!("/v1/accounts/{account}/workers/bad/routes")),
        (
            "DELETE",
            format!("/v1/accounts/{account}/workers/bad/routes/route"),
        ),
    ];

    let unauthorized = [
        ("POST", format!("/v1/accounts/{account}/workers")),
        ("GET", format!("/v1/accounts/{account}/workers")),
        ("GET", format!("/v1/accounts/{account}/workers/{worker}")),
        ("DELETE", format!("/v1/accounts/{account}/workers/{worker}")),
        (
            "POST",
            format!("/v1/accounts/{account}/workers/{worker}/deployments"),
        ),
        (
            "GET",
            format!("/v1/accounts/{account}/workers/{worker}/deployments"),
        ),
        (
            "GET",
            format!("/v1/accounts/{account}/workers/{worker}/deployments/{deployment}"),
        ),
        (
            "DELETE",
            format!("/v1/accounts/{account}/workers/{worker}/deployments/{deployment}"),
        ),
        (
            "POST",
            format!("/v1/accounts/{account}/workers/{worker}/promotions"),
        ),
        (
            "POST",
            format!("/v1/accounts/{account}/workers/{worker}/rollbacks"),
        ),
        (
            "POST",
            format!("/v1/accounts/{account}/workers/{worker}/routes"),
        ),
        (
            "GET",
            format!("/v1/accounts/{account}/workers/{worker}/routes"),
        ),
        (
            "DELETE",
            format!("/v1/accounts/{account}/workers/{worker}/routes/route"),
        ),
    ];
    for (method, path) in unauthorized {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    for (method, path) in malformed {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header("authorization", "Bearer admin-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    let missing_key = [
        ("POST", format!("/v1/accounts/{account}/workers")),
        ("DELETE", format!("/v1/accounts/{account}/workers/{worker}")),
        (
            "POST",
            format!("/v1/accounts/{account}/workers/{worker}/deployments"),
        ),
        (
            "DELETE",
            format!("/v1/accounts/{account}/workers/{worker}/deployments/{deployment}"),
        ),
        (
            "POST",
            format!("/v1/accounts/{account}/workers/{worker}/promotions"),
        ),
        (
            "POST",
            format!("/v1/accounts/{account}/workers/{worker}/rollbacks"),
        ),
        (
            "POST",
            format!("/v1/accounts/{account}/workers/{worker}/routes"),
        ),
        (
            "DELETE",
            format!("/v1/accounts/{account}/workers/{worker}/routes/route"),
        ),
    ];
    for (method, path) in missing_key {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header("authorization", "Bearer admin-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    for path in [
        format!("/v1/accounts/{account}/workers"),
        format!("/v1/accounts/{account}/workers/{worker}/promotions"),
        format!("/v1/accounts/{account}/workers/{worker}/rollbacks"),
        format!("/v1/accounts/{account}/workers/{worker}/routes"),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header("authorization", "Bearer admin-token")
                    .header("idempotency-key", "test-key")
                    .body(Body::from("{"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/v1/accounts/{account}/workers/{worker}/deployments"
                ))
                .header("authorization", "Bearer admin-token")
                .header("idempotency-key", "test-key")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/v1/accounts/{account}/workers/{worker}/deployments"
                ))
                .header("authorization", "Bearer admin-token")
                .header("idempotency-key", "too-large")
                .header(
                    "x-open-compute-deployment-metadata",
                    r#"{"mainModule":"index.js"}"#,
                )
                .header("content-length", (25 * 1024 * 1024 + 1).to_string())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let public = http::public_router(state);
    let response = public
        .clone()
        .oneshot(
            Request::builder()
                .uri("/missing")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let response = public
        .oneshot(
            Request::builder()
                .uri("/missing")
                .header("host", "unknown.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(mock.object_count(), 0);
    let _ = dir;
}

#[tokio::test]
async fn worker_http_crud_replay_and_runtime_failure_paths() {
    let (_dir, mock, state, account) = initialized_worker_http_fixture().await;
    let app = http::admin_router(state.clone());
    let auth = "Bearer admin-token";
    let workers_path = format!("/v1/accounts/{account}/workers");

    let create = || {
        Request::builder()
            .method("POST")
            .uri(&workers_path)
            .header("authorization", auth)
            .header("idempotency-key", "worker-create")
            .body(Body::from(r#"{"name":"coverage-worker"}"#))
            .unwrap()
    };
    let response = app.clone().oneshot(create()).await.unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let bytes = axum::body::to_bytes(response.into_body(), 16_384)
        .await
        .unwrap();
    let created: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let worker = created["worker"]["id"].as_str().unwrap().to_owned();

    let replay = app.clone().oneshot(create()).await.unwrap();
    assert_eq!(replay.status(), StatusCode::CREATED);
    let conflict = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&workers_path)
                .header("authorization", auth)
                .header("idempotency-key", "worker-create")
                .body(Body::from(r#"{"name":"different"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(conflict.status(), StatusCode::CONFLICT);

    for path in [
        workers_path.clone(),
        format!("{workers_path}/{worker}"),
        format!("{workers_path}/{worker}/deployments"),
        format!("{workers_path}/{worker}/routes"),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header("authorization", auth)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    let route_path = format!("{workers_path}/{worker}/routes");
    let route_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&route_path)
                .header("authorization", auth)
                .header("idempotency-key", "route-create")
                .body(Body::from(
                    r#"{"hostname":"Api.Example.com.","pathPrefix":"/api"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(route_response.status(), StatusCode::CREATED);
    let route_bytes = axum::body::to_bytes(route_response.into_body(), 16_384)
        .await
        .unwrap();
    let route_json: serde_json::Value = serde_json::from_slice(&route_bytes).unwrap();
    let route = route_json["route"]["id"].as_str().unwrap().to_owned();

    let named_route = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&route_path)
                .header("authorization", auth)
                .header("idempotency-key", "route-named")
                .body(Body::from(
                    r#"{"hostname":"named.example.com","pathPrefix":"/","entrypoint":"named"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(named_route.status(), StatusCode::CONFLICT);

    for (key, body) in [
        (
            "route-invalid-host",
            r#"{"hostname":"bad host","pathPrefix":"/"}"#,
        ),
        (
            "route-invalid-path",
            r#"{"hostname":"valid.example","pathPrefix":"relative"}"#,
        ),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(&route_path)
                    .header("authorization", auth)
                    .header("idempotency-key", key)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("{route_path}/missing-route"))
                .header("authorization", auth)
                .header("idempotency-key", "route-delete-missing")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let missing_account = open_compute_core::AccountId::generate();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/accounts/{missing_account}/workers"))
                .header("authorization", auth)
                .header("idempotency-key", "worker-missing-account")
                .body(Body::from(r#"{"name":"missing-account"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let missing_worker = open_compute_core::WorkerId::generate();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("{workers_path}/{missing_worker}"))
                .header("authorization", auth)
                .header("idempotency-key", "worker-delete-missing")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let bundle = CanonicalBundle::build(
        "index.js",
        vec![ModuleInput {
            name: "index.js".to_owned(),
            module_type: ModuleType::EsModule,
            bytes: b"export default { fetch() { return new Response('ok'); } };".to_vec(),
        }],
        BundleLimits::default(),
    )
    .unwrap()
    .into_bytes();
    let deployments_path = format!("{workers_path}/{worker}/deployments");
    let invalid_bundle = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&deployments_path)
                .header("authorization", auth)
                .header("idempotency-key", "deployment-invalid-bundle")
                .header(
                    "x-open-compute-deployment-metadata",
                    r#"{"mainModule":"index.js"}"#,
                )
                .body(Body::from("not-a-canonical-bundle"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(invalid_bundle.status(), StatusCode::BAD_REQUEST);
    let mismatch = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&deployments_path)
                .header("authorization", auth)
                .header("idempotency-key", "deployment-mismatch")
                .header(
                    "x-open-compute-deployment-metadata",
                    r#"{"mainModule":"other.js"}"#,
                )
                .body(Body::from(bundle.clone()))
                .unwrap(),
        )
        .await
        .unwrap();
    let mismatch_status = mismatch.status();
    let mismatch_body = axum::body::to_bytes(mismatch.into_body(), 16_384)
        .await
        .unwrap();
    assert_eq!(
        mismatch_status,
        StatusCode::BAD_REQUEST,
        "body={}",
        String::from_utf8_lossy(&mismatch_body)
    );

    let deployment_request = || {
        Request::builder()
            .method("POST")
            .uri(&deployments_path)
            .header("authorization", auth)
            .header("idempotency-key", "deployment-runtime-failure")
            .header(
                "x-open-compute-deployment-metadata",
                r#"{"mainModule":"index.js"}"#,
            )
            .body(Body::from(bundle.clone()))
            .unwrap()
    };
    let rejected = app.clone().oneshot(deployment_request()).await.unwrap();
    assert_eq!(rejected.status(), StatusCode::SERVICE_UNAVAILABLE);
    let replayed_rejection = app.clone().oneshot(deployment_request()).await.unwrap();
    assert_eq!(replayed_rejection.status(), StatusCode::SERVICE_UNAVAILABLE);

    let promote_path = format!("{workers_path}/{worker}/promotions");
    let missing_deployment = open_compute_core::DeploymentId::generate();
    let promotion_body = serde_json::json!({
        "targetDeploymentId": missing_deployment,
        "expectedActiveDeploymentId": null,
    })
    .to_string();
    for _ in 0..2 {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(&promote_path)
                    .header("authorization", auth)
                    .header("idempotency-key", "promotion-missing")
                    .body(Body::from(promotion_body.clone()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    let public = http::public_router(state);
    let ingress = public
        .oneshot(
            Request::builder()
                .uri("/api/item")
                .header("host", "api.example.com")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ingress.status(), StatusCode::NOT_FOUND);

    let delete_route_path = format!("{route_path}/{route}");
    for _ in 0..2 {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(&delete_route_path)
                    .header("authorization", auth)
                    .header("idempotency-key", "route-delete")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
    }

    let delete_worker_path = format!("{workers_path}/{worker}");
    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(delete_worker_path)
                .header("authorization", auth)
                .header("idempotency-key", "worker-delete")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert!(mock.object_count() <= 1);
}

#[tokio::test]
async fn initialized_basic_doctor_is_read_only_and_head_only() {
    let (dir, path, mock) = initialized_doctor_fixture().await;
    let data = dir.path().join("data");
    let before = content_snapshot(&data);
    let wal = data.join("control.sqlite-wal");
    let shm = data.join("control.sqlite-shm");
    assert!(!wal.exists());
    let loaded = load_platform_config(&path).unwrap();
    let report = doctor_report(&loaded, DoctorMode::Basic).await;
    assert_eq!(content_snapshot(&data), before);
    assert!(!wal.exists());
    assert!(!shm.exists());
    let methods: Vec<_> = mock.recorded().into_iter().map(|r| r.method).collect();
    assert!(methods.iter().all(|m| m == "HEAD"), "{methods:?}");
    assert!(!methods.is_empty());
    assert_eq!(check(&report, "s3_canary").status, CheckStatus::Skipped);
    let _ = dir;
}

#[tokio::test]
async fn doctor_reports_key_mismatch_and_env_only_key() {
    let (dir, path, _mock) = initialized_doctor_fixture().await;
    let loaded = load_platform_config(&path).unwrap();
    let other = encode_master_key(&[7u8; 32]);
    write_mode(&loaded.config.storage.master_key_file, &other, 0o600);
    let report = doctor_report(&loaded, DoctorMode::Basic).await;
    assert_eq!(check(&report, "master_key").status, CheckStatus::Failed);
    assert_eq!(
        check(&report, "master_key").code,
        Some("MASTER_KEY_MISMATCH")
    );

    open_compute_storage::set_test_env("OC_TEST_MASTER_KEY_ONLY", &encode_master_key(&[9u8; 32]));
    let mut cfg = loaded.config.storage.clone();
    cfg.master_key_env = Some("OC_TEST_MASTER_KEY_ONLY".into());
    cfg.master_key_file = dir.path().join("missing-master.key");
    open_compute_storage::inspect_master_key(&cfg).expect("env-only key is readable");
    open_compute_storage::clear_test_env();
}

#[tokio::test]
async fn doctor_rejects_future_schema_and_sha256_symlink_and_corrupt_cache() {
    let (dir, path, _mock) = initialized_doctor_fixture().await;
    let data = dir.path().join("data");
    let db = data.join("control.sqlite");
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.pragma_update(None, "user_version", 99).unwrap();
    }
    let loaded = load_platform_config(&path).unwrap();
    let report = doctor_report(&loaded, DoctorMode::Basic).await;
    assert_eq!(check(&report, "sqlite").status, CheckStatus::Failed);

    let (dir, path, _mock) = initialized_doctor_fixture().await;
    let cache = dir.path().join("data/cache/artifacts");
    let sha = cache.join("sha256");
    let _ = fs::remove_dir_all(&sha);
    std::os::unix::fs::symlink("/tmp", &sha).unwrap();
    let loaded = load_platform_config(&path).unwrap();
    let report = doctor_report(&loaded, DoctorMode::Basic).await;
    assert_eq!(
        check(&report, "cache_integrity").status,
        CheckStatus::Failed
    );

    let (dir, path, _mock) = initialized_doctor_fixture().await;
    let digest = "ab".repeat(32);
    let shard = dir
        .path()
        .join("data/cache/artifacts/sha256")
        .join(&digest[..2]);
    fs::create_dir_all(&shard).unwrap();
    let entry = shard.join(&digest[2..]);
    fs::write(&entry, b"corrupt-bytes").unwrap();
    let before_meta = fs::symlink_metadata(&entry).unwrap();
    let before_bytes = fs::read(&entry).unwrap();
    let loaded = load_platform_config(&path).unwrap();
    let report = doctor_report(&loaded, DoctorMode::Basic).await;
    assert_eq!(
        check(&report, "cache_integrity").status,
        CheckStatus::Failed
    );
    assert_eq!(fs::read(&entry).unwrap(), before_bytes);
    let after = fs::symlink_metadata(&entry).unwrap();
    assert_eq!(after.len(), before_meta.len());
    assert_eq!(after.modified().ok(), before_meta.modified().ok());
}

#[tokio::test]
async fn fail_after_stages_release_lock_and_ports() {
    let mock = open_compute_artifacts::MockS3::spawn("open-compute").await;
    let dir = TempDir::new().unwrap();
    let ak = dir.path().join("ak");
    let sk = dir.path().join("sk");
    write_mode(&ak, "AKIAEXAMPLEKEYID01", 0o600);
    write_mode(&sk, "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY", 0o600);
    let extra = format!(
        r#"
[s3]
endpoint = "{}"
region = "us-east-1"
bucket = "open-compute"
force_path_style = true
access_key_id_file = "{}"
secret_access_key_file = "{}"
verify_tls = true
prefix = "system/"
max_retries = 1
retry_backoff_ms = 10
connect_timeout_ms = 500
request_timeout_ms = 2000
"#,
        mock.endpoint,
        ak.display(),
        sk.display()
    );
    let path = write_config(dir.path(), &extra);
    let loaded = load_platform_config(&path).unwrap();
    for stage in [
        FailAfter::Config,
        FailAfter::Storage,
        FailAfter::RuntimeVerify,
        FailAfter::S3,
        FailAfter::Cache,
        FailAfter::Compile,
        FailAfter::Listen,
    ] {
        let opts = RunOptions {
            fail_after: Some(stage),
            ..RunOptions::default()
        };
        let err = run_platform_with(loaded.clone(), opts.clone())
            .await
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::ConfigInvalid);
        let stages = opts.stages.lock().unwrap().clone();
        let expected: &[&str] = match stage {
            FailAfter::Config => &["config"],
            FailAfter::Storage => &["config", "storage"],
            FailAfter::RuntimeVerify => &["config", "storage", "runtime_verify"],
            FailAfter::S3 => &["config", "storage", "runtime_verify", "s3"],
            FailAfter::Cache => &["config", "storage", "runtime_verify", "s3", "cache"],
            FailAfter::Compile => &[
                "config",
                "storage",
                "runtime_verify",
                "s3",
                "cache",
                "compile",
            ],
            FailAfter::Listen => &[
                "config",
                "storage",
                "runtime_verify",
                "s3",
                "cache",
                "compile",
                "listen",
            ],
        };
        assert_eq!(stages, expected, "fail point {stage:?}");
        let addr = *opts.last_public_addr.lock().unwrap();
        if let Some(addr) = addr {
            let _rebind = tokio::net::TcpListener::bind(addr).await.expect("rebind");
        }
        open_compute_storage::PlatformStorage::bootstrap(
            &loaded.config.storage,
            &open_compute_core::SystemClock,
        )
        .expect("lock reacquired");
        assert_eq!(mock.object_count(), 0);
    }
}

#[tokio::test]
async fn full_doctor_uses_embedded_workerd() {
    let (dir, _path, mock) = initialized_doctor_fixture().await;
    let extra = r#"
[runtime]
startup_timeout_ms = 20000
shutdown_grace_ms = 5000
drain_timeout_ms = 5000
kill_timeout_ms = 2000
"#;
    let ak = dir.path().join("ak");
    let sk = dir.path().join("sk");
    write_mode(&ak, "AKIAEXAMPLEKEYID01", 0o600);
    write_mode(&sk, "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY", 0o600);
    let s3 = format!(
        r#"
[s3]
endpoint = "{}"
region = "us-east-1"
bucket = "open-compute"
force_path_style = true
access_key_id_file = "{}"
secret_access_key_file = "{}"
verify_tls = true
prefix = "system/"
max_retries = 1
retry_backoff_ms = 10
connect_timeout_ms = 500
request_timeout_ms = 5000
"#,
        mock.endpoint,
        ak.display(),
        sk.display()
    );
    let path = write_config(dir.path(), &format!("{s3}\n{extra}"));
    let loaded = load_platform_config(&path).unwrap();
    assert!(
        loaded
            .config
            .storage
            .data_dir
            .join("control.sqlite")
            .exists()
    );
    let report = doctor_report(&loaded, DoctorMode::Full).await;
    assert_eq!(
        check(&report, "runtime_cycle").status,
        CheckStatus::Ok,
        "{:?}",
        report.checks
    );
    assert_eq!(check(&report, "s3_canary").status, CheckStatus::Ok);
    assert_eq!(mock.object_count(), 0);
}

#[tokio::test]
async fn doctor_skips_db_when_platform_lock_is_held() {
    let (dir, path, mock) = initialized_doctor_fixture().await;
    let loaded = load_platform_config(&path).unwrap();
    let _storage = open_compute_storage::PlatformStorage::bootstrap(
        &loaded.config.storage,
        &open_compute_core::SystemClock,
    )
    .expect("hold lock");
    let before = content_snapshot(&loaded.config.storage.data_dir);
    let report = doctor_report(&loaded, DoctorMode::Full).await;
    assert_eq!(check(&report, "lock").status, CheckStatus::Failed);
    assert_eq!(check(&report, "lock").code, Some("DATA_DIR_IN_USE"));
    assert_eq!(check(&report, "sqlite").status, CheckStatus::Skipped);
    assert_eq!(check(&report, "schema").status, CheckStatus::Skipped);
    assert_eq!(check(&report, "identity").status, CheckStatus::Skipped);
    assert_eq!(
        check(&report, "cache_integrity").status,
        CheckStatus::Skipped
    );
    assert_eq!(check(&report, "s3_canary").status, CheckStatus::Skipped);
    assert_eq!(check(&report, "runtime_cycle").status, CheckStatus::Skipped);
    assert_eq!(content_snapshot(&loaded.config.storage.data_dir), before);
    assert_eq!(mock.object_count(), 0);
    let _ = dir;
}

#[tokio::test]
async fn doctor_reports_limits_space_and_full_prerequisite_failures() {
    let (dir, path, _mock) = initialized_doctor_fixture().await;
    let mut loaded = load_platform_config(&path).unwrap();
    loaded.config.metrics.max_series = 1;
    loaded.config.storage.free_space_hard_bytes = u64::MAX;
    loaded.config.storage.free_space_soft_bytes = u64::MAX;
    let report = doctor_report(&loaded, DoctorMode::Basic).await;
    assert_eq!(check(&report, "config").status, CheckStatus::Failed);
    assert_eq!(check(&report, "free_space").status, CheckStatus::Failed);

    let mut loaded = load_platform_config(&path).unwrap();
    loaded.config.storage.free_space_hard_bytes = 0;
    loaded.config.storage.free_space_soft_bytes = u64::MAX;
    let report = doctor_report(&loaded, DoctorMode::Basic).await;
    assert_eq!(check(&report, "free_space").status, CheckStatus::Warning);

    let package = open_compute_runtime::materialize_embedded_runtime(
        &loaded.config.storage.data_dir.join("runtime"),
    )
    .unwrap();
    let asset = package.assets_dir().join("config.capnp");
    fs::set_permissions(&asset, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&asset, b"tampered").unwrap();
    let report = doctor_report(&loaded, DoctorMode::Full).await;
    assert_eq!(check(&report, "runtime_binary").status, CheckStatus::Failed);
    assert_eq!(check(&report, "runtime_cycle").status, CheckStatus::Failed);
    let _ = dir;
}

#[tokio::test]
async fn full_doctor_reports_s3_canary_failure_without_leaking_objects() {
    let (dir, path, mock) = initialized_doctor_fixture().await;
    let loaded = load_platform_config(&path).unwrap();
    mock.set_fault(open_compute_artifacts::Fault::Permission);
    let report = doctor_report(&loaded, DoctorMode::Full).await;
    assert_eq!(
        check(&report, "s3_connectivity").status,
        CheckStatus::Failed
    );
    assert_eq!(check(&report, "s3_canary").status, CheckStatus::Failed);
    assert_eq!(mock.object_count(), 0);
    let _ = dir;
}

#[tokio::test]
async fn run_startup_failure_matrix_releases_owned_resources() {
    let (dir, path, mock) = initialized_doctor_fixture().await;
    let base = load_platform_config(&path).unwrap();

    let mut loaded = base.clone();
    loaded.config.metrics.max_series = 1;
    assert_eq!(
        run_platform_with(loaded, RunOptions::default())
            .await
            .unwrap_err()
            .code(),
        ErrorCode::LimitInvalid
    );

    let mut loaded = base.clone();
    loaded.config.storage.data_dir = dir.path().join("not-a-directory");
    fs::write(&loaded.config.storage.data_dir, b"file").unwrap();
    assert_eq!(
        run_platform_with(loaded, RunOptions::default())
            .await
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );

    let package = open_compute_runtime::materialize_embedded_runtime(
        &base.config.storage.data_dir.join("runtime"),
    )
    .unwrap();
    let asset = package.assets_dir().join("config.capnp");
    let original_runtime = fs::read(&asset).unwrap();
    fs::set_permissions(&asset, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&asset, b"tampered").unwrap();
    assert_eq!(
        run_platform_with(base.clone(), RunOptions::default())
            .await
            .unwrap_err()
            .code(),
        ErrorCode::RuntimeInvalid
    );
    fs::write(&asset, original_runtime).unwrap();
    fs::set_permissions(&asset, fs::Permissions::from_mode(0o400)).unwrap();

    let mut loaded = base.clone();
    loaded.config.s3.access_key_id_env = Some(format!(
        "OPEN_COMPUTE_MISSING_RUN_KEY_{}",
        std::process::id()
    ));
    loaded.config.s3.access_key_id_file = None;
    assert_eq!(
        run_platform_with(loaded, RunOptions::default())
            .await
            .unwrap_err()
            .code(),
        ErrorCode::SecretRefInvalid
    );

    let mut loaded = base.clone();
    loaded.config.s3.verify_tls = false;
    assert_eq!(
        run_platform_with(loaded, RunOptions::default())
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ConfigInvalid
    );

    mock.set_fault(open_compute_artifacts::Fault::Permission);
    assert!(
        run_platform_with(base.clone(), RunOptions::default())
            .await
            .is_err()
    );
    mock.set_fault(open_compute_artifacts::Fault::None);

    let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let occupied_addr = occupied.local_addr().unwrap();
    let mut loaded = base.clone();
    loaded.config.server.public_bind = occupied_addr.to_string();
    loaded.config.server.admin_bind = Some(occupied_addr.to_string());
    assert_eq!(
        run_platform_with(loaded, RunOptions::default())
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ConfigInvalid
    );
    drop(occupied);

    let occupied_admin = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut loaded = base;
    loaded.config.server.public_bind = "127.0.0.1:0".to_owned();
    loaded.config.server.admin_bind = Some(occupied_admin.local_addr().unwrap().to_string());
    assert_eq!(
        run_platform_with(loaded, RunOptions::default())
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ConfigInvalid
    );

    open_compute_storage::PlatformStorage::bootstrap(
        &load_platform_config(&path).unwrap().config.storage,
        &open_compute_core::SystemClock,
    )
    .expect("all startup failures released the data-dir lock");
}

#[tokio::test]
async fn run_real_workerd_with_separate_admin_listener_and_maintenance_tick() {
    let (_dir, path, mock) = initialized_doctor_fixture().await;
    let mut loaded = load_platform_config(&path).unwrap();
    loaded.config.runtime.startup_timeout_ms = 60_000;
    loaded.config.runtime.shutdown_grace_ms = 1_000;
    loaded.config.runtime.kill_timeout_ms = 2_000;
    loaded.config.workers.artifact_gc_interval_ms = 20;
    loaded.config.workers.deployment_min_retention_ms = 0;
    loaded.config.workers.retain_rejected_deployments = 1;

    {
        let storage = open_compute_storage::PlatformStorage::bootstrap(
            &loaded.config.storage,
            &open_compute_core::SystemClock,
        )
        .unwrap();
        let repo = open_compute_storage::WorkerRepository::new(storage.db());
        let account = storage.identity().default_account_id;
        let (worker, _) = repo
            .create_worker(
                account,
                "maintenance-worker",
                open_compute_core::RequestId::generate(),
                1,
                1_000_000,
            )
            .unwrap();
        for (index, timestamp) in [(1_u8, 2_i64), (2, 3)] {
            let deployment = open_compute_core::DeploymentId::generate();
            repo.insert_staging_deployment(
                &open_compute_storage::NewDeployment {
                    id: deployment,
                    account_id: account,
                    worker_id: worker.id,
                    content_kind: open_compute_storage::DeploymentContentKind::Worker,
                    artifact_sha256: Some([index; 32]),
                    artifact_size: Some(u64::from(index)),
                    artifact_schema_version: Some(1),
                    main_module: Some("index.js".to_owned()),
                    worker_code_sha256: [index.saturating_add(10); 32],
                    vars: std::collections::BTreeMap::new(),
                    secrets: std::collections::BTreeMap::new(),
                    request_id: open_compute_core::RequestId::generate(),
                    now_ms: timestamp,
                },
                &open_compute_storage::NewDeploymentProducts::default(),
                1_000_000,
            )
            .unwrap();
            repo.mark_rejected(
                deployment,
                open_compute_storage::DeploymentState::Staging,
                ErrorCode::BundleInvalid,
                timestamp,
            )
            .unwrap();
        }
    }

    let reserved = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let admin_addr = reserved.local_addr().unwrap();
    drop(reserved);
    loaded.config.server.admin_bind = Some(admin_addr.to_string());

    let options = RunOptions::default();
    let addresses = options.last_public_addr.clone();
    let mut task = tokio::spawn(run_platform_with(loaded, options));
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            if addresses.lock().unwrap().is_some() {
                break;
            }
            if task.is_finished() {
                panic!("platform startup ended early: {:?}", (&mut task).await);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    rustix::process::kill_process(rustix::process::getpid(), rustix::process::Signal::TERM)
        .unwrap();
    tokio::time::timeout(Duration::from_secs(60), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(mock.object_count(), 0);
}

#[tokio::test]
async fn worker_artifact_gc_skips_when_final_reference_snapshot_fails() {
    let (_dir, path, mock) = initialized_doctor_fixture().await;
    let loaded = load_platform_config(&path).unwrap();
    let storage = Arc::new(
        open_compute_storage::PlatformStorage::bootstrap(
            &loaded.config.storage,
            &open_compute_core::SystemClock,
        )
        .unwrap(),
    );
    let credentials = open_compute_artifacts::resolve_s3_credentials(&loaded.config.s3).unwrap();
    let client = open_compute_artifacts::S3ArtifactClient::connect(
        &loaded.config.s3,
        &credentials,
        loaded.config.cache.max_artifact_bytes,
    )
    .unwrap();
    let store = open_compute_artifacts::ArtifactStore::new(client);
    let payload = bytes::Bytes::from_static(b"unreferenced-old-artifact");
    let digest = hex::encode(sha2::Sha256::digest(&payload));
    store
        .put_verified(
            futures::stream::iter(vec![Ok::<_, std::io::Error>(payload.clone())]),
            &digest,
            payload.len() as u64,
        )
        .await
        .unwrap();
    storage.db().set_foreign_keys_for_test(false).unwrap();
    let mut workers = loaded.config.workers;
    workers.artifact_gc_grace_ms = 0;
    gc_worker_artifacts(
        &storage,
        &store,
        &workers,
        &crate::snapshot_pins::SnapshotPins::empty(),
        None,
    )
    .await
    .unwrap_err();
    assert_eq!(mock.object_count(), 1);
    storage.db().set_foreign_keys_for_test(true).unwrap();
}

#[tokio::test]
async fn reused_old_artifact_commit_precedes_gc_reference_snapshot() {
    let (_dir, path, mock) = initialized_doctor_fixture().await;
    let loaded = load_platform_config(&path).unwrap();
    let storage = Arc::new(
        open_compute_storage::PlatformStorage::bootstrap(
            &loaded.config.storage,
            &open_compute_core::SystemClock,
        )
        .unwrap(),
    );
    let credentials = open_compute_artifacts::resolve_s3_credentials(&loaded.config.s3).unwrap();
    let client = open_compute_artifacts::S3ArtifactClient::connect(
        &loaded.config.s3,
        &credentials,
        loaded.config.cache.max_artifact_bytes,
    )
    .unwrap();
    let store = open_compute_artifacts::ArtifactStore::new(client);
    let payload = bytes::Bytes::from_static(b"reused-old-artifact");
    let digest: [u8; 32] = sha2::Sha256::digest(&payload).into();
    let digest_hex = hex::encode(digest);
    store
        .put_verified(
            futures::stream::iter(vec![Ok::<_, std::io::Error>(payload.clone())]),
            &digest_hex,
            payload.len() as u64,
        )
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(2)).await;

    let repo = open_compute_storage::WorkerRepository::new(storage.db());
    let account = storage.identity().default_account_id;
    let (worker, _) = repo
        .create_worker(
            account,
            "gc-reference-fence",
            open_compute_core::RequestId::generate(),
            1,
            1_000_000,
        )
        .unwrap();
    let reservation = store.reserve_deployment_artifact().await;
    let mut workers = loaded.config.workers;
    workers.artifact_gc_grace_ms = 0;
    let gc_storage = storage.clone();
    let gc_store = store.clone();
    let mut gc = tokio::spawn(async move {
        gc_worker_artifacts(
            &gc_storage,
            &gc_store,
            &workers,
            &crate::snapshot_pins::SnapshotPins::empty(),
            None,
        )
        .await
        .unwrap();
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut gc)
            .await
            .is_err(),
        "GC must wait for the deployment commit reservation"
    );
    let deployment = open_compute_core::DeploymentId::generate();
    repo.insert_staging_deployment(
        &open_compute_storage::NewDeployment {
            id: deployment,
            account_id: account,
            worker_id: worker.id,
            content_kind: open_compute_storage::DeploymentContentKind::Worker,
            artifact_sha256: Some(digest),
            artifact_size: Some(payload.len() as u64),
            artifact_schema_version: Some(1),
            main_module: Some("index.js".to_owned()),
            worker_code_sha256: [7; 32],
            vars: std::collections::BTreeMap::new(),
            secrets: std::collections::BTreeMap::new(),
            request_id: open_compute_core::RequestId::generate(),
            now_ms: 2,
        },
        &open_compute_storage::NewDeploymentProducts::default(),
        1_000_000,
    )
    .unwrap();
    drop(reservation);
    tokio::time::timeout(Duration::from_secs(1), gc)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(mock.object_count(), 1);
}

#[tokio::test]
async fn kv_maintenance_gc_skip_checkpoint_and_corruption_isolation() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data");
    let storage = Arc::new(
        open_compute_storage::PlatformStorage::bootstrap(
            &open_compute_core::StorageConfig {
                data_dir: root.clone(),
                master_key_file: root.join("keys/master.key"),
                master_key_env: None,
                sqlite_busy_timeout_ms: 5_000,
                free_space_soft_bytes: 1_073_741_824,
                free_space_hard_bytes: 268_435_456,
            },
            &open_compute_core::SystemClock,
        )
        .unwrap(),
    );
    let account = storage.identity().default_account_id;
    let pins = open_compute_workers::ResourcePins::new();
    let created = open_compute_workers::ResourceController::new(
        &storage,
        pins.clone(),
        open_compute_workers::KvResourceDriver::new(&storage, 256 * 1024 * 1024),
    )
    .create(&open_compute_workers::CreateResourceRequest {
        account_id: account,
        kind: open_compute_core::BindingKind::KvNamespace,
        name: "maintenance".to_owned(),
        idempotency_key: "maintenance-create".to_owned(),
        driver_schema_version: 1,
        request_id: open_compute_core::RequestId::generate(),
        now_ms: 1,
    })
    .unwrap();
    let resource = match created {
        open_compute_workers::CreateResourceOutcome::Applied(value) => value.resource_id,
        open_compute_workers::CreateResourceOutcome::Replay(_) => unreachable!(),
    };
    let catalog = open_compute_storage::KvNamespaceRepository::new(storage.db());
    let record = catalog.get(account, resource).unwrap();
    let database = open_compute_storage::KvPaths::open(storage.data_dir().root())
        .unwrap()
        .resolve_storage_key(&record.storage_key, account, resource)
        .unwrap();
    let engine = open_compute_storage::KvEngine::from_record(database.clone(), &record).unwrap();
    engine
        .put(
            "expired",
            b"value",
            &open_compute_storage::KvPutOptions {
                expires_at_ms: Some(60_001),
                metadata_json: None,
            },
            1,
        )
        .unwrap();
    let metrics =
        Arc::new(MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd").unwrap());
    let pin = pins.try_pin(resource).unwrap();
    run_kv_maintenance(
        &storage,
        &pins,
        &open_compute_core::KvConfig::default(),
        &metrics,
    )
    .await;
    assert!(engine.get("expired", 1).unwrap().is_some());
    drop(pin);
    run_kv_maintenance(
        &storage,
        &pins,
        &open_compute_core::KvConfig::default(),
        &metrics,
    )
    .await;
    assert!(engine.get("expired", i64::MAX).unwrap().is_none());
    let conn = rusqlite::Connection::open(database).unwrap();
    conn.execute(
        "UPDATE kv_meta SET value = ?1 WHERE key = 'resource_id'",
        [b"wrong".as_slice()],
    )
    .unwrap();
    drop(conn);
    run_kv_maintenance(
        &storage,
        &pins,
        &open_compute_core::KvConfig::default(),
        &metrics,
    )
    .await;
    let isolated = open_compute_storage::ResourceRepository::new(storage.db())
        .get(account, resource)
        .unwrap();
    assert_eq!(
        isolated.availability,
        open_compute_core::ResourceAvailability::Unavailable
    );
    let rendered = metrics.render(&PlatformStatus::starting());
    assert!(rendered.contains("kv_gc_entries_total{outcome=\"success\"} 1"));
    assert!(rendered.contains("kv_checkpoint_total{outcome=\"success\"} 1"));
    assert!(rendered.contains("kv_corruption_total{class=\"sqlite\"} 1"));
}

#[tokio::test]
async fn run_real_workerd_on_merged_listener_serves_status_and_shuts_down() {
    let (_dir, path, mock) = initialized_doctor_fixture().await;
    let mut loaded = load_platform_config(&path).unwrap();
    loaded.config.runtime.startup_timeout_ms = 60_000;
    loaded.config.runtime.shutdown_grace_ms = 1_000;
    loaded.config.runtime.kill_timeout_ms = 2_000;

    let reserved = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = reserved.local_addr().unwrap();
    drop(reserved);
    loaded.config.server.public_bind = address.to_string();
    loaded.config.server.admin_bind = None;

    let mut task = tokio::spawn(run_platform(loaded));
    let response = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            match tokio::net::TcpStream::connect(address).await {
                Ok(mut stream) => {
                    stream
                        .write_all(
                            b"GET /health/status HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
                        )
                        .await
                        .unwrap();
                    let mut response = Vec::new();
                    stream.read_to_end(&mut response).await.unwrap();
                    break response;
                }
                Err(_) => {
                    if task.is_finished() {
                        panic!("platform startup ended early: {:?}", (&mut task).await);
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }
        }
    })
    .await
    .expect("merged listener readiness");

    rustix::process::kill_process(rustix::process::getpid(), rustix::process::Signal::TERM)
        .unwrap();
    tokio::time::timeout(Duration::from_secs(60), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let response = String::from_utf8(response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.contains("\"supervisor\""), "{response}");
    assert_eq!(mock.object_count(), 0);
}
#[path = "p2_3_route_epoch_tests.rs"]
mod p2_3_route_epoch_tests;
