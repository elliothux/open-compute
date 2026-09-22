use super::*;
use crate::{PersistentHostProcess, PersistentHostProcessSpec};
use sha2::Digest as _;
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
    assert!(join_readers(None, Some(stderr_panics), None, std::time::Instant::now()).is_err());
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
        stdin: None,
        stdin_bytes: Vec::new(),
        stdout_file: None,
        max_stdout: 1024,
        max_stderr: 1024,
        cancel: Arc::new(AtomicBool::new(false)),
        deadline_at: now + Duration::from_secs(30),
        hard_deadline: now,
    })
    .unwrap();
    assert!(output.timed_out);
    wait_reaped(pid, Duration::from_secs(2)).unwrap();
}

#[tokio::test]
async fn host_process_uses_explicit_cwd_environment_and_bounded_stdio() {
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("host-process.sh");
    fs::write(
        &executable,
        b"#!/bin/sh\nread value\nprintf '%s|%s|%s' \"$PWD\" \"${HOME-unset}\" \"$value\"\nprintf 'diagnostic' >&2\n",
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let image = VerifiedLaunchImage::from_verified_file(File::open(&executable).unwrap());
    let output = run_host_process(
        &image,
        HostProcessSpec {
            args: Vec::new(),
            environment: vec![("ONLY".into(), "set".into())],
            working_directory: directory.path().to_owned(),
            stdin: b"payload\n".to_vec(),
            deadline: Duration::from_secs(2),
            max_stdout: 4096,
            max_stderr: 4,
            redactor: Redactor::new(),
        },
    )
    .await
    .unwrap();
    assert!(output.status.unwrap().success());
    assert_eq!(
        output.stdout,
        format!(
            "{}|unset|payload",
            directory.path().canonicalize().unwrap().display()
        )
        .as_bytes()
    );
    assert_eq!(output.stderr, b"diag");
    assert!(output.stderr_overflow);
    assert!(!output.stdin_error);
}

#[tokio::test]
async fn host_process_stops_when_stderr_exceeds_its_bound() {
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("stderr-overflow.sh");
    fs::write(
        &executable,
        b"#!/bin/sh\nprintf 'diagnostic' >&2\nsleep 30\n",
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let image = VerifiedLaunchImage::from_verified_file(File::open(&executable).unwrap());
    let started = std::time::Instant::now();
    let output = run_host_process(
        &image,
        HostProcessSpec {
            args: Vec::new(),
            environment: Vec::new(),
            working_directory: directory.path().to_owned(),
            stdin: Vec::new(),
            deadline: Duration::from_secs(5),
            max_stdout: 0,
            max_stderr: 4,
            redactor: Redactor::new(),
        },
    )
    .await
    .unwrap();
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(output.stderr, b"diag");
    assert!(output.stderr_overflow);
    assert!(!output.timed_out);
    wait_reaped(output.pid.unwrap(), Duration::from_secs(2)).unwrap();
}

#[tokio::test]
async fn persistent_host_process_maps_control_fd_and_reaps_on_shutdown() {
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("persistent-host.sh");
    fs::write(
        &executable,
        b"#!/bin/sh\nIFS= read -r value <&0\nprintf '%s' \"$value\" >&0\nwhile :; do sleep 30; done\n",
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let image = VerifiedLaunchImage::from_verified_file(File::open(&executable).unwrap());
    let lease = directory.path().join("provider.lease");
    let (mut parent, child) = std::os::unix::net::UnixStream::pair().unwrap();
    parent
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let process = PersistentHostProcess::spawn(
        &image,
        PersistentHostProcessSpec {
            args: Vec::new(),
            environment: Vec::new(),
            working_directory: directory.path().to_owned(),
            control_fd: Some(child.into()),
            lease_path: lease.clone(),
            binary_sha256: hex::encode(sha2::Sha256::digest(fs::read(&executable).unwrap())),
            redactor: Redactor::new(),
        },
    )
    .unwrap();
    let pid = process.pid();
    parent.write_all(b"ready\n").unwrap();
    let mut reply = [0; 5];
    parent.read_exact(&mut reply).unwrap();
    assert_eq!(&reply, b"ready");
    assert!(process.is_running());
    process
        .shutdown(Duration::from_millis(50), Duration::from_secs(1))
        .await;
    wait_reaped(pid, Duration::from_secs(2)).unwrap();
    assert!(!lease.exists());
}

#[tokio::test]
async fn persistent_host_process_without_control_fd_gets_null_stdin_and_reaps() {
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("persistent-no-control.sh");
    fs::write(
        &executable,
        b"#!/bin/sh\nif IFS= read -r value; then exit 7; fi\nprintf ready > ready\nwhile :; do sleep 30; done\n",
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let image = VerifiedLaunchImage::from_verified_file(File::open(&executable).unwrap());
    let lease = directory.path().join("caddy.lease");
    let process = PersistentHostProcess::spawn(
        &image,
        PersistentHostProcessSpec {
            args: Vec::new(),
            environment: Vec::new(),
            working_directory: directory.path().to_owned(),
            control_fd: None,
            lease_path: lease.clone(),
            binary_sha256: hex::encode(sha2::Sha256::digest(fs::read(&executable).unwrap())),
            redactor: Redactor::new(),
        },
    )
    .unwrap();
    let pid = process.pid();
    let ready = directory.path().join("ready");
    let observed = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if fs::read_to_string(&ready).is_ok_and(|value| value == "ready") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .is_ok();
    process
        .shutdown(Duration::from_millis(50), Duration::from_secs(1))
        .await;
    wait_reaped(pid, Duration::from_secs(2)).unwrap();
    assert!(observed, "child stdin was not closed");
    assert!(!lease.exists());
}

#[tokio::test]
async fn persistent_host_process_notifies_after_unprompted_exit() {
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("persistent-exit.sh");
    fs::write(&executable, b"#!/bin/sh\nexit 7\n").unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let image = VerifiedLaunchImage::from_verified_file(File::open(&executable).unwrap());
    let lease = directory.path().join("caddy.lease");
    let process = PersistentHostProcess::spawn(
        &image,
        PersistentHostProcessSpec {
            args: Vec::new(),
            environment: Vec::new(),
            working_directory: directory.path().to_owned(),
            control_fd: None,
            lease_path: lease.clone(),
            binary_sha256: hex::encode(sha2::Sha256::digest(fs::read(&executable).unwrap())),
            redactor: Redactor::new(),
        },
    )
    .unwrap();
    let pid = process.pid();
    tokio::time::timeout(Duration::from_secs(2), process.wait_exited())
        .await
        .unwrap();
    assert!(!process.is_running());
    let outcome = process
        .shutdown(Duration::from_millis(0), Duration::from_secs(1))
        .await;
    assert_eq!(outcome.exit_code, Some(7));
    assert_eq!(outcome.signal, None);
    assert!(!outcome.reader_failed);
    wait_reaped(pid, Duration::from_secs(2)).unwrap();
    assert!(!lease.exists());
}

#[cfg(target_os = "macos")]
#[test]
fn exec_image_materializes_verified_fd_and_drops_staging() {
    let echo = File::open("/bin/echo").unwrap();
    let image = exec_image(&echo).unwrap();
    assert!(image.program.exists());
    let staged = image.program.parent().map(PathBuf::from);
    drop(image);
    if let Some(dir) = staged {
        assert!(
            !dir.exists(),
            "staging directory leaked after ExecImage drop"
        );
    }
}

#[cfg(target_os = "macos")]
#[test]
fn exec_image_with_lease_writes_and_clears_staging_journal() {
    let data = tempfile::TempDir::new().unwrap();
    let lease_path = data.path().join("child.lease");
    let digest = "ab".repeat(32);
    let echo = File::open("/bin/echo").unwrap();
    let image = exec_image_with_lease(&echo, &lease_path, &digest).unwrap();
    let journal = staging_journal_path(&lease_path);
    // Journal is removed when ExecImage drops after successful materialize?
    // Looking at code: staging_journal is kept on ExecImage and removed on Drop.
    assert!(image.program.exists());
    drop(image);
    assert!(!journal.exists());
    clear_staging_journal(&lease_path).unwrap();
}

#[tokio::test]
async fn exited_unreaped_child_keeps_status_and_output_when_pgid_read_fails() {
    set_pgid_verify_fail_hook(|pid| {
        let raw = Pid::from_raw(pid).unwrap();
        // Wait for this child to exit without reaping it; kill(pid, 0) still succeeds.
        rustix::process::waitid(
            rustix::process::WaitId::Pid(raw),
            rustix::process::WaitIdOptions::EXITED | rustix::process::WaitIdOptions::NOWAIT,
        )
        .unwrap();
        assert!(test_kill_process(raw).is_ok());
        #[cfg(target_os = "macos")]
        assert_eq!(getpgid(Some(raw)), Err(rustix::io::Errno::SRCH));
        true
    });
    let dir = tempfile::tempdir().unwrap();
    let executable = dir.path().join("completed-child");
    fs::write(&executable, b"#!/bin/sh\nprintf 'completed\\n'\nexit 0\n").unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let file = File::open(&executable).unwrap();
    let result = run_verified_fd(
        &file,
        &[],
        Duration::from_secs(2),
        1024,
        &Redactor::new(),
        None,
    )
    .await;
    clear_pgid_verify_fail_hook();
    let output = result.unwrap();
    assert!(output.status.unwrap().success(), "{output:?}");
    assert_eq!(output.stdout, b"completed\n");
    assert!(output.stderr.is_empty());
    assert!(!output.timed_out);
    wait_reaped(output.pid.unwrap(), Duration::from_secs(2)).unwrap();
}

#[cfg(target_os = "macos")]
#[test]
fn staging_journal_helpers_cover_missing_and_clear() {
    let data = tempfile::TempDir::new().unwrap();
    let lease = data.path().join("child.lease");
    fs::write(&lease, b"lease").unwrap();
    clear_staging_journal(&lease).unwrap();
    let journal = staging_journal_path(&lease);
    assert!(!journal.exists());
    // Nested missing parent should fail closed when writing a journal.
    let missing_parent = data.path().join("missing-dir").join("child.lease");
    let err = write_staging_journal(&missing_parent, data.path(), &"ab".repeat(32));
    assert!(err.is_err());
}
