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
  "schemaVersion": 3,
  "release": "v1.20260830.1",
  "revision": "e9dda5963aba7ee4323960db795690ec78fec118",
  "expectedVersionOutput": "{VERSION}",
  "effectiveCompatibilityDate": "2026-09-08",
  "requiredCompatibilityFlags": [],
  "systemCompatibilityFlags": ["experimental", "service_binding_extra_handlers"],
  "processFlags": ["--experimental"],
  "pyodideBundle": {{
    "version": "314.0.6_2026-08-17_2",
    "fileName": "pyodide_314.0.6_2026-08-17_2.capnp.bin",
    "archiveName": "pyodide_314.0.6_2026-08-17_2.capnp.bin.gz",
    "archiveSha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
    "bundleSha256": "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
  }},
  "source": {{
    "repository": "https://github.com/elliothux/workerd",
    "upstreamBase": "dd8133e9b9656fb39f1434247a80aa7a249ee204",
    "buildInputs": {{ "bazel": "9.2.0", "target": "//src/workerd/server:workerd", "mode": "opt" }}
  }},
  "workersTypes": {{
    "version": "5.20260830.1",
    "gitHead": "e9dda5963aba7ee4323960db795690ec78fec118",
    "packageSha256": "d3d7a80d3b27e53116e34736ec1945eb359f53a1000df37b205c4cb59ce29a8e",
    "astSha256": "a00b4783854c9028158f776d605790d9a3e17e6a97f4d255beb70035c59c40dd"
  }},
  "workersSdk": {{
    "revision": "f8085545bcaa2c639f171c25e4424685036a0e10",
    "wranglerVersion": "4.127.1",
    "vitePluginVersion": "1.54.2"
  }},
  "targets": {{
    "{target}": {{
      "archiveName": "{archive}",
      "archiveUrl": "https://github.com/elliothux/workerd/releases/download/v1.20260830.1/{archive}",
      "archiveSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
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

#[allow(
    clippy::type_complexity,
    reason = "the callable signature directly models the runtime protocol"
)]
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
    wait_state_within(sup, want, Duration::from_secs(5)).await
}

async fn wait_state_within(
    sup: &WorkerdSupervisor,
    want: SupervisorState,
    timeout: Duration,
) -> open_compute_runtime::SupervisorSnapshot {
    let mut rx = sup.subscribe();
    let deadline = tokio::time::Instant::now() + timeout;
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

mod argv_exact_stdin_fd3_and_auth_probe;

mod control_faults_reap_pid_and_pgid;

mod term_and_kill_and_descendant;

mod ignore_term_then_kill;

mod logs_bounded_and_redacted;

mod reader_failure_reaches_diagnostics;

mod unexpected_exit_backoff_and_budget;

mod invalid_compile_does_not_retry;

mod shutdown_does_not_consume_budget_and_is_idempotent;

mod drop_reaps_child;

mod timestamps_use_deterministic_clock;

mod real_workerd_control_probe_term_kill;

mod shutdown_before_start_acks_and_is_idempotent;

mod shutdown_cancels_slow_compile_control_probe_and_backoff;

mod post_spawn_failures_reap_child;

mod drop_does_not_signal_or_double_wait_reaped_pid;

mod late_control_event_is_unhealthy_restart;

mod running_resets_consecutive_backoff;

mod shutdown_waits_for_held_blocking_spawn;

mod term_grace_then_kill_order;

mod owner_registry_does_not_grow_across_restarts;

mod term_leader_kill_ignoring_descendant_holding_pipes;

mod compile_failure_does_not_inherit_prior_exit;

fn test_start_key(pid: i32) -> Option<String> {
    if pid <= 0 {
        return None;
    }
    Some(format!("t:{pid}"))
}

mod lease_persist_failure_reaps_child_and_never_runs;

mod teardown_retains_lease_until_reap_is_proved;

macro_rules! supervisor_case {
    ($name:ident) => {
        #[tokio::test]
        async fn $name() {
            $name::run().await;
        }
    };
}

supervisor_case!(argv_exact_stdin_fd3_and_auth_probe);
supervisor_case!(compile_failure_does_not_inherit_prior_exit);
supervisor_case!(control_faults_reap_pid_and_pgid);
supervisor_case!(drop_does_not_signal_or_double_wait_reaped_pid);
supervisor_case!(drop_reaps_child);
supervisor_case!(ignore_term_then_kill);
supervisor_case!(invalid_compile_does_not_retry);
supervisor_case!(late_control_event_is_unhealthy_restart);
supervisor_case!(lease_persist_failure_reaps_child_and_never_runs);
supervisor_case!(logs_bounded_and_redacted);
supervisor_case!(owner_registry_does_not_grow_across_restarts);
supervisor_case!(post_spawn_failures_reap_child);
supervisor_case!(reader_failure_reaches_diagnostics);
supervisor_case!(real_workerd_control_probe_term_kill);
supervisor_case!(running_resets_consecutive_backoff);
supervisor_case!(shutdown_before_start_acks_and_is_idempotent);
supervisor_case!(shutdown_cancels_slow_compile_control_probe_and_backoff);
supervisor_case!(shutdown_does_not_consume_budget_and_is_idempotent);
supervisor_case!(shutdown_waits_for_held_blocking_spawn);
supervisor_case!(teardown_retains_lease_until_reap_is_proved);
supervisor_case!(term_and_kill_and_descendant);
supervisor_case!(term_grace_then_kill_order);
supervisor_case!(term_leader_kill_ignoring_descendant_holding_pipes);
supervisor_case!(timestamps_use_deterministic_clock);
supervisor_case!(unexpected_exit_backoff_and_budget);
