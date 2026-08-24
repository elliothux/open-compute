use super::*;
use crate::process::{clear_signal_log, take_signal_log};
use sha2::Digest as _;
#[cfg(target_os = "macos")]
use std::ffi::OsString;
#[cfg(target_os = "macos")]
use std::os::unix::ffi::OsStringExt as _;
use std::os::unix::process::CommandExt;
use std::sync::atomic::{AtomicI32, Ordering};
use tempfile::TempDir;

static HOOK_PID: AtomicI32 = AtomicI32::new(0);
static HOOK_MODE: AtomicI32 = AtomicI32::new(0);

fn hook_identity(pid: i32) -> Option<String> {
    match HOOK_MODE.load(Ordering::SeqCst) {
        0 => None,
        1 => Some(format!("test-start:{pid}")),
        2 if pid == HOOK_PID.load(Ordering::SeqCst) => Some("shared-start".into()),
        2 => Some(format!("other-start:{pid}")),
        _ => None,
    }
}

fn spawn_sleeper() -> std::process::Child {
    Command::new("/bin/sleep")
        .arg("30")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .expect("sleep")
}

fn digest_of(pid: i32) -> String {
    (0..50)
        .find_map(|_| {
            let d = live_executable_digest(pid);
            if d.is_none() {
                std::thread::sleep(Duration::from_millis(10));
            }
            d
        })
        .expect("live exe digest")
}

fn capture_retry(pid: i32, digest: &str) -> ChildLease {
    (0..50)
        .find_map(|_| {
            let captured = capture_lease(pid, pid, digest);
            if captured.is_none() {
                std::thread::sleep(Duration::from_millis(10));
            }
            captured
        })
        .expect("identity")
}

fn with_hook<R>(mode: i32, f: impl FnOnce() -> R) -> R {
    set_start_key_hook(Some(hook_identity));
    HOOK_MODE.store(mode, Ordering::SeqCst);
    let out = f();
    HOOK_MODE.store(0, Ordering::SeqCst);
    set_start_key_hook(None);
    out
}

#[test]
fn recover_kills_only_fully_verified_leader() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("child.lease");
    with_hook(1, || {
        let mut child = spawn_sleeper();
        let pid = child.id() as i32;
        let digest = digest_of(pid);
        let lease = capture_retry(pid, &digest);
        write_lease(&path, &lease).unwrap();
        clear_signal_log();
        let killed = recover_orphans(&path, &digest).unwrap();
        assert_eq!(killed, Some(pid));
        let signals = take_signal_log();
        assert!(
            signals.iter().any(|(p, k)| *p == pid && *k == "KILL"),
            "must signal verified pgid {signals:?}"
        );
        assert!(!path.exists());
        let _ = child.wait();
    });
}

#[test]
fn same_cwd_different_start_key_is_never_signaled() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("child.lease");
    with_hook(2, || {
        let mut a = spawn_sleeper();
        let mut b = spawn_sleeper();
        let pa = a.id() as i32;
        let pb = b.id() as i32;
        let digest = digest_of(pb);
        HOOK_PID.store(pa, Ordering::SeqCst);
        let mut lease = capture_retry(pb, &digest);
        lease.start_key = "shared-start".into();
        write_lease(&path, &lease).unwrap();
        clear_signal_log();
        let err = recover_orphans(&path, &digest).unwrap_err();
        assert_eq!(err.code(), ErrorCode::RuntimeInvalid);
        let signals = take_signal_log();
        assert!(
            !signals.iter().any(|(p, _)| *p == pb),
            "different start must not be signaled {signals:?}"
        );
        assert!(
            test_kill_process(Pid::from_raw(pb).unwrap()).is_ok(),
            "victim must still be alive"
        );
        assert!(path.exists(), "a live mismatched lease must be retained");
        terminate_cleanup(pa, &mut a);
        terminate_cleanup(pb, &mut b);
    });
}

#[test]
fn matching_start_wrong_pgid_is_never_signaled() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("child.lease");
    with_hook(1, || {
        let mut child = spawn_sleeper();
        let pid = child.id() as i32;
        let digest = digest_of(pid);
        let mut lease = capture_retry(pid, &digest);
        lease.pgid = pid.wrapping_add(99999);
        write_lease(&path, &lease).unwrap();
        clear_signal_log();
        let err = recover_orphans(&path, &digest).unwrap_err();
        assert_eq!(err.code(), ErrorCode::RuntimeInvalid);
        assert!(take_signal_log().iter().all(|(p, _)| *p != pid));
        assert!(path.exists(), "a live mismatched lease must be retained");
        terminate_cleanup(pid, &mut child);
    });
}

#[test]
fn wrong_live_executable_digest_is_never_signaled() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("child.lease");
    let other = "ab".repeat(32);
    with_hook(1, || {
        let mut child = spawn_sleeper();
        let pid = child.id() as i32;
        let digest = digest_of(pid);
        let mut lease = capture_retry(pid, &digest);
        lease.binary_sha256 = other.clone();
        write_lease(&path, &lease).unwrap();
        clear_signal_log();
        let err = recover_orphans(&path, &other).unwrap_err();
        assert_eq!(err.code(), ErrorCode::RuntimeInvalid);
        assert!(take_signal_log().iter().all(|(p, _)| *p != pid));
        assert!(path.exists(), "a live mismatched lease must be retained");
        terminate_cleanup(pid, &mut child);
    });
}

#[test]
fn identity_query_failure_is_fail_closed() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("child.lease");
    let mut child = spawn_sleeper();
    let pid = child.id() as i32;
    let digest = digest_of(pid);
    with_hook(1, || {
        let lease = capture_retry(pid, &digest);
        write_lease(&path, &lease).unwrap();
    });
    with_hook(0, || {
        clear_signal_log();
        let err = recover_orphans(&path, &digest).unwrap_err();
        assert_eq!(err.code(), ErrorCode::RuntimeInvalid);
        assert!(take_signal_log().is_empty());
        assert!(test_kill_process(Pid::from_raw(pid).unwrap()).is_ok());
        assert!(path.exists(), "an unverifiable live lease must be retained");
    });
    terminate_cleanup(pid, &mut child);
}

#[test]
fn atomic_replacement_never_unlinks_before_publish() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("child.lease");
    let a = ChildLease {
        schema_version: 1,
        pid: 8,
        pgid: 8,
        start_key: "one".into(),
        binary_sha256: "aa".repeat(32),
        staged_executable: None,
    };
    let b = ChildLease {
        start_key: "two".into(),
        ..a.clone()
    };
    write_lease(&path, &a).unwrap();
    assert!(path.exists());
    write_lease(&path, &b).unwrap();
    assert!(path.exists());
    let loaded = load_lease(&path).unwrap().unwrap();
    assert_eq!(loaded.start_key, "two");
    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(".partial"))
        .collect();
    assert!(leftovers.is_empty());
}

#[test]
fn nofollow_fd_read_ignores_symlink_path() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("child.lease");
    let digest = "aa".repeat(32);
    let lease = ChildLease {
        schema_version: 1,
        pid: 3,
        pgid: 3,
        start_key: "k".into(),
        binary_sha256: digest,
        staged_executable: None,
    };
    write_lease(&path, &lease).unwrap();
    let loaded = load_lease(&path).unwrap().unwrap();
    assert_eq!(loaded.start_key, "k");
    std::fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink("/etc/passwd", &path).unwrap();
    let err = load_lease(&path).unwrap_err();
    assert_eq!(err.code(), ErrorCode::PathInvalid);
    assert!(path.is_symlink());
}

#[test]
fn malformed_and_oversized_leases_are_fail_closed() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("child.lease");
    std::fs::write(&path, b"not json").unwrap();
    let err = recover_orphans(&path, &"aa".repeat(32)).unwrap_err();
    assert_eq!(err.code(), ErrorCode::RuntimeInvalid);
    assert_eq!(std::fs::read(&path).unwrap(), b"not json");

    std::fs::write(&path, vec![b'x'; MAX_LEASE_BYTES as usize + 1]).unwrap();
    let err = recover_orphans(&path, &"aa".repeat(32)).unwrap_err();
    assert_eq!(err.code(), ErrorCode::RuntimeInvalid);
    assert_eq!(std::fs::metadata(&path).unwrap().len(), MAX_LEASE_BYTES + 1);
}

#[test]
fn unexpected_runtime_digest_is_fail_closed_without_signal() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("child.lease");
    with_hook(1, || {
        let mut child = spawn_sleeper();
        let pid = child.id() as i32;
        let digest = digest_of(pid);
        let lease = capture_retry(pid, &digest);
        write_lease(&path, &lease).unwrap();
        clear_signal_log();
        let err = recover_orphans(&path, &"ab".repeat(32)).unwrap_err();
        assert_eq!(err.code(), ErrorCode::RuntimeInvalid);
        assert!(take_signal_log().is_empty());
        assert!(path.exists());
        terminate_cleanup(pid, &mut child);
    });
}

#[test]
fn conclusively_dead_stale_lease_is_cleared() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("child.lease");
    let digest = "aa".repeat(32);
    let lease = ChildLease {
        schema_version: SCHEMA + 1,
        pid: i32::MAX,
        pgid: i32::MAX,
        start_key: "dead".into(),
        binary_sha256: "bb".repeat(32),
        staged_executable: None,
    };
    write_lease(&path, &lease).unwrap();
    assert_eq!(recover_orphans(&path, &digest).unwrap(), None);
    assert!(!path.exists());
}

#[test]
fn lease_absence_invalid_pid_and_unremovable_path_are_typed() {
    assert!(capture_lease(0, 0, &"aa".repeat(32)).is_none());
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("child.lease");
    std::fs::create_dir(&path).unwrap();
    assert_eq!(
        clear_lease(&path).unwrap_err().code(),
        ErrorCode::PathInvalid
    );
    std::fs::remove_dir(&path).unwrap();
    assert!(clear_lease(&path).is_ok());
}

#[cfg(target_os = "macos")]
#[test]
fn private_staging_path_requires_exact_temp_uuid_and_binary_shape() {
    assert!(!private_staging_path(Path::new("")));
    assert!(!private_staging_path(Path::new("/tmp/workerd")));
    assert!(!private_staging_path(Path::new(
        "/tmp/oc-exec-not-a-uuid/workerd"
    )));
    let uuid = uuid::Uuid::now_v7();
    assert!(!private_staging_path(
        &std::env::temp_dir().join(format!("oc-exec-{uuid}/not-workerd"))
    ));

    let nested_root = TempDir::new().unwrap();
    let nested = nested_root.path().join(format!("oc-exec-{uuid}"));
    std::fs::create_dir(&nested).unwrap();
    let nested_binary = nested.join("workerd");
    std::fs::write(&nested_binary, b"x").unwrap();
    assert!(!private_staging_path(&nested_binary));

    let exact_dir = std::env::temp_dir().join(format!("oc-exec-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir(&exact_dir).unwrap();
    let exact_binary = exact_dir.join("workerd");
    std::fs::write(&exact_binary, b"x").unwrap();
    assert!(private_staging_path(&exact_binary));
    std::fs::remove_file(exact_binary).unwrap();
    std::fs::remove_dir(exact_dir).unwrap();
}

#[test]
fn live_executable_digest_matches_independent_file_hash() {
    let mut child = spawn_sleeper();
    let pid = child.id() as i32;
    let expected = hex::encode(sha2::Sha256::digest(
        std::fs::read("/bin/sleep").expect("read known executable"),
    ));
    assert_eq!(digest_of(pid), expected);
    terminate_cleanup(pid, &mut child);
}

#[test]
fn lease_identity_and_group_helpers_fail_closed() {
    assert!(matches!(
        live_match(
            &ChildLease {
                schema_version: SCHEMA,
                pid: 0,
                pgid: 0,
                start_key: "invalid".into(),
                binary_sha256: "aa".repeat(32),
                staged_executable: None,
            },
            &"aa".repeat(32)
        ),
        LiveMatch::Gone
    ));
    assert!(matches!(
        live_match(
            &ChildLease {
                schema_version: SCHEMA,
                pid: i32::MAX,
                pgid: i32::MAX,
                start_key: "gone".into(),
                binary_sha256: "aa".repeat(32),
                staged_executable: None,
            },
            &"aa".repeat(32)
        ),
        LiveMatch::Gone
    ));
    assert_eq!(
        signal_verified_group(0).unwrap_err().code(),
        ErrorCode::RuntimeInvalid
    );
    assert!(signal_verified_group(i32::MAX).is_ok());
    assert_eq!(
        wait_leader_and_group(2, 3).unwrap_err().code(),
        ErrorCode::RuntimeInvalid
    );
    assert_eq!(
        wait_leader_and_group(0, 0).unwrap_err().code(),
        ErrorCode::RuntimeInvalid
    );

    #[cfg(target_os = "macos")]
    {
        assert!(macos_ps_lstart(i32::MAX).is_none());
        assert!(macos_txt_path(i32::MAX).is_none());
    }
}

#[cfg(target_os = "macos")]
#[test]
fn staging_cleanup_accepts_only_owned_verified_temp_files() {
    let missing_dir = std::env::temp_dir().join(format!("oc-exec-{}", uuid::Uuid::now_v7()));
    let missing = missing_dir.join("workerd");
    cleanup_staging(&missing, &"aa".repeat(32)).unwrap();

    let empty_dir = std::env::temp_dir().join(format!("oc-exec-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir(&empty_dir).unwrap();
    cleanup_staging(&empty_dir.join("workerd"), &"aa".repeat(32)).unwrap();
    assert!(!empty_dir.exists());

    let outside = TempDir::new().unwrap();
    let outside_file = outside.path().join("workerd");
    std::fs::write(&outside_file, b"outside").unwrap();
    assert_eq!(
        cleanup_staging(&outside_file, &"aa".repeat(32))
            .unwrap_err()
            .code(),
        ErrorCode::RuntimeInvalid
    );

    let wrong_dir = std::env::temp_dir().join(format!("oc-exec-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir(&wrong_dir).unwrap();
    let wrong = wrong_dir.join("workerd");
    std::fs::write(&wrong, b"wrong").unwrap();
    assert_eq!(
        cleanup_staging(&wrong, &"aa".repeat(32))
            .unwrap_err()
            .code(),
        ErrorCode::RuntimeInvalid
    );
    std::fs::remove_file(&wrong).unwrap();
    std::fs::remove_dir(&wrong_dir).unwrap();

    let good_dir = std::env::temp_dir().join(format!("oc-exec-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir(&good_dir).unwrap();
    let good = good_dir.join("workerd");
    std::fs::write(&good, b"verified").unwrap();
    let digest = hex::encode(sha2::Sha256::digest(b"verified"));
    cleanup_staging(&good, &digest).unwrap();
    assert!(!good_dir.exists());

    let non_utf8 = Path::new("/tmp").join(OsString::from_vec(vec![0xff]));
    assert!(!private_staging_path(&non_utf8));
    assert!(!private_staging_path(Path::new("/workerd")));
    assert_eq!(
        cleanup_staging(Path::new("/"), &digest).unwrap_err().code(),
        ErrorCode::RuntimeInvalid
    );
}

fn terminate_cleanup(pid: i32, child: &mut std::process::Child) {
    if let Some(raw) = Pid::from_raw(pid) {
        let _ = kill_process_group(raw, Signal::KILL);
    }
    let _ = child.wait();
}
