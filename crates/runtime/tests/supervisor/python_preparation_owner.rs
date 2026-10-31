use super::*;
use open_compute_runtime::{
    GenerationAuthRegistry, PythonPreparationOptions, PythonPreparationProcess,
};

fn options(
    runtime: &open_compute_runtime::VerifiedRuntime,
    root: &Path,
    token: &str,
    mode: &str,
) -> PythonPreparationOptions {
    fs::create_dir_all(root.join("runtime")).unwrap();
    let config = serde_json::json!({
        "mode": mode, "token": token,
        "argv_path": root.join("argv.json"),
        "stdin_marker_path": root.join("stdin.json"),
    });
    let compiled = CompiledConfig::preparation_from_bytes_for_test(
        &root.join("configs"),
        &uuid_digest(),
        &serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    PythonPreparationOptions {
        runtime: runtime.clone(),
        compiled,
        token: SecretString::new(token),
        startup_timeout: Duration::from_secs(5),
        lease_path: root.join("runtime/prepare.lease"),
        external_services: [
            "runtime-source",
            "binding-backend",
            "observability-backend",
            "do-router",
        ]
        .map(|name| ExternalServiceAddress::loopback(name, "127.0.0.1:1".parse().unwrap()).unwrap())
        .to_vec(),
        host_extension_fd: None,
    }
}

fn uuid_digest() -> String {
    StartupId::generate().to_string().replace('-', "")
}

async fn lease_pgid(path: &Path) -> i32 {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(bytes) = fs::read(path) {
            let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            return value["pgid"].as_i64().unwrap() as i32;
        }
        assert!(tokio::time::Instant::now() < deadline, "lease deadline");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

pub(super) async fn run() {
    let dir = TempDir::new().unwrap();
    let runtime = verified(dir.path()).await;
    fs::create_dir(dir.path().join("runtime")).unwrap();
    let parent_stdin = dir.path().join("parent-stdin.json");
    let parent = WorkerdSupervisor::new(
        WorkerdSupervisorOptions {
            runtime: runtime.clone(),
            compiler: compiler(
                dir.path().join("parent-config"),
                "ready",
                None,
                serde_json::json!({"stdin_marker_path": parent_stdin}),
            ),
            config: small_cfg(),
            clock: Arc::new(DeterministicClock::new(UNIX_EPOCH)),
            jitter: Arc::new(SequenceJitter::new(vec![0])),
            lease_path: Some(dir.path().join("runtime/parent.lease")),
        },
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    parent.start();
    let before = wait_state(&parent, SupervisorState::Running).await;
    let body: serde_json::Value = serde_json::from_slice(&fs::read(parent_stdin).unwrap()).unwrap();
    let token = body["token"].as_str().unwrap();
    let auth = GenerationAuthRegistry::new();
    auth.activate_for_test(SecretString::new(token));
    assert!(auth.authorize(token, "instance-generation"));
    let credential = auth.credential().unwrap();
    let launch = options(&runtime, dir.path(), token, "ignore_term");
    let lease = launch.lease_path.clone();
    assert!(!format!("{launch:?}").contains(token));
    let child = PythonPreparationProcess::start(launch).await.unwrap();
    let pid = lease_pgid(&lease).await;
    assert_ne!(pid, before.pid.unwrap());
    assert!(!format!("{child:?}").contains(token));
    assert!(!format!("{child:?}").contains(&child.listen_port().to_string()));
    probe_ready_with_raw_token(child.listen_port(), token, Duration::from_secs(1))
        .await
        .unwrap();
    probe_ready_with_raw_token(child.listen_port(), &"0".repeat(64), Duration::from_secs(1))
        .await
        .unwrap_err();
    let argv: Vec<String> =
        serde_json::from_slice(&fs::read(dir.path().join("argv.json")).unwrap()).unwrap();
    assert!(argv.contains(&"--control-fd=3".to_owned()));
    assert!(!argv.contains(&"--host-extension-fd=4".to_owned()));
    assert_eq!(
        argv.iter()
            .filter(|arg| arg.starts_with("--external-addr="))
            .count(),
        4
    );
    assert!(
        !argv
            .iter()
            .any(|arg| arg.starts_with("--directory-path=") || arg.contains(token))
    );
    assert!(auth.with_current(&credential, || ()).is_some());
    assert!(!auth.authorize(token, "other-generation"));
    child
        .shutdown(Duration::from_millis(10), Duration::from_millis(50))
        .await
        .unwrap();
    assert_reaped(Some(pid)).unwrap();
    assert!(!lease.exists());
    assert_eq!(parent.snapshot().startup_id, before.startup_id);
    probe_ready_with_raw_token(before.listen_port.unwrap(), token, Duration::from_secs(1))
        .await
        .unwrap();

    // Drop still owns cleanup; the retained dead lease is recovered on the next start.
    let child = PythonPreparationProcess::start(options(&runtime, dir.path(), token, "ready"))
        .await
        .unwrap();
    let dropped_pid = lease_pgid(&lease).await;
    drop(child);
    wait_reaped(dropped_pid, Duration::from_secs(3)).unwrap();
    assert!(lease.exists());
    let child = PythonPreparationProcess::start(options(&runtime, dir.path(), token, "ready"))
        .await
        .unwrap();
    assert_ne!(lease_pgid(&lease).await, dropped_pid);
    child
        .shutdown(Duration::ZERO, Duration::from_secs(1))
        .await
        .unwrap();
    assert!(!lease.exists());

    // Cancellation while spawn is blocking cannot abandon its later child.
    clear_blocking_spawn_hold();
    hold_blocking_spawn();
    struct ReleaseHeldSpawn;
    impl Drop for ReleaseHeldSpawn {
        fn drop(&mut self) {
            clear_blocking_spawn_hold();
        }
    }
    let _held = ReleaseHeldSpawn;
    let launch = options(&runtime, dir.path(), token, "ready");
    let task = tokio::spawn(PythonPreparationProcess::start(launch));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while !blocking_spawn_is_waiting() {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let previous_pid = last_spawned_pid().unwrap();
    release_blocking_spawn();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let pid = last_spawned_pid().unwrap();
        if pid != previous_pid && !pid_alive(pid) && !lease.exists() {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "cancelled spawn cleanup deadline"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    clear_blocking_spawn_hold();
    parent.shutdown().await;
    assert_reaped(before.pid).unwrap();
}

pub(super) async fn rejection_matrix() {
    let dir = TempDir::new().unwrap();
    let runtime = verified(dir.path()).await;
    let token = "a".repeat(64);
    for fault in [
        "role",
        "services",
        "duplicate",
        "token",
        "deadline",
        "overflow",
        "path",
        "symlink",
        "corrupt",
    ] {
        let mut launch = options(&runtime, dir.path(), &token, "ready");
        match fault {
            "role" => {
                launch.compiled = CompiledConfig::from_bytes_for_test(
                    &dir.path().join("configs"),
                    &uuid_digest(),
                    b"ordinary runtime",
                )
                .unwrap();
            }
            "services" => {
                launch.external_services.pop();
            }
            "duplicate" => launch.external_services[0] = launch.external_services[1].clone(),
            "token" => launch.token = SecretString::new("invalid"),
            "deadline" => launch.startup_timeout = Duration::ZERO,
            "overflow" => launch.startup_timeout = Duration::MAX,
            "path" => launch.lease_path = PathBuf::from("relative.lease"),
            "symlink" => {
                let link = dir.path().join("linked");
                std::os::unix::fs::symlink(dir.path(), &link).unwrap();
                launch.lease_path = link.join("child.lease");
            }
            "corrupt" => fs::write(launch.compiled.path(), b"changed").unwrap(),
            _ => unreachable!(),
        }
        let error = PythonPreparationProcess::start(launch).await.unwrap_err();
        assert!(!format!("{error:?}").contains(&token));
        assert!(!dir.path().join("runtime/prepare.lease").exists());
        assert!(
            !dir.path().join("stdin.json").exists(),
            "fault {fault} must fail before exec"
        );
    }
    for mode in ["early_exit", "malformed_control", "timeout", "slow_probe"] {
        let mut launch = options(&runtime, dir.path(), &token, mode);
        launch.startup_timeout = Duration::from_millis(300);
        PythonPreparationProcess::start(launch).await.unwrap_err();
        assert!(!dir.path().join("runtime/prepare.lease").exists());
        if let Some(pid) = last_spawned_pid() {
            assert_reaped(Some(pid)).unwrap();
        }
    }
    let child = PythonPreparationProcess::start(options(&runtime, dir.path(), &token, "ready"))
        .await
        .unwrap();
    let lease = dir.path().join("runtime/prepare.lease");
    let pid = lease_pgid(&lease).await;
    open_compute_runtime::set_reap_probe_fail(true);
    let result = child.shutdown(Duration::ZERO, Duration::from_secs(1)).await;
    open_compute_runtime::set_reap_probe_fail(false);
    assert!(result.is_err());
    assert!(
        lease.exists(),
        "failed reap proof must retain recovery authority"
    );
    wait_reaped(pid, Duration::from_secs(2)).unwrap();
    let child = PythonPreparationProcess::start(options(&runtime, dir.path(), &token, "ready"))
        .await
        .unwrap();
    child
        .shutdown(Duration::ZERO, Duration::from_secs(1))
        .await
        .unwrap();
    assert!(!lease.exists());
    set_reader_fail_point();
    let started =
        PythonPreparationProcess::start(options(&runtime, dir.path(), &token, "ready")).await;
    let child = started.unwrap();
    let reader_failure = child.shutdown(Duration::ZERO, Duration::from_secs(1)).await;
    assert!(
        reader_failure.is_err(),
        "reader failure must fail preparation shutdown"
    );
    assert!(
        !lease.exists(),
        "confirmed reaping still permits lease cleanup"
    );
}
