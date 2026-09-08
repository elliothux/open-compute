use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn descendant_holding_pipes_returns_within_deadline() {
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
exit 0
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
    let deadline = Duration::from_secs(2);
    let started = std::time::Instant::now();
    let result = compile_static_config(compile_req(
        &runtime,
        &lock_path,
        dir.path(),
        &data,
        &platform,
        &token,
        &redactor,
        deadline,
    ))
    .await;
    assert!(started.elapsed() < deadline + Duration::from_millis(800));
    if pid_file.exists() {
        let pid: i32 = fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        wait_reaped(pid, Duration::from_secs(2)).expect("pgid gone");
    }
    if child_file.exists() {
        let child: i32 = fs::read_to_string(&child_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        wait_pid_gone(child, Duration::from_secs(2)).expect("descendant gone");
    }
    if result.is_err() {
        let leftovers = leftover_names(&data);
        assert!(
            leftovers.iter().all(|n| {
                let s = n.to_string_lossy();
                !s.contains("partial") && !s.contains("compile")
            }),
            "partials must be removed: {leftovers:?}"
        );
    }
}
