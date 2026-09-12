use super::*;

pub(super) async fn run_round(n: u32, s3: &MockS3, lock: &RuntimeLock) {
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
        .env("XDG_STATE_HOME", round._dir.path().join("state"))
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
