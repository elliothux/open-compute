use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_failure_drop_still_reaps_group() {
    let dir = TempDir::new().unwrap();
    copy_formal_assets(dir.path());
    let pid_file = dir.path().join("pid");
    let child_file = dir.path().join("child");
    let bin = dir.path().join("workerd");
    write_exec(
        &bin,
        &format!(
            "#!/bin/sh
if [ \"$1\" = \"--version\" ]; then echo '{VERSION}'; exit 0; fi
echo $$ > '{pid}'
sleep 30 &
echo $! > '{child}'
sleep 30
",
            pid = pid_file.display(),
            child = child_file.display(),
        ),
    );
    let lock_path = write_lock(dir.path(), &sha256_file(&bin));
    let runtime = verify_ok(&lock_path, &bin).await;
    let data = dir.path().join("data");
    fs::create_dir(&data).unwrap();
    let token = SecretString::new(TOKEN);
    let redactor = redactor_with_token();
    let platform = platform_meta();
    set_wait_fail_hook({
        let pid_file = pid_file.clone();
        let child_file = child_file.clone();
        move || pid_file.exists() && child_file.exists()
    });
    let deadline = Duration::from_secs(1);
    let started = std::time::Instant::now();
    let err = compile_static_config(compile_req(
        &runtime,
        &lock_path,
        dir.path(),
        &data,
        &platform,
        &token,
        &redactor,
        deadline,
    ))
    .await
    .unwrap_err();
    let elapsed = started.elapsed();
    clear_io_fail_hooks();
    assert_eq!(err.code(), ErrorCode::RuntimeInvalid);
    assert!(
        elapsed < deadline + Duration::from_millis(200) + Duration::from_secs(2),
        "wait-fail cleanup must not wait for the 30s fixture: {elapsed:?}"
    );
    let pid: i32 = fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    wait_reaped(pid, Duration::from_secs(2)).expect("wait-fail Drop reaped group");
    if child_file.exists() {
        let child: i32 = fs::read_to_string(&child_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        wait_pid_gone(child, Duration::from_secs(2)).expect("descendant reaped");
    }
    let leftovers = leftover_names(&data);
    assert!(
        leftovers
            .iter()
            .all(|n| !n.to_string_lossy().contains("partial")),
        "partials must be removed: {leftovers:?}"
    );
}
