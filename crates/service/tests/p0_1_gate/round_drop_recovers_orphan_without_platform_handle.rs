use super::*;

pub(super) async fn run() {
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
