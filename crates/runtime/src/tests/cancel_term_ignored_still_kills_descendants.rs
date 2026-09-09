use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_term_ignored_still_kills_descendants() {
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
trap '' TERM
sleep 30 &
echo $! > '{child}'
while true; do sleep 0.05; done
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
    let mut fut = Box::pin(compile_static_config(compile_req(
        &runtime,
        &lock_path,
        dir.path(),
        &data,
        &platform,
        &token,
        &redactor,
        Duration::from_secs(30),
    )));
    let wait_pid = async {
        let pid = read_pid_file(&pid_file, Duration::from_secs(10)).await;
        let child = read_pid_file(&child_file, Duration::from_secs(10)).await;
        (pid, child)
    };
    let (pid, child) = tokio::select! {
        _ = fut.as_mut() => panic!("compile finished before cancellation"),
        ids = wait_pid => ids,
    };
    let started = std::time::Instant::now();
    drop(fut);
    wait_reaped(pid, Duration::from_secs(4)).expect("parent killed");
    wait_pid_gone(child, Duration::from_secs(4)).expect("descendant killed");
    assert!(started.elapsed() < Duration::from_secs(3));
    let leftovers: Vec<_> = fs::read_dir(&data)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name())
        .collect();
    assert!(
        leftovers.is_empty(),
        "work directories must be removed: {leftovers:?}"
    );
}
