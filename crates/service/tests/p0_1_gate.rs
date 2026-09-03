//! P0.1 process-level Gate: one fresh-process scenario against the
//! real `ocd` binary, pinned stock workerd, and a local `SigV4` S3 server.

use bytes::Bytes;
use futures::stream;
use open_compute_artifacts::{
    ArtifactCache, ArtifactRef, ArtifactStore, MapEnv, MockS3, S3ArtifactClient,
    resolve_s3_credentials_with,
};
use open_compute_core::{CacheConfig, S3Config, StartupId};
use open_compute_runtime::{RuntimeLock, load_runtime_lock, recover_orphan_for_test};
use rustix::process::{Pid, Signal, kill_process, test_kill_process};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use tempfile::TempDir;

const GATE_RESTART_BUDGET: usize = 2;
const PLATFORM_READY_TIMEOUT_SECS: u64 = 90;
const ADMIN_TOKEN: &str = "p0-1-admin";

struct Round {
    _dir: TempDir,
    prefix: String,
    bind: String,
    data: PathBuf,
    config: PathBuf,
    key: PathBuf,
    stderr: PathBuf,
    runtime_digest: String,
    child: Option<Child>,
    tracked_pids: Vec<i32>,
    tracked_ports: Vec<u16>,
    known_tokens: Vec<String>,
    ok: bool,
}

impl Drop for Round {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let pid = child.id() as i32;
            for c in child_pids(pid) {
                self.tracked_pids.push(c);
            }
            kill_tree(pid);
            let _ = child.wait();
        }
        let lease = self.data.join("runtime/child.lease");
        if let Err(error) = recover_orphan_for_test(&lease, &self.runtime_digest) {
            eprintln!(
                "P0.1 Gate orphan cleanup failed with {} for {}",
                error.code(),
                lease.display()
            );
        }
        if !self.ok {
            retain_failure(self);
        }
    }
}

#[test]
fn public_health_port_ignores_private_listener_that_appears_first() {
    let private = TcpListener::bind("127.0.0.1:0").expect("bind private listener");
    let public = TcpListener::bind("127.0.0.1:0").expect("bind public listener");
    let private_port = private.local_addr().unwrap().port();
    let public_port = public.local_addr().unwrap().port();
    let private_server = respond_once(private, 404);
    let public_server = respond_once(public, 200);

    assert_eq!(
        health_port_from(&[private_port, public_port]),
        Some(public_port)
    );
    private_server.join().expect("private response thread");
    public_server.join().expect("public response thread");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn p0_1_process_gate() {
    let workerd = required_workerd();
    let repo = repo_root();
    let lock_path = repo.join("packages/runtime/workerd.lock.json");
    let (lock, _) = load_runtime_lock(&lock_path).expect("lock");
    verify_workerd(&lock, &workerd);
    let staging_before = staging_directories();

    let s3 = MockS3::spawn("open-compute").await;
    run_round(1, &s3, &lock).await;
    assert_eq!(
        staging_directories(),
        staging_before,
        "P0.1 Gate leaked a macOS executable staging directory"
    );
    drop(s3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn round_drop_recovers_orphan_without_platform_handle() {
    let workerd = required_workerd();
    let repo = repo_root();
    let lock_path = repo.join("packages/runtime/workerd.lock.json");
    let (lock, _) = load_runtime_lock(&lock_path).expect("lock");
    verify_workerd(&lock, &workerd);
    let staging_before = staging_directories();
    let s3 = MockS3::spawn("open-compute").await;
    let mut round = setup_round(90, &s3, &lock);
    let bin = env!("CARGO_BIN_EXE_ocd");
    let env_id = "OC_S3_ID_90";
    let env_secret = "OC_S3_SECRET_90";

    spawn_ocd(&mut round, bin, env_id, env_secret);
    wait_ready(&mut round, PLATFORM_READY_TIMEOUT_SECS);
    let platform_pid = round.child.as_ref().unwrap().id() as i32;
    let workerd_pid = child_pids(platform_pid)
        .into_iter()
        .find(|&pid| pid != platform_pid)
        .expect("workerd child");
    let staged_executable = staged_executable(workerd_pid);
    wait_path(&round.data.join("runtime/child.lease"), 10);
    note_tree(&mut round, platform_pid);

    let mut platform = round.child.take().unwrap();
    let _ = kill_process(Pid::from_raw(platform_pid).unwrap(), Signal::KILL);
    let _ = platform.wait();
    assert_gone(platform_pid, "SIGKILL ocd");
    assert!(pid_alive(workerd_pid), "fixture must create a live orphan");

    round.ok = true;
    drop(round);
    assert_gone(workerd_pid, "Round::drop recovered orphan");
    if let Some(path) = staged_executable {
        assert!(
            !path.exists(),
            "staged executable leaked {}",
            path.display()
        );
        assert!(
            !path.parent().unwrap().exists(),
            "staging directory leaked {}",
            path.parent().unwrap().display()
        );
    }
    assert_eq!(
        staging_directories(),
        staging_before,
        "orphan cleanup leaked a macOS executable staging directory"
    );
    drop(s3);
}

async fn run_round(n: u32, s3: &MockS3, lock: &RuntimeLock) {
    let mut round = setup_round(n, s3, lock);
    let bin = env!("CARGO_BIN_EXE_ocd");
    let env_id = format!("OC_S3_ID_{n}");
    let env_secret = format!("OC_S3_SECRET_{n}");

    s3.clear_recorded();
    let mut starting_seen = false;
    spawn_ocd(&mut round, bin, &env_id, &env_secret);
    let pid = round.child.as_ref().unwrap().id();
    let mut ready_ok = false;
    let deadline = Instant::now() + Duration::from_secs(PLATFORM_READY_TIMEOUT_SECS);
    let mut public_port = None;
    while Instant::now() < deadline {
        if public_port.is_none() {
            public_port = public_health_port(pid as i32);
        }
        if let Some(port) = public_port {
            let live = http_status(port, "/health/live");
            let ready = http_get(port, "/health/ready");
            if live == Some(200) {
                if ready
                    .as_ref()
                    .is_some_and(|(c, b)| *c == 503 && b.contains("STARTING"))
                {
                    starting_seen = true;
                }
                if ready.as_ref().is_some_and(|(c, _)| *c == 200) {
                    ready_ok = true;
                    break;
                }
            }
        }
        if let Some(status) = round.child.as_mut().unwrap().try_wait().unwrap() {
            panic!(
                "round {n} ocd exited before readiness with {status}; stderr={}",
                read_lossy(&round.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        ready_ok,
        "round {n} never became ready; listeners={:?}; stderr={}",
        listen_ports(pid as i32),
        read_lossy(&round.stderr)
    );
    assert!(
        starting_seen,
        "round {n} never observed STARTING on /health/ready"
    );
    let port = public_port.expect("public port");
    assert_eq!(http_status(port, "/health/live"), Some(200));
    assert_eq!(http_status(port, "/health/ready"), Some(200));
    let status = http_get(port, "/client/v4/open-compute/system/status").expect("status");
    assert_eq!(status.0, 200);
    assert!(!status.1.contains("gate-secret-value"));
    assert!(!status.1.contains("gate-access"));
    let metrics = http_get(port, "/metrics").expect("metrics");
    assert!(!metrics.1.contains("gate-secret-value"));

    let workerd_pid = child_pids(pid as i32)
        .into_iter()
        .find(|&p| p != pid as i32)
        .expect("workerd child");
    let runtime_port = sole_listen_port(workerd_pid).expect("workerd listen");
    assert_ne!(
        runtime_port, port,
        "workerd must bind an ephemeral loopback port"
    );
    let token = extract_token(&round.data, lock, runtime_port).expect("token in compiled config");
    round.known_tokens.push(token.clone());
    assert_token_absent(
        &token,
        pid as i32,
        workerd_pid,
        &status.1,
        &metrics.1,
        &round.stderr,
    );
    assert!(
        probe_workerd(runtime_port, &token),
        "authenticated workerd readiness must succeed"
    );

    note_tree(&mut round, pid as i32);
    if let Some(p) = public_port {
        round.tracked_ports.push(p);
    }
    round.tracked_ports.push(runtime_port);
    assert_round_preflight(s3, &round.prefix);

    cache_survives_s3_outage(s3, &round.data).await;

    let id1 = platform_id(&round.data);
    let key1 = fs::read(&round.key).unwrap();
    term_and_wait(&mut round);
    assert_no_leaks(&round, s3);
    spawn_ocd(&mut round, bin, &env_id, &env_secret);
    wait_ready(&mut round, PLATFORM_READY_TIMEOUT_SECS);
    let id2 = {
        let port = public_health_port(round.child.as_ref().unwrap().id() as i32).unwrap();
        assert_eq!(http_status(port, "/health/ready"), Some(200));
        platform_id(&round.data)
    };
    assert_eq!(id1, id2, "platform identity must be stable across restart");
    assert_eq!(fs::read(&round.key).unwrap(), key1);

    let second = Command::new(bin)
        .args(["--config", round.config.to_str().unwrap(), "run"])
        .env(&env_id, "gate-access")
        .env(&env_secret, "gate-secret-value")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let out = second.wait_with_output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("DATA_DIR_IN_USE"),
        "second instance must fail closed: {err}"
    );
    let port = public_health_port(round.child.as_ref().unwrap().id() as i32).unwrap();
    assert_eq!(http_status(port, "/health/live"), Some(200));
    assert_eq!(http_status(port, "/health/ready"), Some(200));

    let platform_pid = round.child.as_ref().unwrap().id() as i32;
    let wpid = child_pids(platform_pid)
        .into_iter()
        .find(|&p| p != platform_pid)
        .expect("workerd");
    let _ = kill_process(Pid::from_raw(wpid).unwrap(), Signal::KILL);
    let crash_deadline = Instant::now() + Duration::from_secs(15);
    let mut saw_runtime_unready = false;
    while Instant::now() < crash_deadline {
        let live = http_status(port, "/health/live");
        let ready = http_get(port, "/health/ready");
        if live == Some(200)
            && ready.as_ref().is_some_and(|(c, b)| {
                *c == 503 && (b.contains("RUNTIME") || b.contains("STARTING"))
            })
        {
            saw_runtime_unready = true;
        }
        if live == Some(200)
            && ready.as_ref().is_some_and(|(c, _)| *c == 200)
            && saw_runtime_unready
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(saw_runtime_unready, "ready must drop during runtime crash");
    assert_eq!(http_status(port, "/health/ready"), Some(200));
    let new_wpid = child_pids(platform_pid)
        .into_iter()
        .find(|&p| p != platform_pid)
        .expect("restarted workerd");
    assert_ne!(new_wpid, wpid, "restart must use a new PID");
    let new_port = sole_listen_port(new_wpid).expect("new port");
    assert_ne!(new_port, runtime_port);
    let new_token = extract_token(&round.data, lock, new_port).expect("new token");
    round.known_tokens.push(new_token.clone());
    assert_ne!(new_token, token, "restart must mint a new internal token");

    rapid_crash_budget(&mut round, bin, &env_id, &env_secret);
    term_ignore_kill_deadline(&mut round, bin, &env_id, &env_secret);
    orphan_sigkill_recovery(&mut round, bin, &env_id, &env_secret);
    partial_startup_crashes(&mut round, bin, &env_id, &env_secret, s3);

    term_and_wait(&mut round);
    assert_no_leaks(&round, s3);
    round.ok = true;
    eprintln!("P0.1 gate scenario {n} core and crash assertions complete");
}

fn setup_round(n: u32, s3: &MockS3, lock: &RuntimeLock) -> Round {
    let dir = TempDir::new().unwrap();
    let data = dir.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let mut perms = fs::metadata(&data).unwrap().permissions();
    perms.set_mode(0o700);
    fs::set_permissions(&data, perms).unwrap();
    let key = data.join("keys").join("master.key");
    fs::create_dir_all(key.parent().unwrap()).unwrap();
    let cfg = dir.path().join("config.toml");
    let prefix = format!("round{n}/");
    let bind = "127.0.0.1:0".to_string();
    let env_id = format!("OC_S3_ID_{n}");
    let env_secret = format!("OC_S3_SECRET_{n}");
    let admin_token = dir.path().join("admin.token");
    let deployer_token = dir.path().join("deployer.token");
    let read_only_token = dir.path().join("read-only.token");
    fs::write(&admin_token, b"p0-1-admin\n").unwrap();
    fs::set_permissions(&admin_token, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&deployer_token, b"p0-1-deployer\n").unwrap();
    fs::set_permissions(&deployer_token, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&read_only_token, b"p0-1-read-only\n").unwrap();
    fs::set_permissions(&read_only_token, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(
        &cfg,
        format!(
            r#"
[server]
public_bind = "{bind}"

[server.admin_auth]
file = "{admin_token}"

[server.deployer_auth]
file = "{deployer_token}"

[server.read_only_auth]
file = "{read_only_token}"

[storage]
data_dir = "{data}"
master_key_file = "{key}"
[s3]
endpoint = "{endpoint}"
region = "auto"
bucket = "open-compute"
force_path_style = true
access_key_id_env = "{env_id}"
secret_access_key_env = "{env_secret}"
prefix = "{prefix}"
connect_timeout_ms = 2000
request_timeout_ms = 4000
max_retries = 1
retry_backoff_ms = 50
[runtime]
startup_timeout_ms = 20000
shutdown_grace_ms = 400
drain_timeout_ms = 50
kill_timeout_ms = 400
restart_budget = {restart_budget}
restart_window_ms = 15000
restart_backoff_initial_ms = 50
restart_backoff_max_ms = 200
[cache]
max_bytes = 1048576
max_artifact_bytes = 65536
"#,
            data = data.display(),
            key = key.display(),
            endpoint = s3.endpoint,
            admin_token = admin_token.display(),
            deployer_token = deployer_token.display(),
            read_only_token = read_only_token.display(),
            restart_budget = GATE_RESTART_BUDGET,
        ),
    )
    .unwrap();
    Round {
        stderr: dir.path().join("stderr.log"),
        runtime_digest: lock
            .current_target()
            .expect("current target in lock")
            .1
            .binary_sha256
            .clone(),
        _dir: dir,
        prefix,
        bind,
        data,
        config: cfg,
        key,
        child: None,
        tracked_pids: Vec::new(),
        tracked_ports: Vec::new(),
        known_tokens: Vec::new(),
        ok: false,
    }
}

fn spawn_ocd(round: &mut Round, bin: &str, env_id: &str, env_secret: &str) {
    let err = fs::File::create(&round.stderr).unwrap();
    let child = Command::new(bin)
        .args(["--config", round.config.to_str().unwrap(), "run"])
        .env(env_id, "gate-access")
        .env(env_secret, "gate-secret-value")
        .stdout(Stdio::null())
        .stderr(Stdio::from(err))
        .spawn()
        .expect("spawn ocd");
    round.child = Some(child);
}

#[track_caller]
fn wait_ready(round: &mut Round, secs: u64) {
    let pid = round.child.as_ref().unwrap().id() as i32;
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        if let Some(port) = public_health_port(pid)
            && http_status(port, "/health/ready") == Some(200)
        {
            return;
        }
        if let Some(child) = round.child.as_mut()
            && child.try_wait().ok().flatten().is_some()
        {
            panic!("ocd exited early: {}", read_lossy(&round.stderr));
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    panic!(
        "timeout waiting ready; listeners={:?}; health={:?}: {}",
        listen_ports(pid),
        public_health_port(pid)
            .and_then(|port| http_get(port, "/client/v4/open-compute/system/status")),
        read_lossy(&round.stderr)
    );
}

fn term_and_wait(round: &mut Round) {
    let Some(mut child) = round.child.take() else {
        return;
    };
    let pid = child.id() as i32;
    note_tree(round, pid);
    let _ = kill_process(Pid::from_raw(pid).unwrap(), Signal::TERM);
    let started = Instant::now();
    loop {
        if child.try_wait().ok().flatten().is_some() {
            break;
        }
        if started.elapsed() > Duration::from_secs(8) {
            let _ = kill_process(Pid::from_raw(pid).unwrap(), Signal::KILL);
            let _ = child.wait();
            panic!("SIGTERM drain exceeded deadline");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_gone(pid, "ocd after SIGTERM");
    for tracked in round.tracked_pids.clone() {
        assert_gone(tracked, "tracked child after SIGTERM");
    }
}

fn rapid_crash_budget(round: &mut Round, bin: &str, env_id: &str, env_secret: &str) {
    // Start with a fresh supervisor so the ordinary crash assertion earlier in
    // the round cannot consume this subcase's rolling budget.
    term_and_wait(round);
    spawn_ocd(round, bin, env_id, env_secret);
    wait_ready(round, PLATFORM_READY_TIMEOUT_SECS);
    let pid = round.child.as_ref().unwrap().id() as i32;
    let port = public_health_port(pid).expect("public port");
    let mut last = None;
    for i in 0..GATE_RESTART_BUDGET {
        let deadline = Instant::now() + Duration::from_secs(15);
        let wpid = loop {
            let kids: Vec<_> = child_pids(pid).into_iter().filter(|&p| p != pid).collect();
            let next = kids
                .into_iter()
                .find(|&p| last.is_none_or(|prev| p != prev));
            if http_status(port, "/health/ready") == Some(200)
                && let Some(w) = next
            {
                break w;
            }
            if Instant::now() > deadline {
                panic!(
                    "budget crash {i}: no new RUNNING workerd generation; ready={:?} status={:?}",
                    http_get(port, "/health/ready"),
                    http_get(port, "/client/v4/open-compute/system/status")
                );
            }
            std::thread::sleep(Duration::from_millis(30));
        };
        note_tree(round, wpid);
        last = Some(wpid);
        let _ = kill_process(Pid::from_raw(wpid).unwrap(), Signal::KILL);
        assert_gone(wpid, "killed generation");
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut failed = false;
    let mut last_live = None;
    let mut last_ready = None;
    let mut last_status = None;
    while Instant::now() < deadline {
        let live = http_status(port, "/health/live");
        let ready = http_get(port, "/health/ready");
        let status = http_get(port, "/client/v4/open-compute/system/status");
        last_live = live;
        last_ready = ready.clone();
        last_status = status.clone();
        if live == Some(200)
            && ready
                .as_ref()
                .is_some_and(|(c, b)| *c == 503 && b.contains("RUNTIME_INVALID"))
            && status.as_ref().is_some_and(|(c, b)| {
                // Vendor status uses readiness.as_str() for result.state (RUNTIME_INVALID)
                // and ComponentState::as_str() for components (failed), not "FAILED".
                *c == 200
                    && b.contains(r#""state":"RUNTIME_INVALID""#)
                    && b.contains(r#""name":"runtime","state":"failed""#)
            })
        {
            failed = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        failed,
        "budget exhaustion must be RUNTIME_INVALID with failed runtime; ready=503 live=200; live={last_live:?} ready={last_ready:?} status={last_status:?} children={:?}",
        child_pids(pid)
    );
    assert_eq!(http_status(port, "/health/live"), Some(200));
    let quiet = Instant::now();
    while quiet.elapsed() < Duration::from_millis(500) {
        assert!(
            child_pids(pid).is_empty(),
            "no new workerd after budget exhaustion"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    term_and_wait(round);
    spawn_ocd(round, bin, env_id, env_secret);
    wait_ready(round, PLATFORM_READY_TIMEOUT_SECS);
}

fn term_ignore_kill_deadline(round: &mut Round, bin: &str, env_id: &str, env_secret: &str) {
    let pid = round.child.as_ref().unwrap().id() as i32;
    let wpid = child_pids(pid)
        .into_iter()
        .find(|&p| p != pid)
        .expect("workerd");
    note_tree(round, wpid);
    let _ = kill_process(Pid::from_raw(wpid).unwrap(), Signal::STOP);
    let mut child = round.child.take().unwrap();
    let started = Instant::now();
    let _ = kill_process(Pid::from_raw(pid).unwrap(), Signal::TERM);
    loop {
        if child.try_wait().ok().flatten().is_some() {
            break;
        }
        if started.elapsed() > Duration::from_secs(8) {
            let _ = kill_process(Pid::from_raw(pid).unwrap(), Signal::KILL);
            let _ = kill_process(Pid::from_raw(wpid).unwrap(), Signal::CONT);
            let _ = kill_process(Pid::from_raw(wpid).unwrap(), Signal::KILL);
            let _ = child.wait();
            panic!("stopped workerd did not finish within outer deadline");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_millis(400),
        "stopped workerd must consume TERM grace before KILL, elapsed={elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(8),
        "must finish within outer deadline, elapsed={elapsed:?}"
    );
    assert_gone(pid, "ocd after forced KILL path");
    assert_gone(wpid, "stopped workerd after KILL path");
    spawn_ocd(round, bin, env_id, env_secret);
    wait_ready(round, PLATFORM_READY_TIMEOUT_SECS);
}

fn orphan_sigkill_recovery(round: &mut Round, bin: &str, env_id: &str, env_secret: &str) {
    let pid = round.child.as_ref().unwrap().id() as i32;
    let wpid = child_pids(pid)
        .into_iter()
        .find(|&p| p != pid)
        .expect("workerd");
    note_tree(round, wpid);
    let mut child = round.child.take().unwrap();
    let _ = kill_process(Pid::from_raw(pid).unwrap(), Signal::KILL);
    let _ = child.wait();
    assert_gone(pid, "SIGKILL ocd");
    assert!(
        pid_alive(wpid),
        "workerd orphan must outlive SIGKILL of ocd"
    );
    spawn_ocd(round, bin, env_id, env_secret);
    wait_ready(round, PLATFORM_READY_TIMEOUT_SECS);
    assert_gone(
        wpid,
        "previous orphan must be gone before replacement is accepted",
    );
    let new_pid = round.child.as_ref().unwrap().id() as i32;
    let new_w = child_pids(new_pid)
        .into_iter()
        .find(|&p| p != new_pid)
        .expect("replacement workerd");
    assert_ne!(new_w, wpid);
}

fn partial_startup_crashes(
    round: &mut Round,
    bin: &str,
    env_id: &str,
    env_secret: &str,
    s3: &MockS3,
) {
    term_and_wait(round);
    assert_no_leaks(round, s3);
    switch_to_fresh_data(round, "partial-master-key");
    assert!(!round.key.exists());

    kill_before_ready(round, bin, env_id, env_secret, "master-key", |r, pid| {
        wait_path(&r.key, 10);
        assert_pre_ready(pid, "master-key");
    });
    recover_partial_state(round, bin, env_id, env_secret, s3, "master-key");

    term_and_wait(round);
    assert_no_leaks(round, s3);
    switch_to_fresh_data(round, "partial-control-db");
    assert!(!round.data.join("control.sqlite").exists());
    kill_before_ready(round, bin, env_id, env_secret, "control-db", |r, pid| {
        wait_path(&r.data.join("control.sqlite"), 10);
        assert_pre_ready(pid, "control-db");
    });
    recover_partial_state(round, bin, env_id, env_secret, s3, "control-db");

    term_and_wait(round);
    assert_no_leaks(round, s3);
    switch_to_fresh_data(round, "partial-runtime-config");
    assert!(!has_runtime_config(&round.data));
    kill_before_ready(
        round,
        bin,
        env_id,
        env_secret,
        "runtime-config",
        |r, pid| {
            // This observes the complete first-start pipeline, including embedded payload
            // materialization, rather than only the bounded workerd compile subprocess.
            wait_runtime_config(&r.data, PLATFORM_READY_TIMEOUT_SECS);
            assert_pre_ready(pid, "runtime-config");
        },
    );
    recover_partial_state(round, bin, env_id, env_secret, s3, "runtime-config");
}

fn switch_to_fresh_data(round: &mut Round, label: &str) {
    assert!(round.child.is_none());
    let old = round.data.clone();
    let next = round._dir.path().join(label);
    assert!(!next.exists(), "fresh crash fixture must not pre-exist");
    let config = fs::read_to_string(&round.config).expect("read round config");
    let old_text = old.to_str().expect("UTF-8 temporary data path");
    let next_text = next.to_str().expect("UTF-8 temporary data path");
    assert!(
        config.contains(old_text),
        "config must reference current data dir"
    );
    fs::write(&round.config, config.replace(old_text, next_text)).expect("rewrite round config");
    fs::create_dir(&next).expect("create fresh data dir");
    let mut perms = fs::metadata(&next).unwrap().permissions();
    perms.set_mode(0o700);
    fs::set_permissions(&next, perms).unwrap();
    round.data = next;
    round.key = round.data.join("keys/master.key");
}

fn recover_partial_state(
    round: &mut Round,
    bin: &str,
    env_id: &str,
    env_secret: &str,
    s3: &MockS3,
    boundary: &str,
) {
    spawn_ocd(round, bin, env_id, env_secret);
    wait_ready(round, PLATFORM_READY_TIMEOUT_SECS);
    let identity = platform_id(&round.data);
    term_and_wait(round);
    assert_no_leaks(round, s3);
    spawn_ocd(round, bin, env_id, env_secret);
    wait_ready(round, PLATFORM_READY_TIMEOUT_SECS);
    assert_eq!(
        platform_id(&round.data),
        identity,
        "{boundary} recovery must not create a second authority"
    );
}

fn kill_before_ready(
    round: &mut Round,
    bin: &str,
    env_id: &str,
    env_secret: &str,
    boundary: &str,
    wait: impl FnOnce(&Round, i32),
) {
    spawn_ocd(round, bin, env_id, env_secret);
    let pid = round.child.as_ref().unwrap().id() as i32;
    wait(round, pid);
    note_tree(round, pid);
    let mut child = round.child.take().unwrap();
    let _ = kill_process(Pid::from_raw(pid).unwrap(), Signal::KILL);
    let _ = child.wait();
    assert_gone(pid, boundary);
}

fn assert_pre_ready(pid: i32, boundary: &str) {
    if let Some(port) = public_health_port(pid) {
        let ready = http_status(port, "/health/ready");
        assert_ne!(
            ready,
            Some(200),
            "{boundary} kill happened after READY, not a startup crash"
        );
    }
}

async fn cache_survives_s3_outage(s3: &MockS3, data: &Path) {
    let s3_cfg = S3Config {
        endpoint: s3.endpoint.clone(),
        access_key_id_env: Some("OC_S3_ID_1".into()),
        secret_access_key_env: Some("OC_S3_SECRET_1".into()),
        prefix: "artifacts/".into(),
        max_retries: 1,
        connect_timeout_ms: 1000,
        request_timeout_ms: 2000,
        ..S3Config::default()
    };
    let map = MapEnv::new()
        .with("OC_S3_ID_1", "gate-access")
        .with("OC_S3_SECRET_1", "gate-secret-value");
    let creds = resolve_s3_credentials_with(&s3_cfg, &map).expect("resolve Gate S3 credentials");
    let client = S3ArtifactClient::connect(&s3_cfg, &creds, 65_536).unwrap();
    let store = ArtifactStore::new(client);
    let body = b"immutable-cache-body".to_vec();
    let digest = hex::encode(Sha256::digest(&body));
    let stream = stream::iter([Ok::<_, std::io::Error>(Bytes::from(body.clone()))]);
    let artifact = store
        .put_verified(stream, &digest, body.len() as u64)
        .await
        .expect("put");
    let cache_root = data.join("cache/artifacts");
    let cache = ArtifactCache::open(cache_root, CacheConfig::default(), StartupId::generate())
        .expect("cache");
    let mut pinned = cache.acquire(&store, &artifact).await.expect("cold");
    assert_eq!(pinned.read_all().unwrap(), body);
    s3.set_fault(open_compute_artifacts::Fault::Timeout);
    let mut hit = cache.acquire_cached(&artifact).await.expect("cached hit");
    assert_eq!(hit.read_all().unwrap(), body);
    s3.set_fault(open_compute_artifacts::Fault::None);
    let _ = ArtifactRef::new(1, &digest, body.len() as u64);
}

fn required_workerd() -> PathBuf {
    let path = std::env::var("OPEN_COMPUTE_TEST_WORKERD").unwrap_or_default();
    assert!(
        !path.is_empty(),
        "OPEN_COMPUTE_TEST_WORKERD is required; refusing to skip the Gate"
    );
    let p = PathBuf::from(path);
    assert!(
        p.is_absolute(),
        "OPEN_COMPUTE_TEST_WORKERD must be absolute"
    );
    assert!(p.is_file(), "OPEN_COMPUTE_TEST_WORKERD is not a file");
    p
}

fn verify_workerd(lock: &RuntimeLock, binary: &Path) {
    let target = lock.current_target().expect("current target in lock").1;
    let bytes = fs::read(binary).expect("read workerd");
    let digest = hex::encode(Sha256::digest(&bytes));
    assert_eq!(
        digest, target.binary_sha256,
        "test workerd hash must match the formal lock"
    );
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn http_status(port: u16, path: &str) -> Option<u16> {
    http_get(port, path).map(|(c, _)| c)
}

fn http_get(port: u16, path: &str) -> Option<(u16, String)> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    let auth = if path.starts_with("/client/v4/") || path == "/metrics" {
        format!("Authorization: Bearer {ADMIN_TOKEN}\r\n")
    } else {
        String::new()
    };
    let req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n{auth}Connection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).ok()?;
    let mut buf = Vec::new();
    let _ = stream.read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf);
    let code = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())?;
    let body = text.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    Some((code, body))
}

fn probe_workerd(port: u16, token: &str) -> bool {
    let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) else {
        return false;
    };
    let req = format!(
        "GET {} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{}: {token}\r\nConnection: close\r\n\r\n",
        open_compute_runtime::READY_PATH,
        open_compute_runtime::TOKEN_HEADER
    );
    let _ = stream.write_all(req.as_bytes());
    let mut buf = Vec::new();
    let _ = stream.read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf);
    text.contains("204") || text.contains("200")
}

fn listen_ports(pid: i32) -> Vec<u16> {
    let out = Command::new("/usr/sbin/lsof")
        .args(["-nP", "-p", &pid.to_string(), "-a", "-iTCP", "-sTCP:LISTEN"])
        .output()
        .or_else(|first| {
            Command::new("lsof")
                .args(["-nP", "-p", &pid.to_string(), "-a", "-iTCP", "-sTCP:LISTEN"])
                .output()
                .map_err(|_| first)
        })
        .expect("lsof is required by the process Gate");
    let code = out.status.code();
    assert!(
        matches!(code, Some(0 | 1)),
        "lsof failed for pid {pid}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    let mut ports = Vec::new();
    for line in text.lines().filter(|line| line.contains("(LISTEN)")) {
        let fields: Vec<_> = line.split_whitespace().collect();
        let Some(listen_index) = fields.iter().position(|field| *field == "(LISTEN)") else {
            continue;
        };
        let Some(address) = listen_index
            .checked_sub(1)
            .and_then(|index| fields.get(index))
        else {
            continue;
        };
        let Some((_, port)) = address.rsplit_once(':') else {
            continue;
        };
        if let Ok(port) = port.parse::<u16>()
            && port != 0
            && !ports.contains(&port)
        {
            ports.push(port);
        }
    }
    ports
}

fn health_port_from(ports: &[u16]) -> Option<u16> {
    ports
        .iter()
        .copied()
        .find(|&port| http_status(port, "/health/live") == Some(200))
}

fn public_health_port(pid: i32) -> Option<u16> {
    health_port_from(&listen_ports(pid))
}

fn sole_listen_port(pid: i32) -> Option<u16> {
    let ports = listen_ports(pid);
    match ports.as_slice() {
        [] => None,
        [port] => Some(*port),
        _ => panic!("pid {pid} has multiple listener ports: {ports:?}"),
    }
}

fn respond_once(listener: TcpListener, status: u16) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept health probe");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set probe read timeout");
        let mut request = [0u8; 1024];
        let _ = stream.read(&mut request);
        let reason = if status == 200 { "OK" } else { "Not Found" };
        write!(
            stream,
            "HTTP/1.1 {status} {reason}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )
        .expect("write health response");
    })
}

fn staged_executable(pid: i32) -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let out = Command::new("lsof")
            .args(["-p", &pid.to_string(), "-a", "-d", "txt", "-Fn"])
            .stdin(Stdio::null())
            .output()
            .expect("lsof is required by the process Gate");
        assert!(out.status.success(), "lsof failed for workerd pid {pid}");
        let text = String::from_utf8(out.stdout).expect("lsof output must be UTF-8");
        text.lines()
            .filter_map(|line| line.strip_prefix('n'))
            .map(PathBuf::from)
            .find(|path| {
                path.file_name().is_some_and(|name| name == "workerd")
                    && path
                        .parent()
                        .and_then(Path::file_name)
                        .is_some_and(|name| name.to_string_lossy().starts_with("oc-exec-"))
            })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pid;
        None
    }
}

fn staging_directories() -> BTreeSet<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        fs::read_dir(std::env::temp_dir())
            .expect("inspect temporary directory")
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("oc-exec-"))
            .map(|entry| entry.path())
            .collect()
    }
    #[cfg(not(target_os = "macos"))]
    {
        BTreeSet::new()
    }
}

fn child_pids(parent: i32) -> Vec<i32> {
    let out = Command::new("pgrep")
        .args(["-P", &parent.to_string()])
        .output()
        .expect("pgrep is required by the process Gate");
    match out.status.code() {
        Some(0) => {}
        Some(1) => return Vec::new(),
        code => panic!(
            "pgrep failed for parent {parent} with {code:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        ),
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|line| {
            line.trim()
                .parse()
                .unwrap_or_else(|_| panic!("pgrep returned a non-PID line: {line:?}"))
        })
        .collect()
}

fn pid_alive(pid: i32) -> bool {
    let raw = Pid::from_raw(pid).expect("tracked PID must be positive");
    match test_kill_process(raw) {
        Ok(()) => true,
        Err(err) if err == rustix::io::Errno::SRCH => false,
        Err(err) => panic!("failed to probe pid {pid}: {err}"),
    }
}

fn assert_gone(pid: i32, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while pid_alive(pid) {
        if Instant::now() >= deadline {
            panic!("{what} pid {pid} still live after wait deadline");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn note_tree(round: &mut Round, pid: i32) {
    round.tracked_pids.push(pid);
    for c in child_pids(pid) {
        round.tracked_pids.push(c);
        round.tracked_ports.extend(listen_ports(c));
    }
    round.tracked_ports.extend(listen_ports(pid));
}

fn assert_round_preflight(s3: &MockS3, prefix: &str) {
    let rec: Vec<_> = s3
        .recorded()
        .into_iter()
        .filter(|r| r.path.contains(prefix.trim_end_matches('/')) && r.path.contains("preflight"))
        .collect();
    let methods: Vec<_> = rec.iter().map(|r| r.method.as_str()).collect();
    assert_eq!(
        methods,
        ["PUT", "HEAD", "GET", "DELETE", "HEAD"],
        "round prefix {prefix} methods {methods:?}"
    );
    assert!(rec.iter().all(|r| r.has_authorization));
    let leftover: Vec<_> = s3
        .keys()
        .into_iter()
        .filter(|k| k.contains("preflight") && k.contains(prefix.trim_end_matches('/')))
        .collect();
    assert!(leftover.is_empty(), "canary left behind {leftover:?}");
}

fn kill_tree(pid: i32) {
    for c in child_pids(pid) {
        kill_tree(c);
    }
    if let Some(raw) = Pid::from_raw(pid) {
        let _ = kill_process(raw, Signal::KILL);
    }
}

fn extract_token(data: &Path, lock: &RuntimeLock, runtime_port: u16) -> Option<String> {
    let skip: Vec<String> = lock
        .targets
        .values()
        .flat_map(|t| [t.binary_sha256.clone(), t.archive_sha256.clone()])
        .collect();
    let runtime = data.join("runtime");
    let entries = fs::read_dir(runtime).ok()?;
    let mut candidates = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("bin")
            && let Ok(bytes) = fs::read(&path)
        {
            let text = String::from_utf8_lossy(&bytes);
            for c in extract_hex64(&text) {
                if !skip.iter().any(|s| s == &c) && !candidates.contains(&c) {
                    candidates.push(c);
                }
            }
        }
    }
    candidates
        .into_iter()
        .find(|c| probe_workerd(runtime_port, c))
}

fn extract_hex64(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i + 64 <= bytes.len() {
        if bytes[i..i + 64]
            .iter()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            let s = String::from_utf8_lossy(&bytes[i..i + 64]).into_owned();
            if !out.contains(&s) {
                out.push(s);
            }
            i += 64;
        } else {
            i += 1;
        }
    }
    out
}

fn assert_token_absent(
    token: &str,
    platform: i32,
    workerd: i32,
    status: &str,
    metrics: &str,
    stderr: &Path,
) {
    assert!(!status.contains(token));
    assert!(!metrics.contains(token));
    assert!(!read_lossy(stderr).contains(token));
    for pid in [platform, workerd] {
        let out = Command::new("/bin/ps")
            .args(["eww", "-p", &pid.to_string()])
            .output()
            .expect("/bin/ps must succeed to assert token absence");
        assert!(
            out.status.success(),
            "/bin/ps failed for pid {pid}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(!text.contains(token), "token leaked in process listing");
    }
}

fn platform_id(data: &Path) -> String {
    let raw = fs::read(data.join("platform.lock")).expect("lock metadata");
    let v: serde_json::Value = serde_json::from_slice(&raw).expect("lock json");
    v.get("platform_id")
        .and_then(|x| x.as_str())
        .expect("platform_id")
        .to_owned()
}

fn wait_path(path: &Path, secs: u64) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        if path.exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {}", path.display());
}

fn wait_runtime_config(data: &Path, secs: u64) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        if has_runtime_config(data) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for compiled config");
}

fn has_runtime_config(data: &Path) -> bool {
    match fs::read_dir(data.join("runtime")) {
        Ok(rd) => rd
            .map(|entry| entry.expect("inspect runtime config entry"))
            .any(|e| {
                e.path()
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("config.") && n.ends_with(".bin"))
            }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
        Err(err) => panic!("failed to inspect runtime config directory: {err}"),
    }
}

fn assert_no_leaks(round: &Round, s3: &MockS3) {
    assert!(round.child.is_none());
    for pid in &round.tracked_pids {
        assert_gone(*pid, "tracked pid");
    }
    for port in &round.tracked_ports {
        assert!(
            TcpStream::connect(("127.0.0.1", *port)).is_err(),
            "port {port} still bound"
        );
    }
    let lock = round.data.join("platform.lock");
    if lock.exists() {
        assert!(
            open_compute_storage::DataDirLock::probe_available(&lock)
                .expect("lock probe must succeed"),
            "data dir lock still held"
        );
    }
    assert_no_partials(&round.data);
    assert!(
        !round.data.join("runtime/child.staging").exists(),
        "runtime staging journal leaked"
    );
    let leftover: Vec<_> = s3
        .keys()
        .into_iter()
        .filter(|k| k.contains("preflight") && k.contains(round.prefix.trim_end_matches('/')))
        .collect();
    assert!(leftover.is_empty(), "S3 canary leaked {leftover:?}");
    let _ = round.bind.as_str();
}

fn assert_no_partials(root: &Path) {
    fn walk(path: &Path) {
        let rd = fs::read_dir(path)
            .unwrap_or_else(|err| panic!("failed to inspect {}: {err}", path.display()));
        for entry in rd {
            let e = entry.unwrap_or_else(|err| {
                panic!("failed to inspect an entry under {}: {err}", path.display())
            });
            let p = e.path();
            let name = e.file_name();
            let name = name.to_string_lossy();
            assert!(
                !name.contains(".partial") && !name.starts_with(".work") && name != ".tmp",
                "partial file leaked {}",
                p.display()
            );
            if p.is_dir() {
                walk(&p);
            }
        }
    }
    assert!(root.is_dir(), "expected data root {}", root.display());
    walk(root);
}

fn read_lossy(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

fn retain_failure(round: &Round) {
    let dest = repo_root().join(".temp/p0-1-run/failed").join(format!(
        "{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::create_dir_all(&dest);
    if let Ok(mut file) = fs::File::open(&round.stderr) {
        const MAX_RETAINED_STDERR: u64 = 64 * 1024;
        let len = file.metadata().map_or(0, |m| m.len());
        let _ = file.seek(SeekFrom::Start(len.saturating_sub(MAX_RETAINED_STDERR)));
        let mut raw = Vec::new();
        let _ = file.take(MAX_RETAINED_STDERR).read_to_end(&mut raw);
        let mut redacted = String::from_utf8_lossy(&raw)
            .replace("gate-secret-value", "[redacted]")
            .replace("gate-access", "[redacted]");
        for token in &round.known_tokens {
            redacted = redacted.replace(token, "[redacted-token]");
        }
        assert!(!redacted.contains("gate-secret-value"));
        assert!(!redacted.contains("gate-access"));
        assert!(
            round
                .known_tokens
                .iter()
                .all(|token| !redacted.contains(token))
        );
        let _ = fs::write(dest.join("stderr.log"), redacted);
    }
    let control = round.data.join("control.sqlite");
    let diagnostic_control = control
        .parent()
        .and_then(|parent| fs::canonicalize(parent).ok())
        .and_then(|parent| control.file_name().map(|name| parent.join(name)))
        .unwrap_or_else(|| control.clone());
    let opened = rusqlite::Connection::open_with_flags(
        &diagnostic_control,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
    );
    let mut diagnostic = format!(
        "path_metadata={:?}\n",
        fs::symlink_metadata(&control)
            .map(|metadata| (metadata.len(), metadata.file_type().is_file()))
    );
    if let Ok(connection) = &opened {
        let version =
            connection.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0));
        diagnostic.push_str(&format!("user_version={version:?}\n"));
        match connection
            .prepare("SELECT key, typeof(value), length(value) FROM platform_meta ORDER BY key")
        {
            Ok(mut statement) => match statement.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            }) {
                Ok(rows) => {
                    for row in rows {
                        diagnostic.push_str(&format!("meta={row:?}\n"));
                    }
                }
                Err(error) => diagnostic.push_str(&format!("meta_query={error:?}\n")),
            },
            Err(error) => diagnostic.push_str(&format!("meta_prepare={error:?}\n")),
        }
    } else {
        diagnostic.push_str(&format!("open={opened:?}\n"));
    }
    let _ = fs::write(dest.join("database-diagnostic.log"), diagnostic);
}
