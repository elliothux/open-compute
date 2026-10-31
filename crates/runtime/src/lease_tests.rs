use super::*;
use std::os::unix::process::CommandExt;

#[test]
fn recovery_stops_recorded_group_and_clears_lease() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("child.lease");
    let mut child = std::process::Command::new("/bin/sleep")
        .arg("30")
        .process_group(0)
        .spawn()
        .unwrap();
    let pgid = child.id() as i32;
    write_lease(&path, pgid).unwrap();
    let recorded: ChildLease = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(recorded.boot_id, host_boot_id().unwrap());
    assert_eq!(
        assert_no_live_orphan(&path).unwrap_err().code(),
        ErrorCode::PlatformUnavailable
    );
    assert_eq!(recover_orphans(&path).unwrap(), Some(pgid));
    let _ = child.wait();
    assert!(!path.exists());
    assert_no_live_orphan(&path).unwrap();
    assert_eq!(recover_orphans(&path).unwrap(), None);
}

#[test]
fn recovery_kills_group_after_leader_exits() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("child.lease");
    let marker = directory.path().join("ready");
    let mut child = std::process::Command::new("/bin/sh")
        .args([
            "-c",
            "trap '' TERM; sleep 30 & echo ready > \"$1\"; exit 0",
            "sh",
        ])
        .arg(&marker)
        .process_group(0)
        .spawn()
        .unwrap();
    let pgid = child.id() as i32;
    child.wait().unwrap();
    assert!(marker.exists());
    write_lease(&path, pgid).unwrap();
    recover_orphans(&path).unwrap();
    assert!(!crate::process::process_group_live(pgid));
}

#[test]
fn malformed_and_special_groups_fail_without_signalling() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("child.lease");
    let current = host_boot_id().unwrap();
    for bytes in [
        b"invalid".to_vec(),
        serde_json::to_vec(&serde_json::json!({"pgid": 0, "boot_id": current})).unwrap(),
        serde_json::to_vec(&serde_json::json!({"pgid": 1, "boot_id": current})).unwrap(),
    ] {
        std::fs::write(&path, bytes).unwrap();
        assert!(recover_orphans(&path).is_err());
        assert!(path.exists());
    }
}

#[test]
fn recovery_discards_other_boot_and_legacy_leases_without_signalling() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("child.lease");
    let mut child = std::process::Command::new("/bin/sleep")
        .arg("30")
        .process_group(0)
        .spawn()
        .unwrap();
    let pgid = child.id() as i32;
    for record in [
        serde_json::json!({"pgid": pgid, "boot_id": "previous-boot"}),
        serde_json::json!({"pgid": pgid}),
    ] {
        std::fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        assert_no_live_orphan(&path).unwrap();
        assert!(path.exists(), "read-only inspection removed a lease");
        assert_eq!(recover_orphans(&path).unwrap(), None);
        assert!(!path.exists());
        assert!(
            child.try_wait().unwrap().is_none(),
            "stale lease signalled a live group"
        );
    }
    write_lease(&path, pgid).unwrap();
    assert_eq!(recover_orphans(&path).unwrap(), Some(pgid));
    let _ = child.wait();
    assert!(!crate::process::process_group_live(pgid));
}
