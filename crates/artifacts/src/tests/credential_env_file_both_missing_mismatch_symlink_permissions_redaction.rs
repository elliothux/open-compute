use super::*;

#[test]
fn credential_env_file_both_missing_mismatch_symlink_permissions_redaction() {
    let dir = TempDir::new().unwrap();
    let access = dir.path().join("access");
    let secret = dir.path().join("secret");
    write_mode(&access, "AKIAEXAMPLEKEYID01\n", 0o600);
    write_mode(&secret, "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY\n", 0o600);

    let mut cfg = s3_config("http://127.0.0.1:9");
    cfg.access_key_id_file = Some(access.clone());
    cfg.secret_access_key_file = Some(secret.clone());

    let creds = resolve_s3_credentials_with(&cfg, &env()).expect("both match");
    let debug = format!("{creds:?}");
    let display = creds.to_string();
    assert!(!debug.contains("AKIA"));
    assert!(!display.contains("AKIA"));
    assert!(!debug.contains("wJalr"));

    let mismatch = env().with("S3_ACCESS_KEY_ID", "OTHER");
    assert_eq!(
        resolve_s3_credentials_with(&cfg, &mismatch)
            .unwrap_err()
            .code(),
        ErrorCode::SecretRefInvalid
    );

    let empty = MapEnv::new();
    cfg.access_key_id_file = None;
    cfg.secret_access_key_file = None;
    assert_eq!(
        resolve_s3_credentials_with(&cfg, &empty)
            .unwrap_err()
            .code(),
        ErrorCode::SecretRefInvalid
    );

    write_mode(&access, "", 0o600);
    cfg.access_key_id_file = Some(access.clone());
    cfg.access_key_id_env = None;
    cfg.secret_access_key_file = Some(secret.clone());
    cfg.secret_access_key_env = None;
    assert_eq!(
        resolve_s3_credentials_with(&cfg, &empty)
            .unwrap_err()
            .code(),
        ErrorCode::SecretRefInvalid
    );

    write_mode(&access, "AKIAEXAMPLEKEYID01", 0o644);
    cfg.access_key_id_file = Some(access.clone());
    assert_eq!(
        resolve_s3_credentials_with(&cfg, &empty)
            .unwrap_err()
            .code(),
        ErrorCode::SecretRefInvalid
    );

    let target = dir.path().join("secret-target");
    write_mode(&target, "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY", 0o600);
    let before = fs::read(&target).unwrap();
    let link = dir.path().join("access-link");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    cfg.access_key_id_file = Some(link);
    write_mode(&access, "AKIAEXAMPLEKEYID01", 0o600);
    assert_eq!(
        resolve_s3_credentials_with(&cfg, &empty)
            .unwrap_err()
            .code(),
        ErrorCode::SecretRefInvalid
    );
    assert_eq!(fs::read(&target).unwrap(), before);

    let process_err = resolve_s3_credentials(&s3_config("http://127.0.0.1:9"));
    match process_err {
        Ok(c) => {
            assert!(!format!("{c:?}").contains("AKIA"));
            assert!(!c.to_string().contains("AKIA"));
        }
        Err(err) => assert_eq!(err.code(), ErrorCode::SecretRefInvalid),
    }
}
