use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pgid_verify_failure_kills_spawned_child() {
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
wait
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
    set_pgid_verify_fail_hook({
        let pid_file = pid_file.clone();
        let child_file = child_file.clone();
        move |_| {
            let started = std::time::Instant::now();
            while started.elapsed() < Duration::from_secs(2) {
                if pid_file.exists() && child_file.exists() {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            true
        }
    });
    let err = compile_static_config(compile_req(
        &runtime,
        &lock_path,
        dir.path(),
        &data,
        &platform,
        &token,
        &redactor,
        Duration::from_secs(5),
    ))
    .await
    .unwrap_err();
    clear_pgid_verify_fail_hook();
    assert_eq!(err.code(), ErrorCode::RuntimeInvalid);
    let pid: i32 = fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    wait_reaped(pid, Duration::from_secs(5)).expect("pgid-verify RAII reaped group");
    if child_file.exists() {
        let child: i32 = fs::read_to_string(&child_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        wait_pid_gone(child, Duration::from_secs(5)).expect("descendant reaped");
    }
    let leftovers = leftover_names(&data);
    assert!(
        leftovers
            .iter()
            .all(|n| !n.to_string_lossy().contains("partial")),
        "partials must be removed: {leftovers:?}"
    );
}
