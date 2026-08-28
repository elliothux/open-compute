//! workerd supervisor lifecycle tests against the fixture child and real workerd.

use open_compute_core::config::RuntimeConfig;
use open_compute_core::error::{ErrorCode, ReadinessReason};
use open_compute_core::ids::StartupId;
use open_compute_core::{DeterministicClock, Redactor, SecretString};
use open_compute_runtime::compile::CompiledConfig;
use open_compute_runtime::process::{
    assert_reaped, clear_signal_log, take_signal_log, wait_reaped,
};
use open_compute_runtime::supervisor::{
    DirectoryServicePath, ExternalServiceAddress, FnCompiler, SequenceJitter, SupervisorState,
    WorkerdSupervisor, WorkerdSupervisorOptions, blocking_spawn_is_waiting,
    clear_blocking_spawn_hold, hold_blocking_spawn, last_spawned_pid, probe_ready_with_raw_token,
    release_blocking_spawn, serve_argv, set_reader_fail_point, set_spawn_fail_point,
    take_owner_wait_count, token_fingerprint,
};
use open_compute_runtime::verify_runtime_binary;
use rustix::process::{Pid, test_kill_process};
use sha2::{Digest, Sha256};
use std::fs;
use std::future::Future;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};
use tempfile::TempDir;

const VERSION: &str = "workerd 2026-08-26";

fn host_target() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("macos", "x86_64") => "darwin-x64",
        ("linux", "x86_64") => "linux-x64",
        ("linux", "aarch64") => "linux-arm64",
        other => panic!("unsupported test host {other:?}"),
    }
}

fn host_archive() -> &'static str {
    match host_target() {
        "darwin-arm64" => "workerd-darwin-arm64.gz",
        "darwin-x64" => "workerd-darwin-64.gz",
        "linux-x64" => "workerd-linux-64.gz",
        "linux-arm64" => "workerd-linux-arm64.gz",
        other => panic!("unsupported test target {other}"),
    }
}

fn sha256_file(path: &Path) -> String {
    hex::encode(Sha256::digest(fs::read(path).expect("read")))
}

fn write_exec(path: &Path, src: &Path) {
    fs::copy(src, path).expect("copy fixture");
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}

fn write_lock(dir: &Path, binary_sha: &str) -> PathBuf {
    let target = host_target();
    let archive = host_archive();
    let lock = format!(
        r#"{{
  "schemaVersion": 1,
  "release": "v1.20260826.1",
  "expectedVersionOutput": "{VERSION}",
  "hostCompatibilityDate": "2026-08-22",
  "processFlags": ["--experimental"],
  "hostCompatibilityFlags": ["nodejs_compat", "rpc", "enable_ctx_exports", "experimental"],
  "targets": {{
    "{target}": {{
      "archiveName": "{archive}",
      "archiveUrl": "https://github.com/cloudflare/workerd/releases/download/v1.20260826.1/{archive}",
      "archiveSha256": "22657ec7045a3677b7f52e97f106fe0493add57810687e755e8c6f4fba4b1dba",
      "binarySha256": "{binary_sha}"
    }}
  }}
}}"#
    );
    let path = dir.join("workerd.lock.json");
    fs::write(&path, lock).unwrap();
    path
}

fn write_versioned_fixture(dir: &Path) -> PathBuf {
    let fixture = PathBuf::from(env!("CARGO_BIN_EXE_open-compute-supervisor-fixture"));
    let binary = dir.join("workerd");
    write_exec(&binary, &fixture);
    binary
}

async fn verified(dir: &Path) -> open_compute_runtime::VerifiedRuntime {
    let bin = write_versioned_fixture(dir);
    let lock = write_lock(dir, &sha256_file(&bin));
    verify_runtime_binary(&lock, &bin, Duration::from_secs(5), &Redactor::new())
        .await
        .expect("verify fixture")
}

fn small_cfg() -> RuntimeConfig {
    RuntimeConfig {
        startup_timeout_ms: 5_000,
        shutdown_grace_ms: 200,
        drain_timeout_ms: 10,
        kill_timeout_ms: 200,
        restart_budget: 3,
        restart_window_ms: 60_000,
        restart_backoff_initial_ms: 5,
        restart_backoff_max_ms: 20,
    }
}

#[allow(clippy::type_complexity)]
fn compiler(
    data: PathBuf,
    mode: &'static str,
    argv_path: Option<PathBuf>,
    extra: serde_json::Value,
) -> FnCompiler<
    impl Fn(
        SecretString,
        StartupId,
    ) -> Pin<
        Box<dyn Future<Output = Result<CompiledConfig, open_compute_core::PlatformError>> + Send>,
    >,
> {
    FnCompiler(move |token: SecretString, id: StartupId| {
        let data = data.clone();
        let argv_path = argv_path.clone();
        let extra = extra.clone();
        Box::pin(async move {
            let mut body = extra;
            if !body.is_object() {
                body = serde_json::json!({});
            }
            let obj = body.as_object_mut().unwrap();
            obj.insert("mode".into(), serde_json::Value::String(mode.into()));
            obj.insert(
                "token".into(),
                serde_json::Value::String(token.expose().to_owned()),
            );
            if let Some(p) = argv_path {
                obj.insert(
                    "argv_path".into(),
                    serde_json::Value::String(p.display().to_string()),
                );
            }
            let digest = id.to_string().replace('-', "");
            CompiledConfig::from_bytes_for_test(&data, &digest, &serde_json::to_vec(&body).unwrap())
        })
            as Pin<
                Box<
                    dyn Future<Output = Result<CompiledConfig, open_compute_core::PlatformError>>
                        + Send,
                >,
            >
    })
}

async fn wait_state(
    sup: &WorkerdSupervisor,
    want: SupervisorState,
) -> open_compute_runtime::SupervisorSnapshot {
    let mut rx = sup.subscribe();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let snap = rx.borrow().clone();
        if snap.state == want {
            return snap;
        }
        tokio::select! {
            () = tokio::time::sleep_until(deadline) => panic!(
                "timeout waiting for {want:?}, last={snap:?}, diagnostics={:?}",
                sup.last_diagnostics()
            ),
            changed = rx.changed() => { changed.expect("watch"); }
        }
    }
}

fn pid_alive(pid: i32) -> bool {
    test_kill_process(Pid::from_raw(pid).unwrap()).is_ok()
}

#[tokio::test]
async fn argv_exact_stdin_fd3_and_auth_probe() {
    let dir = TempDir::new().unwrap();
    let runtime = verified(dir.path()).await;
    let argv_path = dir.path().join("argv.json");
    let stdin_path = dir.path().join("stdin.bin");
    let data = dir.path().join("runtime-data");
    fs::create_dir(&data).unwrap();
    let clock = Arc::new(DeterministicClock::new(UNIX_EPOCH));
    let extra = serde_json::json!({"stdin_marker_path": stdin_path.display().to_string()});
    let sup = WorkerdSupervisor::new(WorkerdSupervisorOptions {
        runtime: runtime.clone(),
        compiler: compiler(data, "ready", Some(argv_path.clone()), extra),
        config: small_cfg(),
        clock,
        jitter: Arc::new(SequenceJitter::new(vec![0])),
        redactor: Redactor::new(),
        lease_path: None,
    });
    sup.start();
    let snap = wait_state(&sup, SupervisorState::Running).await;
    assert_eq!(snap.reason, ReadinessReason::Ready);
    assert_eq!(snap.pid, snap.pgid);
    let argv: Vec<String> = serde_json::from_slice(&fs::read(&argv_path).unwrap()).unwrap();
    assert_eq!(argv, serve_argv(runtime.lock()));
    let joined = argv.join(" ");
    assert!(!joined.contains("token"));
    let stdin = fs::read(&stdin_path).unwrap();
    assert!(stdin.windows(5).any(|w| w == b"token"));
    let port = snap.listen_port.expect("port");
    probe_ready_with_raw_token(port, "00".repeat(32).as_str(), Duration::from_secs(1))
        .await
        .expect_err("wrong token must fail closed");
    let fp = snap.token_fingerprint.clone().unwrap();
    assert_eq!(fp.len(), 16);
    sup.shutdown().await;
    wait_reaped(snap.pid.unwrap(), Duration::from_secs(2)).unwrap();
}

#[tokio::test]
async fn control_faults_reap_pid_and_pgid() {
    for mode in [
        "malformed_control",
        "oversized_control",
        "duplicate_control",
        "wrong_socket",
        "non_loopback",
        "timeout",
        "early_exit",
        "bind_fail",
        "no_control",
    ] {
        let dir = TempDir::new().unwrap();
        let runtime = verified(dir.path()).await;
        let data = dir.path().join("d");
        fs::create_dir(&data).unwrap();
        let mut cfg = small_cfg();
        cfg.startup_timeout_ms = 300;
        cfg.restart_budget = 1;
        let sup = WorkerdSupervisor::new(WorkerdSupervisorOptions {
            runtime,
            compiler: compiler(data, mode, None, serde_json::json!({})),
            config: cfg,
            clock: Arc::new(DeterministicClock::new(UNIX_EPOCH)),
            jitter: Arc::new(SequenceJitter::new(vec![0])),
            redactor: Redactor::new(),
            lease_path: None,
        });
        sup.start();
        let snap = wait_state(&sup, SupervisorState::Failed).await;
        if let Some(pid) = snap.pid {
            assert!(!pid_alive(pid));
        }
        sup.shutdown().await;
        let _ = mode;
    }
}

#[tokio::test]
async fn term_and_kill_and_descendant() {
    let dir = TempDir::new().unwrap();
    let runtime = verified(dir.path()).await;
    let data = dir.path().join("d");
    fs::create_dir(&data).unwrap();
    let child_pid_path = dir.path().join("child.pid");
    let extra = serde_json::json!({"child_pid_path": child_pid_path.display().to_string()});
    let mut cfg = small_cfg();
    cfg.shutdown_grace_ms = 150;
    let sup = WorkerdSupervisor::new(WorkerdSupervisorOptions {
        runtime,
        compiler: compiler(data, "child", None, extra),
        config: cfg,
        clock: Arc::new(DeterministicClock::new(UNIX_EPOCH)),
        jitter: Arc::new(SequenceJitter::new(vec![0])),
        redactor: Redactor::new(),
        lease_path: None,
    });
    sup.start();
    let snap = wait_state(&sup, SupervisorState::Running).await;
    let pid = snap.pid.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let descendant: i32 = fs::read_to_string(&child_pid_path)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    sup.shutdown().await;
    wait_reaped(pid, Duration::from_secs(2)).unwrap();
    assert!(!pid_alive(descendant), "descendant leaked");
}

#[tokio::test]
async fn ignore_term_then_kill() {
    let dir = TempDir::new().unwrap();
    let runtime = verified(dir.path()).await;
    let data = dir.path().join("d");
    fs::create_dir(&data).unwrap();
    let mut cfg = small_cfg();
    cfg.shutdown_grace_ms = 80;
    cfg.kill_timeout_ms = 200;
    let sup = WorkerdSupervisor::new(WorkerdSupervisorOptions {
        runtime,
        compiler: compiler(data, "ignore_term", None, serde_json::json!({})),
        config: cfg,
        clock: Arc::new(DeterministicClock::new(UNIX_EPOCH)),
        jitter: Arc::new(SequenceJitter::new(vec![0])),
        redactor: Redactor::new(),
        lease_path: None,
    });
    sup.start();
    let snap = wait_state(&sup, SupervisorState::Running).await;
    let pid = snap.pid.unwrap();
    sup.shutdown().await;
    wait_reaped(pid, Duration::from_secs(3)).unwrap();
}

#[tokio::test]
async fn logs_bounded_and_redacted() {
    let dir = TempDir::new().unwrap();
    let runtime = verified(dir.path()).await;
    let data = dir.path().join("d");
    fs::create_dir(&data).unwrap();
    let mut redactor = Redactor::new();
    redactor.register_str("/secret/token-path");
    let mut cfg = small_cfg();
    cfg.restart_budget = 1;
    let sup = WorkerdSupervisor::new(WorkerdSupervisorOptions {
        runtime,
        compiler: compiler(data, "secret_logs", None, serde_json::json!({})),
        config: cfg,
        clock: Arc::new(DeterministicClock::new(UNIX_EPOCH)),
        jitter: Arc::new(SequenceJitter::new(vec![0])),
        redactor,
        lease_path: None,
    });
    sup.start();
    let snap = wait_state(&sup, SupervisorState::Running).await;
    let token_in_config = snap.token_fingerprint.clone();
    let _ = token_in_config;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let now = sup.snapshot();
        if now.state == SupervisorState::Failed || now.state == SupervisorState::BackingOff {
            break;
        }
        if tokio::time::Instant::now() > deadline {
            panic!("secret_logs did not exit, last={now:?}");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let snap = sup.snapshot();
    let debug = format!("{snap:?}");
    let status = serde_json::to_string(&snap).unwrap();
    assert!(!debug.contains("Authorization"));
    assert!(!debug.to_lowercase().contains("token="));
    assert!(!debug.contains("/secret/token-path"));
    assert!(!status.contains("token_fingerprint"));
    let exit = snap.last_exit.expect("sanitized exit");
    assert_eq!(exit.code, Some(42));
    assert_eq!(exit.signal, None);
    let diag = sup.last_diagnostics().expect("diagnostics");
    assert!(diag.stdout_tail.len() <= 16 * 1024);
    assert!(diag.stderr_tail.len() <= 16 * 1024);
    assert!(!diag.stdout_tail.contains("Authorization"));
    assert!(!diag.stderr_tail.contains("Authorization: Bearer"));
    assert!(!diag.stdout_tail.contains("/secret/token-path"));
    assert!(!diag.stderr_tail.contains("/secret/token-path"));
    assert!(
        diag.stderr_tail.contains("[REDACTED]") || diag.stdout_tail.contains("[REDACTED]"),
        "headers must be redacted"
    );
    assert!(
        !diag.stderr_tail.contains(&"A".repeat(9000)),
        "oversized lines must be bounded"
    );
    assert!(
        diag.stderr_tail.contains('\u{fffd}') || diag.stdout_tail.contains('\u{fffd}'),
        "invalid utf8 must be lossy-redacted"
    );
    sup.shutdown().await;
}

#[tokio::test]
async fn reader_failure_reaches_diagnostics() {
    let dir = TempDir::new().unwrap();
    let runtime = verified(dir.path()).await;
    let data = dir.path().join("d");
    fs::create_dir(&data).unwrap();
    set_reader_fail_point();
    let sup = WorkerdSupervisor::new(WorkerdSupervisorOptions {
        runtime,
        compiler: compiler(data, "ready", None, serde_json::json!({})),
        config: small_cfg(),
        clock: Arc::new(DeterministicClock::new(UNIX_EPOCH)),
        jitter: Arc::new(SequenceJitter::new(vec![0])),
        redactor: Redactor::new(),
        lease_path: None,
    });
    sup.start();
    let _ = wait_state(&sup, SupervisorState::Running).await;
    sup.shutdown().await;
    let diag = sup.last_diagnostics().expect("diagnostics");
    assert!(diag.reader_failed, "reader failure must be retained");
}

#[tokio::test]
async fn unexpected_exit_backoff_and_budget() {
    let dir = TempDir::new().unwrap();
    let runtime = verified(dir.path()).await;
    let data = dir.path().join("d");
    fs::create_dir(&data).unwrap();
    let mut cfg = small_cfg();
    cfg.restart_budget = 3;
    cfg.restart_backoff_initial_ms = 5;
    cfg.restart_backoff_max_ms = 40;
    let clock = Arc::new(DeterministicClock::new(UNIX_EPOCH));
    let sup = WorkerdSupervisor::new(WorkerdSupervisorOptions {
        runtime,
        compiler: compiler(data, "early_exit", None, serde_json::json!({})),
        config: cfg,
        clock: clock.clone(),
        jitter: Arc::new(SequenceJitter::new(vec![0, 0, 0, 0])),
        redactor: Redactor::new(),
        lease_path: None,
    });
    sup.start();
    let mut fingerprints = Vec::new();
    let mut seen_backoff = 0u32;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        clock.advance(Duration::from_millis(50));
        let snap = sup.snapshot();
        if let Some(fp) = snap.token_fingerprint.clone()
            && !fingerprints.contains(&fp)
        {
            fingerprints.push(fp);
        }
        if snap.state == SupervisorState::BackingOff {
            seen_backoff += 1;
        }
        if snap.state == SupervisorState::Failed {
            break;
        }
        if tokio::time::Instant::now() > deadline {
            panic!("did not fail, last={snap:?}");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        fingerprints.len() >= 3,
        "fresh token each attempt {fingerprints:?}"
    );
    assert!(seen_backoff >= 1);
    let final_snap = sup.snapshot();
    assert_eq!(final_snap.reason, ReadinessReason::RuntimeInvalid);
    let _ = seen_backoff;
    sup.shutdown().await;
}

#[tokio::test]
async fn invalid_compile_does_not_retry() {
    let dir = TempDir::new().unwrap();
    let runtime = verified(dir.path()).await;
    let clock = Arc::new(DeterministicClock::new(UNIX_EPOCH));
    let sup = WorkerdSupervisor::new(WorkerdSupervisorOptions {
        runtime,
        compiler: FnCompiler(|_t, _id| {
            Box::pin(async {
                Err(open_compute_core::PlatformError::new(
                    ErrorCode::ConfigCompileFailed,
                    "static config compilation failed",
                ))
            })
                as Pin<
                    Box<
                        dyn Future<
                                Output = Result<CompiledConfig, open_compute_core::PlatformError>,
                            > + Send,
                    >,
                >
        }),
        config: small_cfg(),
        clock: clock.clone(),
        jitter: Arc::new(SequenceJitter::new(vec![0])),
        redactor: Redactor::new(),
        lease_path: None,
    });
    sup.start();
    let snap = wait_state(&sup, SupervisorState::Failed).await;
    assert_eq!(snap.reason, ReadinessReason::ConfigInvalid);
    clock.advance(Duration::from_secs(10));
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert_eq!(sup.snapshot().state, SupervisorState::Failed);
    assert_eq!(sup.snapshot().attempt, 1);
    sup.shutdown().await;
}

#[tokio::test]
async fn shutdown_does_not_consume_budget_and_is_idempotent() {
    let dir = TempDir::new().unwrap();
    let runtime = verified(dir.path()).await;
    let data = dir.path().join("d");
    fs::create_dir(&data).unwrap();
    let sup = WorkerdSupervisor::new(WorkerdSupervisorOptions {
        runtime,
        compiler: compiler(data, "ready", None, serde_json::json!({})),
        config: small_cfg(),
        clock: Arc::new(DeterministicClock::new(UNIX_EPOCH)),
        jitter: Arc::new(SequenceJitter::new(vec![0])),
        redactor: Redactor::new(),
        lease_path: None,
    });
    sup.start();
    let snap = wait_state(&sup, SupervisorState::Running).await;
    let pid = snap.pid.unwrap();
    sup.shutdown().await;
    wait_state(&sup, SupervisorState::Stopped).await;
    assert!(snap.last_exit.is_none() || !snap.last_exit.as_ref().unwrap().retryable);
    wait_reaped(pid, Duration::from_secs(2)).unwrap();
    sup.shutdown().await;
    let after = sup.snapshot();
    assert_eq!(after.state, SupervisorState::Stopped);
    assert!(
        after
            .last_exit
            .as_ref()
            .is_none_or(|e| e.code_name != "RUNTIME_INVALID"),
        "clean shutdown must not be classified as runtime invalid: {:?}",
        after.last_exit
    );
}

#[tokio::test]
async fn drop_reaps_child() {
    let dir = TempDir::new().unwrap();
    let runtime = verified(dir.path()).await;
    let data = dir.path().join("d");
    fs::create_dir(&data).unwrap();
    let pid;
    {
        let sup = WorkerdSupervisor::new(WorkerdSupervisorOptions {
            runtime,
            compiler: compiler(data, "ready", None, serde_json::json!({})),
            config: small_cfg(),
            clock: Arc::new(DeterministicClock::new(UNIX_EPOCH)),
            jitter: Arc::new(SequenceJitter::new(vec![0])),
            redactor: Redactor::new(),
            lease_path: None,
        });
        sup.start();
        let snap = wait_state(&sup, SupervisorState::Running).await;
        pid = snap.pid.unwrap();
    }
    wait_reaped(pid, Duration::from_secs(3)).unwrap();
}

#[tokio::test]
async fn timestamps_use_deterministic_clock() {
    let dir = TempDir::new().unwrap();
    let runtime = verified(dir.path()).await;
    let data = dir.path().join("d");
    fs::create_dir(&data).unwrap();
    let clock = Arc::new(DeterministicClock::new(
        UNIX_EPOCH + Duration::from_secs(42),
    ));
    let sup = WorkerdSupervisor::new(WorkerdSupervisorOptions {
        runtime,
        compiler: compiler(data, "ready", None, serde_json::json!({})),
        config: small_cfg(),
        clock: clock.clone(),
        jitter: Arc::new(SequenceJitter::new(vec![0])),
        redactor: Redactor::new(),
        lease_path: None,
    });
    sup.start();
    let snap = wait_state(&sup, SupervisorState::Running).await;
    assert_eq!(
        snap.last_transition_at,
        UNIX_EPOCH + Duration::from_secs(42)
    );
    let _ = token_fingerprint(&SecretString::new(
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    ));
    sup.shutdown().await;
}

#[tokio::test]
async fn real_workerd_control_probe_term_kill() {
    let Some(path) = std::env::var_os("OPEN_COMPUTE_TEST_WORKERD") else {
        eprintln!("OPEN_COMPUTE_TEST_WORKERD unset; real workerd supervisor test not executed");
        return;
    };
    let path = PathBuf::from(path);
    assert!(
        path.is_absolute(),
        "OPEN_COMPUTE_TEST_WORKERD must be absolute"
    );
    let lock_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/workerd.lock.json");
    let lock_path = lock_path.canonicalize().unwrap();
    let assets = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../runtime")
        .canonicalize()
        .unwrap();
    let runtime =
        verify_runtime_binary(&lock_path, &path, Duration::from_secs(10), &Redactor::new())
            .await
            .expect("real workerd must verify");
    let dir = TempDir::new().unwrap();
    let data = dir.path().join("runtime");
    let do_storage = dir.path().join("do-storage");
    fs::create_dir(&data).unwrap();
    fs::create_dir(&do_storage).unwrap();
    let compiler = open_compute_runtime::StaticConfigCompiler::new(
        runtime.clone(),
        lock_path,
        assets,
        data,
        open_compute_runtime::PlatformReleaseMeta {
            version: "0.1.0-test".into(),
        },
        Duration::from_secs(20),
        Redactor::new(),
    );
    let mut cfg = small_cfg();
    cfg.startup_timeout_ms = 20_000;
    cfg.shutdown_grace_ms = 1_000;
    cfg.kill_timeout_ms = 1_000;
    cfg.drain_timeout_ms = 10;
    let runtime_source = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let runtime_source_addr = runtime_source.local_addr().unwrap();
    let binding_backend = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let binding_backend_addr = binding_backend.local_addr().unwrap();
    let sup = WorkerdSupervisor::new_with_services_and_auth(
        WorkerdSupervisorOptions {
            runtime,
            compiler,
            config: cfg,
            clock: Arc::new(DeterministicClock::new(UNIX_EPOCH)),
            jitter: Arc::new(SequenceJitter::new(vec![0])),
            redactor: Redactor::new(),
            lease_path: None,
        },
        vec![
            ExternalServiceAddress::loopback("runtime-source", runtime_source_addr).unwrap(),
            ExternalServiceAddress::loopback("binding-backend", binding_backend_addr).unwrap(),
        ],
        vec![DirectoryServicePath::local("do-storage", &do_storage).unwrap()],
        Vec::new(),
    );
    sup.start();
    let snap = wait_state(&sup, SupervisorState::Running).await;
    let port = snap.listen_port.expect("ephemeral port");
    let pid = snap.pid.unwrap();
    probe_ready_with_raw_token(port, "00".repeat(32).as_str(), Duration::from_secs(2))
        .await
        .expect_err("wrong token must fail against real workerd");
    sup.shutdown().await;
    wait_reaped(pid, Duration::from_secs(5)).unwrap();
    assert_reaped(Some(pid)).unwrap();
}

#[tokio::test]
async fn shutdown_before_start_acks_and_is_idempotent() {
    let dir = TempDir::new().unwrap();
    let runtime = verified(dir.path()).await;
    let data = dir.path().join("d");
    fs::create_dir(&data).unwrap();
    let sup = WorkerdSupervisor::new(WorkerdSupervisorOptions {
        runtime,
        compiler: compiler(data, "ready", None, serde_json::json!({})),
        config: small_cfg(),
        clock: Arc::new(DeterministicClock::new(UNIX_EPOCH)),
        jitter: Arc::new(SequenceJitter::new(vec![0])),
        redactor: Redactor::new(),
        lease_path: None,
    });
    tokio::time::timeout(Duration::from_secs(2), sup.shutdown())
        .await
        .expect("shutdown before start must acknowledge");
    assert_eq!(sup.snapshot().state, SupervisorState::Stopped);
    tokio::time::timeout(Duration::from_secs(2), sup.shutdown())
        .await
        .expect("second shutdown must be idempotent");
}

#[tokio::test]
async fn shutdown_cancels_slow_compile_control_probe_and_backoff() {
    let dir = TempDir::new().unwrap();
    let runtime = verified(dir.path()).await;
    let data = dir.path().join("d");
    fs::create_dir(&data).unwrap();

    let slow = WorkerdSupervisor::new(WorkerdSupervisorOptions {
        runtime: runtime.clone(),
        compiler: FnCompiler(|_t, _id| {
            Box::pin(async {
                tokio::time::sleep(Duration::from_secs(60)).await;
                Err(open_compute_core::PlatformError::new(
                    ErrorCode::ConfigCompileFailed,
                    "static config compilation failed",
                ))
            })
                as Pin<
                    Box<
                        dyn Future<
                                Output = Result<CompiledConfig, open_compute_core::PlatformError>,
                            > + Send,
                    >,
                >
        }),
        config: small_cfg(),
        clock: Arc::new(DeterministicClock::new(UNIX_EPOCH)),
        jitter: Arc::new(SequenceJitter::new(vec![0])),
        redactor: Redactor::new(),
        lease_path: None,
    });
    slow.start();
    tokio::time::sleep(Duration::from_millis(30)).await;
    tokio::time::timeout(Duration::from_secs(2), slow.shutdown())
        .await
        .expect("shutdown during compile");

    for mode in ["no_control", "slow_probe"] {
        let data = dir.path().join(mode);
        fs::create_dir(&data).unwrap();
        let mut cfg = small_cfg();
        cfg.startup_timeout_ms = 30_000;
        let sup = WorkerdSupervisor::new(WorkerdSupervisorOptions {
            runtime: runtime.clone(),
            compiler: compiler(data, mode, None, serde_json::json!({})),
            config: cfg,
            clock: Arc::new(DeterministicClock::new(UNIX_EPOCH)),
            jitter: Arc::new(SequenceJitter::new(vec![0])),
            redactor: Redactor::new(),
            lease_path: None,
        });
        sup.start();
        tokio::time::sleep(Duration::from_millis(80)).await;
        let pid = last_spawned_pid();
        tokio::time::timeout(Duration::from_secs(3), sup.shutdown())
            .await
            .unwrap_or_else(|_| panic!("shutdown during {mode}"));
        if let Some(pid) = pid {
            wait_reaped(pid, Duration::from_secs(3)).unwrap();
        }
    }

    let data = dir.path().join("bo");
    fs::create_dir(&data).unwrap();
    let mut cfg = small_cfg();
    cfg.restart_backoff_initial_ms = 60_000;
    cfg.restart_backoff_max_ms = 60_000;
    let clock = Arc::new(DeterministicClock::new(UNIX_EPOCH));
    let sup = WorkerdSupervisor::new(WorkerdSupervisorOptions {
        runtime,
        compiler: compiler(data, "early_exit", None, serde_json::json!({})),
        config: cfg,
        clock,
        jitter: Arc::new(SequenceJitter::new(vec![0])),
        redactor: Redactor::new(),
        lease_path: None,
    });
    sup.start();
    wait_state(&sup, SupervisorState::BackingOff).await;
    tokio::time::timeout(Duration::from_secs(2), sup.shutdown())
        .await
        .expect("shutdown during backoff");
}

#[tokio::test]
async fn post_spawn_failures_reap_child() {
    for point in ["pgid", "stdin", "control", "logs"] {
        let dir = TempDir::new().unwrap();
        let runtime = verified(dir.path()).await;
        let data = dir.path().join("d");
        fs::create_dir(&data).unwrap();
        set_spawn_fail_point(point);
        let mut cfg = small_cfg();
        cfg.restart_budget = 1;
        let sup = WorkerdSupervisor::new(WorkerdSupervisorOptions {
            runtime,
            compiler: compiler(data, "ready", None, serde_json::json!({})),
            config: cfg,
            clock: Arc::new(DeterministicClock::new(UNIX_EPOCH)),
            jitter: Arc::new(SequenceJitter::new(vec![0])),
            redactor: Redactor::new(),
            lease_path: None,
        });
        sup.start();
        wait_state(&sup, SupervisorState::Failed).await;
        if let Some(pid) = last_spawned_pid() {
            wait_reaped(pid, Duration::from_secs(3)).unwrap();
            assert_reaped(Some(pid)).unwrap();
        } else {
            panic!("expected spawned pid at fail point {point}");
        }
        sup.shutdown().await;
    }
}

#[tokio::test]
async fn drop_does_not_signal_or_double_wait_reaped_pid() {
    let dir = TempDir::new().unwrap();
    let runtime = verified(dir.path()).await;
    let data = dir.path().join("d");
    fs::create_dir(&data).unwrap();
    clear_signal_log();
    let _ = take_owner_wait_count();
    let pid;
    {
        let mut cfg = small_cfg();
        cfg.restart_budget = 1;
        let sup = WorkerdSupervisor::new(WorkerdSupervisorOptions {
            runtime,
            compiler: compiler(data, "crash_after_ready", None, serde_json::json!({})),
            config: cfg,
            clock: Arc::new(DeterministicClock::new(UNIX_EPOCH)),
            jitter: Arc::new(SequenceJitter::new(vec![0])),
            redactor: Redactor::new(),
            lease_path: None,
        });
        sup.start();
        let snap = wait_state(&sup, SupervisorState::Running).await;
        pid = snap.pid.unwrap();
        wait_state(&sup, SupervisorState::Failed).await;
        wait_reaped(pid, Duration::from_secs(3)).unwrap();
        let waits_before_drop = take_owner_wait_count();
        assert!(waits_before_drop >= 1, "owner must reap once");
        let signals_before = take_signal_log();
        drop(sup);
        let signals_after = take_signal_log();
        assert!(
            signals_after.iter().all(|(p, _)| *p != pid),
            "Drop must not signal a reaped snapshot pid {signals_before:?} {signals_after:?}"
        );
        let waits_after = take_owner_wait_count();
        assert_eq!(waits_after, 0, "Drop must not double-wait");
    }
    assert_reaped(Some(pid)).unwrap();
}

#[tokio::test]
async fn late_control_event_is_unhealthy_restart() {
    for mode in ["late_duplicate_control", "late_malformed_control"] {
        let dir = TempDir::new().unwrap();
        let runtime = verified(dir.path()).await;
        let data = dir.path().join("d");
        fs::create_dir(&data).unwrap();
        let mut cfg = small_cfg();
        cfg.restart_budget = 3;
        let clock = Arc::new(DeterministicClock::new(UNIX_EPOCH));
        let sup = WorkerdSupervisor::new(WorkerdSupervisorOptions {
            runtime,
            compiler: compiler(data, mode, None, serde_json::json!({})),
            config: cfg,
            clock: clock.clone(),
            jitter: Arc::new(SequenceJitter::new(vec![0])),
            redactor: Redactor::new(),
            lease_path: None,
        });
        sup.start();
        let snap = wait_state(&sup, SupervisorState::Running).await;
        let pid = snap.pid.unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            clock.advance(Duration::from_millis(20));
            let now = sup.snapshot();
            if now.state == SupervisorState::BackingOff || now.state == SupervisorState::Starting {
                break;
            }
            if now.state == SupervisorState::Failed {
                break;
            }
            if tokio::time::Instant::now() > deadline {
                panic!("{mode} did not teardown, last={now:?}");
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        wait_reaped(pid, Duration::from_secs(3)).unwrap();
        sup.shutdown().await;
    }
}

#[tokio::test]
async fn running_resets_consecutive_backoff() {
    let dir = TempDir::new().unwrap();
    let runtime = verified(dir.path()).await;
    let data = dir.path().join("d");
    fs::create_dir(&data).unwrap();
    let n = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let data2 = data.clone();
    let compiler = FnCompiler(move |token: SecretString, id: StartupId| {
        let data = data2.clone();
        let attempt = n.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async move {
            let mode = if attempt == 0 {
                "crash_after_ready"
            } else {
                "ready"
            };
            let body = serde_json::json!({"mode": mode, "token": token.expose()});
            let digest = id.to_string().replace('-', "");
            CompiledConfig::from_bytes_for_test(&data, &digest, &serde_json::to_vec(&body).unwrap())
        })
            as Pin<
                Box<
                    dyn Future<Output = Result<CompiledConfig, open_compute_core::PlatformError>>
                        + Send,
                >,
            >
    });
    let mut cfg = small_cfg();
    cfg.restart_backoff_initial_ms = 10;
    cfg.restart_backoff_max_ms = 80;
    cfg.restart_budget = 8;
    let clock = Arc::new(DeterministicClock::new(UNIX_EPOCH));
    let sup = WorkerdSupervisor::new(WorkerdSupervisorOptions {
        runtime,
        compiler,
        config: cfg,
        clock: clock.clone(),
        jitter: Arc::new(SequenceJitter::new(vec![0, 0, 0, 0])),
        redactor: Redactor::new(),
        lease_path: None,
    });
    sup.start();
    wait_state(&sup, SupervisorState::Running).await;
    let backoff = wait_state(&sup, SupervisorState::BackingOff).await;
    let first = backoff
        .next_retry_at
        .unwrap()
        .duration_since(backoff.last_transition_at)
        .unwrap();
    clock.advance(first + Duration::from_millis(1));
    wait_state(&sup, SupervisorState::Running).await;
    sup.report_unhealthy();
    let backoff2 = wait_state(&sup, SupervisorState::BackingOff).await;
    let second = backoff2
        .next_retry_at
        .unwrap()
        .duration_since(backoff2.last_transition_at)
        .unwrap();
    assert_eq!(
        second, first,
        "successful RUNNING must reset consecutive backoff"
    );
    sup.shutdown().await;
}

#[tokio::test]
async fn shutdown_waits_for_held_blocking_spawn() {
    let dir = TempDir::new().unwrap();
    let runtime = verified(dir.path()).await;
    let data = dir.path().join("d");
    fs::create_dir(&data).unwrap();
    clear_blocking_spawn_hold();
    hold_blocking_spawn();
    let sup = Arc::new(WorkerdSupervisor::new(WorkerdSupervisorOptions {
        runtime,
        compiler: compiler(data, "ready", None, serde_json::json!({})),
        config: small_cfg(),
        clock: Arc::new(DeterministicClock::new(UNIX_EPOCH)),
        jitter: Arc::new(SequenceJitter::new(vec![0])),
        redactor: Redactor::new(),
        lease_path: None,
    }));
    sup.start();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while !blocking_spawn_is_waiting() {
        if tokio::time::Instant::now() > deadline {
            clear_blocking_spawn_hold();
            panic!("spawn never entered the blocking hold");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let shut_sup = sup.clone();
    let shut = tokio::spawn(async move { shut_sup.shutdown().await });
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !shut.is_finished(),
        "shutdown must not acknowledge while blocking spawn is held"
    );
    release_blocking_spawn();
    tokio::time::timeout(Duration::from_secs(5), async { shut.await.unwrap() })
        .await
        .expect("shutdown after release");
    clear_blocking_spawn_hold();
    assert_eq!(sup.snapshot().state, SupervisorState::Stopped);
    assert_eq!(sup.owner_registry_len(), 0);
    if let Some(pid) = last_spawned_pid() {
        wait_reaped(pid, Duration::from_secs(3)).unwrap();
        assert!(!pid_alive(pid));
    }
}

#[tokio::test]
async fn term_grace_then_kill_order() {
    let dir = TempDir::new().unwrap();
    let runtime = verified(dir.path()).await;
    let data = dir.path().join("d");
    fs::create_dir(&data).unwrap();
    clear_signal_log();
    let mut cfg = small_cfg();
    cfg.shutdown_grace_ms = 250;
    cfg.kill_timeout_ms = 400;
    cfg.drain_timeout_ms = 10;
    let sup = WorkerdSupervisor::new(WorkerdSupervisorOptions {
        runtime: runtime.clone(),
        compiler: compiler(data.clone(), "ready", None, serde_json::json!({})),
        config: cfg.clone(),
        clock: Arc::new(DeterministicClock::new(UNIX_EPOCH)),
        jitter: Arc::new(SequenceJitter::new(vec![0])),
        redactor: Redactor::new(),
        lease_path: None,
    });
    sup.start();
    let snap = wait_state(&sup, SupervisorState::Running).await;
    let pid = snap.pid.unwrap();
    let started = std::time::Instant::now();
    sup.shutdown().await;
    let elapsed = started.elapsed();
    wait_reaped(pid, Duration::from_secs(2)).unwrap();
    let signals = take_signal_log();
    let for_pid: Vec<_> = signals
        .iter()
        .filter(|(p, _)| *p == pid)
        .map(|(_, k)| *k)
        .collect();
    assert!(
        for_pid.contains(&"TERM"),
        "TERM-responsive child must receive TERM {signals:?}"
    );
    assert!(
        !for_pid.contains(&"KILL"),
        "TERM-responsive child must not receive KILL during grace {signals:?}"
    );
    assert!(
        elapsed < Duration::from_millis(250),
        "TERM-responsive shutdown should finish within grace, elapsed={elapsed:?}"
    );

    clear_signal_log();
    let data2 = dir.path().join("d2");
    fs::create_dir(&data2).unwrap();
    let mut cfg2 = cfg;
    cfg2.shutdown_grace_ms = 200;
    cfg2.kill_timeout_ms = 400;
    let sup = WorkerdSupervisor::new(WorkerdSupervisorOptions {
        runtime,
        compiler: compiler(data2, "ignore_term", None, serde_json::json!({})),
        config: cfg2,
        clock: Arc::new(DeterministicClock::new(UNIX_EPOCH)),
        jitter: Arc::new(SequenceJitter::new(vec![0])),
        redactor: Redactor::new(),
        lease_path: None,
    });
    sup.start();
    let snap = wait_state(&sup, SupervisorState::Running).await;
    let pid = snap.pid.unwrap();
    let started = std::time::Instant::now();
    sup.shutdown().await;
    let elapsed = started.elapsed();
    wait_reaped(pid, Duration::from_secs(3)).unwrap();
    let signals = take_signal_log();
    let for_pid: Vec<_> = signals
        .iter()
        .filter(|(p, _)| *p == pid)
        .map(|(_, k)| *k)
        .collect();
    let term_at = for_pid.iter().position(|k| *k == "TERM");
    let kill_at = for_pid.iter().position(|k| *k == "KILL");
    assert!(
        term_at.is_some(),
        "ignore_term must receive TERM {signals:?}"
    );
    assert!(
        kill_at.is_some(),
        "ignore_term must receive KILL {signals:?}"
    );
    assert!(
        kill_at.unwrap() > term_at.unwrap(),
        "KILL must follow TERM {signals:?}"
    );
    assert!(
        elapsed >= Duration::from_millis(180),
        "KILL must wait for grace, elapsed={elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_millis(200 + 400 + 500),
        "KILL must complete within grace+kill deadline, elapsed={elapsed:?}"
    );
}

#[tokio::test]
async fn owner_registry_does_not_grow_across_restarts() {
    let dir = TempDir::new().unwrap();
    let runtime = verified(dir.path()).await;
    let data = dir.path().join("d");
    fs::create_dir(&data).unwrap();
    let n = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let data2 = data.clone();
    let n_spawn = n.clone();
    let compiler = FnCompiler(move |token: SecretString, id: StartupId| {
        let data = data2.clone();
        let attempt = n_spawn.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async move {
            let mode = if attempt < 4 {
                "crash_after_ready"
            } else {
                "ready"
            };
            let body = serde_json::json!({"mode": mode, "token": token.expose()});
            let digest = id.to_string().replace('-', "");
            CompiledConfig::from_bytes_for_test(&data, &digest, &serde_json::to_vec(&body).unwrap())
        })
            as Pin<
                Box<
                    dyn Future<Output = Result<CompiledConfig, open_compute_core::PlatformError>>
                        + Send,
                >,
            >
    });
    let mut cfg = small_cfg();
    cfg.restart_budget = 8;
    cfg.restart_backoff_initial_ms = 5;
    cfg.restart_backoff_max_ms = 10;
    let clock = Arc::new(DeterministicClock::new(UNIX_EPOCH));
    let sup = WorkerdSupervisor::new(WorkerdSupervisorOptions {
        runtime,
        compiler,
        config: cfg,
        clock: clock.clone(),
        jitter: Arc::new(SequenceJitter::new(vec![0, 0, 0, 0, 0, 0, 0, 0])),
        redactor: Redactor::new(),
        lease_path: None,
    });
    sup.start();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        clock.advance(Duration::from_millis(20));
        if sup.snapshot().state == SupervisorState::Running
            && n.load(std::sync::atomic::Ordering::SeqCst) >= 5
        {
            break;
        }
        if tokio::time::Instant::now() > deadline {
            panic!(
                "did not reach later running generation, last={:?}",
                sup.snapshot()
            );
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    wait_state(&sup, SupervisorState::Running).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        sup.owner_registry_len() <= 1,
        "registry leaked senders: {}",
        sup.owner_registry_len()
    );
    sup.shutdown().await;
    assert_eq!(sup.owner_registry_len(), 0);
}

#[tokio::test]
async fn term_leader_kill_ignoring_descendant_holding_pipes() {
    let dir = TempDir::new().unwrap();
    let runtime = verified(dir.path()).await;
    let data = dir.path().join("d");
    fs::create_dir(&data).unwrap();
    let child_pid_path = dir.path().join("child.pid");
    let extra = serde_json::json!({"child_pid_path": child_pid_path.display().to_string()});
    let mut cfg = small_cfg();
    cfg.shutdown_grace_ms = 150;
    cfg.kill_timeout_ms = 400;
    cfg.drain_timeout_ms = 10;
    clear_signal_log();
    let sup = WorkerdSupervisor::new(WorkerdSupervisorOptions {
        runtime,
        compiler: compiler(data, "child_ignore_term", None, extra),
        config: cfg,
        clock: Arc::new(DeterministicClock::new(UNIX_EPOCH)),
        jitter: Arc::new(SequenceJitter::new(vec![0])),
        redactor: Redactor::new(),
        lease_path: None,
    });
    sup.start();
    let snap = wait_state(&sup, SupervisorState::Running).await;
    let pid = snap.pid.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let descendant: i32 = fs::read_to_string(&child_pid_path)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), sup.shutdown())
        .await
        .expect("shutdown must not hang on TERM-ignoring descendant holding pipes");
    wait_reaped(pid, Duration::from_secs(2)).unwrap();
    assert!(!pid_alive(descendant), "TERM-ignoring descendant leaked");
    assert!(!pid_alive(pid));
    let signals = take_signal_log();
    let kinds: Vec<_> = signals
        .iter()
        .filter(|(p, _)| *p == pid)
        .map(|(_, k)| *k)
        .collect();
    assert!(kinds.contains(&"TERM"), "expected TERM {signals:?}");
    assert!(
        kinds.contains(&"KILL"),
        "expected KILL after descendant survived TERM {signals:?}"
    );
    let term_at = kinds.iter().position(|k| *k == "TERM").unwrap();
    let kill_at = kinds.iter().position(|k| *k == "KILL").unwrap();
    assert!(kill_at > term_at, "KILL must follow TERM {signals:?}");
    assert_eq!(sup.owner_registry_len(), 0);
    let diag = sup.last_diagnostics();
    assert!(diag.is_some(), "readers must join and retain diagnostics");
}

#[tokio::test]
async fn compile_failure_does_not_inherit_prior_exit() {
    let dir = TempDir::new().unwrap();
    let runtime = verified(dir.path()).await;
    let data = dir.path().join("d");
    fs::create_dir(&data).unwrap();
    let n = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let data2 = data.clone();
    let n_spawn = n.clone();
    let compiler = FnCompiler(move |token: SecretString, id: StartupId| {
        let data = data2.clone();
        let attempt = n_spawn.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async move {
            if attempt == 0 {
                let body =
                    serde_json::json!({"mode": "crash_after_ready", "token": token.expose()});
                let digest = id.to_string().replace('-', "");
                CompiledConfig::from_bytes_for_test(
                    &data,
                    &digest,
                    &serde_json::to_vec(&body).unwrap(),
                )
            } else {
                Err(open_compute_core::PlatformError::new(
                    ErrorCode::ConfigCompileFailed,
                    "static config compilation failed",
                ))
            }
        })
            as Pin<
                Box<
                    dyn Future<Output = Result<CompiledConfig, open_compute_core::PlatformError>>
                        + Send,
                >,
            >
    });
    let mut cfg = small_cfg();
    cfg.restart_budget = 8;
    cfg.restart_backoff_initial_ms = 5;
    cfg.restart_backoff_max_ms = 10;
    let clock = Arc::new(DeterministicClock::new(UNIX_EPOCH));
    let sup = WorkerdSupervisor::new(WorkerdSupervisorOptions {
        runtime,
        compiler,
        config: cfg,
        clock: clock.clone(),
        jitter: Arc::new(SequenceJitter::new(vec![0, 0, 0, 0])),
        redactor: Redactor::new(),
        lease_path: None,
    });
    sup.start();
    wait_state(&sup, SupervisorState::Running).await;
    let backoff = wait_state(&sup, SupervisorState::BackingOff).await;
    let gen1 = backoff.last_exit.clone().expect("generation 1 exit");
    assert_eq!(gen1.code, Some(9));
    clock.advance(Duration::from_millis(50));
    let snap = wait_state(&sup, SupervisorState::Failed).await;
    let exit = snap.last_exit.expect("generation 2 exit");
    assert_eq!(exit.code_name, "CONFIG_COMPILE_FAILED");
    assert_eq!(
        exit.code, None,
        "compile failure must not inherit prior exit code"
    );
    assert_eq!(
        exit.signal, None,
        "compile failure must not inherit prior signal"
    );
    sup.shutdown().await;
}

fn test_start_key(pid: i32) -> Option<String> {
    if pid <= 0 {
        return None;
    }
    Some(format!("t:{pid}"))
}

#[tokio::test]
async fn lease_persist_failure_reaps_child_and_never_runs() {
    let dir = TempDir::new().unwrap();
    let runtime = verified(dir.path()).await;
    let data = dir.path().join("d");
    fs::create_dir(&data).unwrap();
    let lease = dir.path().join("child.lease");
    open_compute_runtime::set_start_key_hook(Some(test_start_key));
    open_compute_runtime::set_lease_write_fail(true);
    let sup = WorkerdSupervisor::new(WorkerdSupervisorOptions {
        runtime,
        compiler: compiler(data, "ready", None, serde_json::json!({})),
        config: small_cfg(),
        clock: Arc::new(DeterministicClock::new(UNIX_EPOCH)),
        jitter: Arc::new(SequenceJitter::new(vec![0])),
        redactor: Redactor::new(),
        lease_path: Some(lease),
    });
    sup.start();
    let snap = wait_state(&sup, SupervisorState::Failed).await;
    assert_ne!(snap.state, SupervisorState::Running);
    if let Some(pid) = last_spawned_pid() {
        wait_reaped(pid, Duration::from_secs(3)).unwrap();
    }
    open_compute_runtime::set_lease_write_fail(false);
    open_compute_runtime::set_start_key_hook(None);
    sup.shutdown().await;
}

#[tokio::test]
async fn teardown_retains_lease_until_reap_is_proved() {
    let dir = TempDir::new().unwrap();
    let runtime = verified(dir.path()).await;
    let data = dir.path().join("teardown-proof");
    fs::create_dir(&data).unwrap();
    let lease = dir.path().join("child.lease");
    open_compute_runtime::set_start_key_hook(Some(test_start_key));

    let sup = WorkerdSupervisor::new(WorkerdSupervisorOptions {
        runtime: runtime.clone(),
        compiler: compiler(data.clone(), "ready", None, serde_json::json!({})),
        config: small_cfg(),
        clock: Arc::new(DeterministicClock::new(UNIX_EPOCH)),
        jitter: Arc::new(SequenceJitter::new(vec![0])),
        redactor: Redactor::new(),
        lease_path: Some(lease.clone()),
    });
    sup.start();
    let running = wait_state(&sup, SupervisorState::Running).await;
    let pid = running.pid.unwrap();
    assert!(lease.exists());

    open_compute_runtime::set_reap_probe_fail(true);
    sup.report_unhealthy();
    let failed = wait_state(&sup, SupervisorState::Failed).await;
    assert_eq!(failed.reason, ReadinessReason::RuntimeInvalid);
    assert!(lease.exists(), "failed reap proof must retain the lease");
    let attempts = failed.attempt;
    sup.start();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        sup.snapshot().attempt,
        attempts,
        "fail-closed state must not restart"
    );

    open_compute_runtime::set_reap_probe_fail(false);
    wait_reaped(pid, Duration::from_secs(3)).unwrap();
    sup.shutdown().await;
    assert!(
        lease.exists(),
        "the same actor must not clear a lease after losing its reap proof"
    );

    let replacement = WorkerdSupervisor::new(WorkerdSupervisorOptions {
        runtime,
        compiler: compiler(data, "ready", None, serde_json::json!({})),
        config: small_cfg(),
        clock: Arc::new(DeterministicClock::new(UNIX_EPOCH)),
        jitter: Arc::new(SequenceJitter::new(vec![0])),
        redactor: Redactor::new(),
        lease_path: Some(lease.clone()),
    });
    replacement.start();
    let next = wait_state(&replacement, SupervisorState::Running).await;
    assert_ne!(next.pid, Some(pid));
    replacement.shutdown().await;
    assert!(!lease.exists());
    open_compute_runtime::set_start_key_hook(None);
}
