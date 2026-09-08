use super::*;

#[tokio::test]
async fn cancel_sends_term_before_process_is_gone() {
    let dir = TempDir::new().unwrap();
    copy_formal_assets(dir.path());
    let pid_file = dir.path().join("pid");
    let marker = dir.path().join("term-marker");
    let bin = dir.path().join("workerd");
    write_exec(
        &bin,
        &format!(
            "#!/bin/sh
if [ \"$1\" = \"--version\" ]; then echo '{VERSION}'; exit 0; fi
trap 'echo term > \"{marker}\"; exit 0' TERM
echo $$ > '{pid}'
sleep 30
",
            pid = pid_file.display(),
            marker = marker.display(),
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
    loop {
        if marker.exists() {
            break;
        }
        if !pid_alive(pid) {
            panic!("process exited before the TERM marker was written");
        }
        if started.elapsed() > Duration::from_secs(2) {
            panic!("TERM marker was not written");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    wait_pid_gone(pid, Duration::from_secs(4)).expect("pid gone after TERM");
    wait_reaped(pid, Duration::from_secs(4)).expect("reaped after TERM");
}
