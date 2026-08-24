use super::*;
use std::os::unix::fs::PermissionsExt;

#[tokio::test]
async fn process_spawn_deadline_pgid_and_output_faults_are_typed_and_reaped() {
    let dir = tempfile::tempdir().unwrap();
    let non_executable = dir.path().join("not-executable");
    fs::write(&non_executable, b"#!/definitely/missing/interpreter\n").unwrap();
    fs::set_permissions(&non_executable, fs::Permissions::from_mode(0o600)).unwrap();
    let file = File::open(&non_executable).unwrap();
    assert_eq!(
        run_verified_fd(
            &file,
            &[],
            Duration::from_secs(1),
            1024,
            &Redactor::new(),
            None,
        )
        .await
        .unwrap_err()
        .code(),
        ErrorCode::RuntimeInvalid
    );

    let sleep = File::open("/bin/sleep").unwrap();
    assert_eq!(
        run_verified_fd(&sleep, &["30"], Duration::MAX, 1024, &Redactor::new(), None,)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::RuntimeInvalid
    );

    assert_eq!(
        verify_self_pgid(0).unwrap_err().code(),
        ErrorCode::RuntimeInvalid
    );
    assert_eq!(
        verify_self_pgid(i32::MAX).unwrap_err().code(),
        ErrorCode::RuntimeInvalid
    );
    assert_eq!(
        verify_self_pgid(std::process::id() as i32)
            .unwrap_err()
            .code(),
        ErrorCode::RuntimeInvalid
    );
    assert_eq!(
        wait_pid_gone(std::process::id() as i32, Duration::ZERO)
            .unwrap_err()
            .code(),
        ErrorCode::RuntimeInvalid
    );

    let output = dir.path().join("output");
    STDOUT_FLUSH_FAIL.store(true, Ordering::SeqCst);
    let echo = File::open("/bin/echo").unwrap();
    let error = run_verified_fd(
        &echo,
        &["hello"],
        Duration::from_secs(2),
        1024,
        &Redactor::new(),
        Some(File::create(&output).unwrap()),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::ConfigCompileFailed);

    STDOUT_SYNC_FAIL.store(true, Ordering::SeqCst);
    let echo = File::open("/bin/echo").unwrap();
    let error = run_verified_fd(
        &echo,
        &["hello"],
        Duration::from_secs(2),
        1024,
        &Redactor::new(),
        Some(File::create(&output).unwrap()),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::ConfigCompileFailed);

    STDERR_READ_FAIL.store(true, Ordering::SeqCst);
    let shell = File::open("/bin/sh").unwrap();
    let error = run_verified_fd(
        &shell,
        &["-c", "echo failure >&2"],
        Duration::from_secs(2),
        1024,
        &Redactor::new(),
        None,
    )
    .await
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::RuntimeInvalid);
    clear_io_fail_hooks();

    OWNER_SPAWN_FAIL.store(true, Ordering::SeqCst);
    let sleep = File::open("/bin/sleep").unwrap();
    assert_eq!(
        run_verified_fd(
            &sleep,
            &["30"],
            Duration::from_secs(2),
            1024,
            &Redactor::new(),
            None,
        )
        .await
        .unwrap_err()
        .code(),
        ErrorCode::RuntimeInvalid
    );

    set_owner_reaped_hook(|| panic!("owner completion hook panic"));
    let echo = File::open("/bin/echo").unwrap();
    assert_eq!(
        run_verified_fd(
            &echo,
            &["done"],
            Duration::from_secs(2),
            1024,
            &Redactor::new(),
            None,
        )
        .await
        .unwrap_err()
        .code(),
        ErrorCode::RuntimeInvalid
    );
    clear_owner_reaped_hook();
}

#[test]
fn owned_child_wait_failure_is_retained_while_the_child_is_still_reaped() {
    let mut command = std::process::Command::new("/bin/sleep");
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let child = command.arg("30").spawn().unwrap();
    let pid = child.id() as i32;
    let mut owned = OwnedChild::new(child, pid);
    WAIT_FAIL.store(true, Ordering::SeqCst);
    let mut status = None;
    let mut error = None;
    reap_after_kill(&mut owned, &mut status, &mut error);
    WAIT_FAIL.store(false, Ordering::SeqCst);
    assert_eq!(error.unwrap().code(), ErrorCode::RuntimeInvalid);
    owned.disarm();
    wait_reaped(pid, Duration::from_secs(2)).unwrap();

    let stderr_panics = std::thread::spawn(|| panic!("stderr reader panic"));
    assert!(join_readers(None, Some(stderr_panics), std::time::Instant::now()).is_err());
}

#[test]
fn owner_wait_hard_deadline_reaps_without_waiting_for_the_soft_deadline() {
    let mut command = std::process::Command::new("/bin/sleep");
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.arg("30").spawn().unwrap();
    let pid = child.id() as i32;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let now = std::time::Instant::now();
    let output = owner_wait(OwnerWait {
        owned: OwnedChild::new(child, pid),
        stdout,
        stderr,
        stdout_file: None,
        max_stdout: 1024,
        cancel: Arc::new(AtomicBool::new(false)),
        deadline_at: now + Duration::from_secs(30),
        hard_deadline: now,
    })
    .unwrap();
    assert!(output.timed_out);
    wait_reaped(pid, Duration::from_secs(2)).unwrap();
}
