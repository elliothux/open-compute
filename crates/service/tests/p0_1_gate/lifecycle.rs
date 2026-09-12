use super::*;

pub(super) fn setup_round(n: u32, s3: &MockS3, lock: &RuntimeLock) -> Round {
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
    let r2_prefix = format!("tenant/r2/authority-{n}/");
    let bind = "127.0.0.1:0".to_string();
    let env_id = format!("OC_S3_ID_{n}");
    let env_secret = format!("OC_S3_SECRET_{n}");
    let admin_token = dir.path().join("admin.token");
    let deployer_token = dir.path().join("deployer.token");
    let read_only_token = dir.path().join("read-only.token");
    fs::write(&admin_token, b"p0-1-admin\n").unwrap();
    fs::write(&deployer_token, b"p0-1-deployer\n").unwrap();
    fs::write(&read_only_token, b"p0-1-read-only\n").unwrap();
    fs::set_permissions(&admin_token, fs::Permissions::from_mode(0o600)).unwrap();
    fs::set_permissions(&deployer_token, fs::Permissions::from_mode(0o600)).unwrap();
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

[data]
path = "{data}"
master_key_file = "{key}"
[storage]
backend = "s3"
endpoint = "{endpoint}"
region = "auto"
bucket = "open-compute"
force_path_style = true
access_key_id_env = "{env_id}"
secret_access_key_env = "{env_secret}"
prefix = "{prefix}"
r2_prefix = "{r2_prefix}"
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
        r2_prefix,
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

pub(super) fn spawn_ocd(round: &mut Round, bin: &str, env_id: &str, env_secret: &str) {
    let err = fs::File::create(&round.stderr).unwrap();
    let child = Command::new(bin)
        .args(["--config", round.config.to_str().unwrap(), "run"])
        .env("XDG_STATE_HOME", round._dir.path().join("state"))
        .env(env_id, "gate-access")
        .env(env_secret, "gate-secret-value")
        .stdout(Stdio::null())
        .stderr(Stdio::from(err))
        .spawn()
        .expect("spawn ocd");
    round.child = Some(child);
}

#[track_caller]
pub(super) fn wait_ready(round: &mut Round, secs: u64) {
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

pub(super) fn term_and_wait(round: &mut Round) {
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

pub(super) fn rapid_crash_budget(round: &mut Round, bin: &str, env_id: &str, env_secret: &str) {
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
                if *c != 200 {
                    return false;
                }
                let Ok(body) = serde_json::from_str::<serde_json::Value>(b) else {
                    return false;
                };
                body["result"]["state"].as_str() == Some("RUNTIME_INVALID")
                    && body["result"]["components"]
                        .as_array()
                        .is_some_and(|components| {
                            components.iter().any(|component| {
                                component["name"].as_str() == Some("runtime")
                                    && component["state"].as_str() == Some("failed")
                                    && component["message"].as_str() == Some("RUNTIME_INVALID")
                            })
                        })
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

pub(super) fn term_ignore_kill_deadline(
    round: &mut Round,
    bin: &str,
    env_id: &str,
    env_secret: &str,
) {
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

pub(super) fn orphan_sigkill_recovery(
    round: &mut Round,
    bin: &str,
    env_id: &str,
    env_secret: &str,
) {
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

pub(super) fn partial_startup_crashes(
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

pub(super) fn switch_to_fresh_data(round: &mut Round, label: &str) {
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
    let next_prefix = format!("{}{label}/", round.prefix);
    let next_r2_prefix = format!("{}{label}/", round.r2_prefix);
    let old_prefix_line = format!("prefix = \"{}\"", round.prefix);
    let next_prefix_line = format!("prefix = \"{next_prefix}\"");
    let old_r2_prefix_line = format!("r2_prefix = \"{}\"", round.r2_prefix);
    let next_r2_prefix_line = format!("r2_prefix = \"{next_r2_prefix}\"");
    assert!(config.contains(&old_prefix_line));
    assert!(config.contains(&old_r2_prefix_line));
    fs::write(
        &round.config,
        config
            .replace(old_text, next_text)
            .replace(&old_prefix_line, &next_prefix_line)
            .replace(&old_r2_prefix_line, &next_r2_prefix_line),
    )
    .expect("rewrite round config");
    fs::create_dir(&next).expect("create fresh data dir");
    let mut perms = fs::metadata(&next).unwrap().permissions();
    perms.set_mode(0o700);
    fs::set_permissions(&next, perms).unwrap();
    round.data = next;
    round.key = round.data.join("keys/master.key");
    round.prefix = next_prefix;
    round.r2_prefix = next_r2_prefix;
}

pub(super) fn recover_partial_state(
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

pub(super) fn kill_before_ready(
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

pub(super) fn assert_pre_ready(pid: i32, boundary: &str) {
    if let Some(port) = public_health_port(pid) {
        let ready = http_status(port, "/health/ready");
        assert_ne!(
            ready,
            Some(200),
            "{boundary} kill happened after READY, not a startup crash"
        );
    }
}
