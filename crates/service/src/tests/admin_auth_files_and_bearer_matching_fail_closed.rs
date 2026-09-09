use super::*;

#[test]
fn admin_auth_files_and_bearer_matching_fail_closed() {
    let dir = TempDir::new().unwrap();
    let valid = dir.path().join("admin-token");
    write_mode(&valid, "secret-value\r\n", 0o600);
    let secret = resolve_admin_auth(&admin_reference(Some(&valid))).unwrap();
    assert_eq!(secret.expose(), "secret-value");
    assert!(bearer_matches(Some("Bearer secret-value"), &secret));
    assert!(!bearer_matches(Some("Bearer Secret-value"), &secret));
    assert!(!bearer_matches(Some("secret-value"), &secret));
    assert!(!bearer_matches(None, &secret));

    assert_eq!(
        resolve_admin_auth(&admin_reference(Some(Path::new("relative"))))
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );
    let loose = dir.path().join("loose");
    write_mode(&loose, "secret", 0o644);
    assert_eq!(
        resolve_admin_auth(&admin_reference(Some(&loose)))
            .unwrap_err()
            .code(),
        ErrorCode::SecretRefInvalid
    );
    for (name, bytes) in [
        ("empty", Vec::new()),
        ("newline-only", b"\r\n".to_vec()),
        ("large", vec![b'x'; 257]),
        ("invalid-utf8", vec![0xff]),
    ] {
        let path = dir.path().join(name);
        fs::write(&path, bytes).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            resolve_admin_auth(&admin_reference(Some(&path)))
                .unwrap_err()
                .code(),
            ErrorCode::SecretRefInvalid
        );
    }
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(&valid, &link).unwrap();
    assert_eq!(
        resolve_admin_auth(&admin_reference(Some(&link)))
            .unwrap_err()
            .code(),
        ErrorCode::SecretRefInvalid
    );
    assert_eq!(
        resolve_admin_auth(&admin_reference(Some(dir.path())))
            .unwrap_err()
            .code(),
        ErrorCode::SecretRefInvalid
    );
    assert_eq!(
        resolve_admin_auth(&admin_reference(None))
            .unwrap_err()
            .code(),
        ErrorCode::SecretRefInvalid
    );

    let missing = format!("OPEN_COMPUTE_TEST_MISSING_ADMIN_{}", std::process::id());
    let fallback = resolve_admin_auth(&SecretReference {
        env: Some(missing.clone()),
        file: Some(valid),
    })
    .unwrap();
    assert_eq!(fallback.expose(), "secret-value");
    assert_eq!(
        resolve_admin_auth(&SecretReference {
            env: Some(missing),
            file: None,
        })
        .unwrap_err()
        .code(),
        ErrorCode::SecretRefInvalid
    );
}
