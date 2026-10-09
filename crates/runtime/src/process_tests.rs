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
            lease: None,
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
            lease: None,
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
async fn bounded_host_process_lease_is_cleared_after_reap_and_invalid_lease_fails() {
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("bounded-host.sh");
    let script = b"#!/bin/sh\n/bin/sleep 0.5\n";
    fs::write(&executable, script).unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let digest = hex::encode(sha2::Sha256::digest(script));
    let lease_path = directory.path().join("task.lease");
    let image = VerifiedLaunchImage::from_verified_file(File::open(&executable).unwrap());
    let working_directory = directory.path().to_owned();
    let digest_for_task = digest.clone();
    let lease_for_task = lease_path.clone();
    let task = tokio::spawn(async move {
        run_host_process(
            &image,
            HostProcessSpec {
                args: Vec::new(),
                environment: Vec::new(),
                working_directory,
                stdin: Vec::new(),
                deadline: Duration::from_secs(2),
                max_stdout: 1024,
                max_stderr: 1024,
                redactor: Redactor::new(),
                lease: Some(HostProcessLease {
                    path: lease_for_task,
                    binary_sha256: digest_for_task,
                }),
            },
        )
        .await
    });
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while !lease_path.exists() && !task.is_finished() {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(lease_path.exists());
    assert!(task.await.unwrap().unwrap().status.unwrap().success());
    assert!(!lease_path.exists());

    let image = VerifiedLaunchImage::from_verified_file(File::open(&executable).unwrap());
    assert!(
        run_host_process(
            &image,
            HostProcessSpec {
                args: Vec::new(),
                environment: Vec::new(),
                working_directory: directory.path().to_owned(),
                stdin: Vec::new(),
                deadline: Duration::from_secs(2),
                max_stdout: 1024,
                max_stderr: 1024,
                redactor: Redactor::new(),
                lease: Some(HostProcessLease {
                    path: directory.path().join("missing/task.lease"),
                    binary_sha256: digest.clone(),
                }),
            },
        )
        .await
        .is_err()
    );

    crate::lease::set_lease_write_fail(true);
    let image = VerifiedLaunchImage::from_verified_file(File::open(&executable).unwrap());
    let write_failure = run_host_process(
        &image,
        HostProcessSpec {
            args: Vec::new(),
            environment: Vec::new(),
            working_directory: directory.path().to_owned(),
            stdin: Vec::new(),
            deadline: Duration::from_secs(2),
            max_stdout: 1024,
            max_stderr: 1024,
            redactor: Redactor::new(),
            lease: Some(HostProcessLease {
                path: lease_path.clone(),
                binary_sha256: digest,
            }),
        },
    )
    .await;
    crate::lease::set_lease_write_fail(false);
    assert!(write_failure.is_err());
    assert!(!lease_path.exists());
}

#[tokio::test]
async fn bounded_host_process_crash_helper() {
    let Some(workspace) = std::env::var_os("OC_TEST_BOUNDED_CHILD_WORKSPACE") else {
        return;
    };
    let workspace = PathBuf::from(workspace);
    let executable = File::open("/bin/sleep").unwrap();
    let digest = hex::encode(sha2::Sha256::digest(fs::read("/bin/sleep").unwrap()));
    run_host_process(
        &VerifiedLaunchImage::from_verified_file(executable),
        HostProcessSpec {
            args: vec![OsString::from("30")],
            environment: Vec::new(),
            working_directory: workspace.clone(),
            stdin: Vec::new(),
            deadline: Duration::from_secs(35),
            max_stdout: 0,
            max_stderr: 0,
            redactor: Redactor::new(),
            lease: Some(HostProcessLease {
                path: workspace.join("child.lease"),
                binary_sha256: digest,
            }),
        },
    )
    .await
    .unwrap();
}

#[test]
fn bounded_host_process_recovers_after_real_owner_sigkill() {
    let workspace = tempfile::tempdir().unwrap();
    let lease = workspace.path().join("child.lease");
    let digest = hex::encode(sha2::Sha256::digest(fs::read("/bin/sleep").unwrap()));
    let executable = std::env::current_exe().unwrap();
    let mut owner = std::process::Command::new(executable)
        .args([
            "--exact",
            "process::coverage_tests::bounded_host_process_crash_helper",
            "--nocapture",
        ])
        .env("OC_TEST_BOUNDED_CHILD_WORKSPACE", workspace.path())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !lease.exists() {
        assert!(
            owner.try_wait().unwrap().is_none(),
            "owner exited before lease"
        );
        assert!(
            std::time::Instant::now() < deadline,
            "lease was not written"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    owner.kill().unwrap();
    owner.wait().unwrap();
    assert!(
        crate::lease::recover_orphan_for_test(&lease, &digest)
            .unwrap()
            .is_some()
    );
    assert!(!lease.exists());
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
            private_fds: vec![(child.into(), 0)],
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
            private_fds: Vec::new(),
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
            private_fds: Vec::new(),
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
    assert!(image.program.starts_with(data.path().join("staging")));
    drop(image);
    assert!(!journal.exists());
    clear_staging_journal(&lease_path).unwrap();
}

#[cfg(target_os = "macos")]
#[test]
fn bounded_host_child_stages_executable_inside_its_owned_task_root() {
    let task = tempfile::TempDir::new().unwrap();
    let echo = File::open("/bin/echo").unwrap();
    let image = exec_image_for_task(&echo, task.path()).unwrap();
    let staging_root = task.path().join("tmp/staging");
    assert!(image.program.starts_with(&staging_root));
    let staged = image.program.parent().unwrap().to_owned();
    drop(image);
    assert!(!staged.exists());
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

#[cfg(test)]
#[test]
fn fallback_skips_signal_after_owner_reaps() {
    let mut cmd = std::process::Command::new("/bin/sleep");
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    let child = cmd
        .arg("30")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn");
    let pid = child.id() as i32;
    let mut owned = OwnedChild::new(child, pid);
    owned.fail_safe_kill();
    owned.disarm();
    drop(owned);
    wait_reaped(pid, Duration::from_secs(2)).expect("owner reaped without a fallback signal");
}

#[cfg(all(test, target_os = "macos"))]
fn test_staging_path(lease_path: &Path) -> PathBuf {
    let root = lease_path.parent().unwrap().join("staging");
    crate::fsutil::create_dir_secure(&root).unwrap();
    root.join(format!("oc-exec-{}", uuid::Uuid::now_v7()))
}

#[cfg(all(test, target_os = "macos"))]
#[test]
fn staging_journal_recovers_interrupted_copy_without_child_lease() {
    let data = tempfile::TempDir::new().expect("temporary runtime data");
    let lease_path = data.path().join("child.lease");
    let digest = "ab".repeat(32);
    let staging = test_staging_path(&lease_path);
    fs::create_dir(&staging).expect("create interrupted staging directory");
    fs::write(staging.join("workerd"), b"partial verified executable copy")
        .expect("write interrupted copy");
    let journal = write_staging_journal(&lease_path, &staging, &digest).expect("write journal");

    recover_unleased_staging(&lease_path, &digest).expect("recover interrupted staging");

    assert!(!staging.exists(), "interrupted staging directory leaked");
    assert!(!journal.exists(), "staging journal leaked");
}

#[cfg(all(test, target_os = "macos"))]
#[test]
fn staging_journal_recovers_crash_before_directory_creation() {
    let data = tempfile::TempDir::new().expect("temporary runtime data");
    let lease_path = data.path().join("child.lease");
    let digest = "ab".repeat(32);
    let staging = test_staging_path(&lease_path);
    assert!(!staging.exists());
    let journal = write_staging_journal(&lease_path, &staging, &digest).expect("write journal");

    recover_unleased_staging(&lease_path, &digest).expect("recover empty staging journal");

    assert!(!staging.exists());
    assert!(!journal.exists(), "empty staging journal leaked");
}

#[cfg(all(test, target_os = "macos"))]
#[test]
fn complete_staging_without_child_lease_is_recovered() {
    use sha2::Digest as _;

    let data = tempfile::TempDir::new().expect("temporary runtime data");
    let lease_path = data.path().join("child.lease");
    let staging = test_staging_path(&lease_path);
    fs::create_dir(&staging).expect("create complete staging directory");
    let executable = staging.join("workerd");
    let bytes = b"complete verified executable copy";
    fs::write(&executable, bytes).expect("write complete copy");
    let digest = hex::encode(sha2::Sha256::digest(bytes));
    let journal = write_staging_journal(&lease_path, &staging, &digest).expect("write journal");

    recover_unleased_staging(&lease_path, &digest).expect("recover complete staging");

    assert!(!staging.exists(), "complete unleased staging leaked");
    assert!(
        !journal.exists(),
        "complete unleased staging journal leaked"
    );
}

#[cfg(test)]
#[test]
fn process_helpers_fail_closed_on_absent_and_invalid_processes() {
    let data = tempfile::TempDir::new().expect("temporary data");
    let lease = data.path().join("child.lease");
    assert_eq!(
        staging_journal_path(&lease),
        data.path().join("child.staging")
    );
    clear_staging_journal(&lease).expect("missing journal is already clear");
    cleanup_staging_dir_strict(&data.path().join("missing")).expect("missing staging is clear");

    assert!(!process_group_live(0));
    assert_reaped(None).expect("no pid is reaped");
    assert_reaped(Some(0)).expect("invalid pid is treated as absent");
    wait_pid_gone(0, Duration::ZERO).expect("invalid pid is absent");
    terminate_group_term(None);
    terminate_group_term(Some(0));
    terminate_group_kill(None);
    terminate_group_kill(Some(0));

    let pipe = PipeState::new();
    assert!(pipe.take_error().is_none());
    assert!(pipe.take_bytes().is_empty());
    join_readers(None, None, None, std::time::Instant::now()).expect("no readers");
    let panicking = std::thread::spawn(|| panic!("reader failure"));
    assert!(join_readers(Some(panicking), None, None, std::time::Instant::now()).is_err());

    let mut owned = OwnedChild {
        child: None,
        pid: 0,
        disarmed: false,
    };
    assert!(owned.take_stdout().is_none());
    assert!(owned.take_stderr().is_none());
    assert!(owned.try_wait().expect("empty owner").is_none());
    assert!(owned.wait().expect("empty owner").is_none());
    let mut status = Some(
        std::process::Command::new("/usr/bin/true")
            .status()
            .unwrap(),
    );
    let mut error = None;
    reap_after_kill(&mut owned, &mut status, &mut error);
    assert!(error.is_none());
    owned.disarm();

    let mut guard = ProcessGuard {
        cancel: None,
        owner: None,
    };
    guard.disarm();
}

#[cfg(all(test, target_os = "macos"))]
#[test]
fn staging_journal_validation_matrix_is_fail_closed() {
    let data = tempfile::TempDir::new().expect("temporary data");
    let lease = data.path().join("child.lease");
    let journal = staging_journal_path(&lease);
    let digest = "ab".repeat(32);

    assert!(!private_staging_dir(&lease, Path::new("relative")));
    assert!(!private_staging_dir(&lease, Path::new("/")));
    assert!(!private_staging_dir(
        &lease,
        &std::env::temp_dir().join("wrong-prefix")
    ));
    assert!(!private_staging_dir(
        &lease,
        &std::env::temp_dir().join("oc-exec-not-a-uuid")
    ));
    assert_eq!(
        executable_user(&data.path().join("missing")).expect("missing executable"),
        None
    );

    fs::write(&journal, b"not json").unwrap();
    assert!(recover_unleased_staging(&lease, &digest).is_err());
    fs::write(&journal, vec![b'x'; 4097]).unwrap();
    assert!(recover_unleased_staging(&lease, &digest).is_err());

    for body in [
        serde_json::json!({
            "schemaVersion": 2,
            "directory": std::env::temp_dir().join(format!("oc-exec-{}", uuid::Uuid::now_v7())),
            "binarySha256": digest,
        }),
        serde_json::json!({
            "schemaVersion": 1,
            "directory": std::env::temp_dir().join(format!("oc-exec-{}", uuid::Uuid::now_v7())),
            "binarySha256": "cd".repeat(32),
        }),
        serde_json::json!({
            "schemaVersion": 1,
            "directory": data.path().join("not-private"),
            "binarySha256": digest,
        }),
    ] {
        fs::write(&journal, serde_json::to_vec(&body).unwrap()).unwrap();
        assert!(recover_unleased_staging(&lease, &digest).is_err());
    }
    fs::remove_file(&journal).unwrap();

    let staging = std::env::temp_dir().join(format!("oc-exec-{}", uuid::Uuid::now_v7()));
    fs::create_dir(&staging).unwrap();
    fs::create_dir(staging.join("unexpected")).unwrap();
    assert!(cleanup_staging_dir_strict(&staging).is_err());
    fs::remove_dir(staging.join("unexpected")).unwrap();
    fs::remove_dir(&staging).unwrap();
}
