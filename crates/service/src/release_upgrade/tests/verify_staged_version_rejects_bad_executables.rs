use super::*;

#[test]
fn verify_staged_version_rejects_bad_executables() {
    let temp = TempDir::new().unwrap();
    let bad = temp.path().join("bad");
    fs::write(&bad, b"not-an-executable").unwrap();
    let _ = fs::set_permissions(&bad, fs::Permissions::from_mode(0o644));
    assert!(verify_staged_version(&bad, "0.1.0").is_err());
    let failing = temp.path().join("failing");
    fs::write(&failing, b"#!/bin/sh\nexit 1\n").unwrap();
    fs::set_permissions(&failing, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        verify_staged_version(&failing, "0.1.0")
            .unwrap_err()
            .message()
            .contains("unsuccessfully")
    );
}
