use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fallback_does_not_signal_after_owner_reaps() {
    let dir = TempDir::new().unwrap();
    copy_formal_assets(dir.path());
    let pid_file = dir.path().join("pid");
    let bin = dir.path().join("workerd");
    write_exec(
        &bin,
        &format!(
            "#!/bin/sh
if [ \"$1\" = \"--version\" ]; then echo '{VERSION}'; exit 0; fi
echo $$ > '{pid}'
sleep 30
",
            pid = pid_file.display(),
            VERSION = VERSION,
        ),
    );
    let lock_path = write_lock(dir.path(), &sha256_file(&bin));
    let runtime = verify_ok(&lock_path, &bin).await;
    let data = dir.path().join("data");
    fs::create_dir(&data).unwrap();
    let token = SecretString::new(TOKEN);
    let redactor = redactor_with_token();
    let platform = platform_meta();
    let owner_reaped = Arc::new(std::sync::atomic::AtomicBool::new(false));
    set_owner_reaped_hook({
        let owner_reaped = owner_reaped.clone();
        move || owner_reaped.store(true, std::sync::atomic::Ordering::SeqCst)
    });
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
    let pid = loop {
        tokio::select! {
            _ = fut.as_mut() => panic!("compile finished before cancellation"),
            _ = tokio::time::sleep(Duration::from_millis(20)) => {
                if let Ok(contents) = fs::read_to_string(&pid_file)
                    && let Ok(pid) = contents.trim().parse::<i32>()
                {
                    break pid;
                }
            }
        }
    };
    drop(fut);
    let started = std::time::Instant::now();
    while !owner_reaped.load(std::sync::atomic::Ordering::SeqCst)
        && started.elapsed() < Duration::from_secs(4)
    {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        owner_reaped.load(std::sync::atomic::Ordering::SeqCst),
        "owner must reap before the delayed fallback runs"
    );
    wait_reaped(pid, Duration::from_secs(4)).expect("owner reaped the original child");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "owner must finish TERM/KILL/reap without a delayed fallback thread"
    );
    clear_owner_reaped_hook();
}
