use super::*;

#[test]
fn credential_sources_reject_malformed_files_and_expose_only_explicitly() {
    let dir = TempDir::new().unwrap();
    let access = dir.path().join("access");
    let secret = dir.path().join("secret");
    write_mode(&access, "AKIAEXAMPLEKEYID01\r\n", 0o600);
    write_mode(&secret, "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY\n", 0o600);

    let mut cfg = s3_config("http://127.0.0.1:9");
    cfg.access_key_id_env = None;
    cfg.secret_access_key_env = None;
    cfg.access_key_id_file = Some(access.clone());
    cfg.secret_access_key_file = Some(secret.clone());
    let creds = resolve_s3_credentials_with(&cfg, &MapEnv::new()).unwrap();
    assert_eq!(creds.access_key_id().expose(), "AKIAEXAMPLEKEYID01");
    assert_eq!(
        creds.secret_access_key().expose(),
        "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY"
    );
    assert_eq!(creds.to_string(), "S3Credentials([REDACTED])");

    cfg.access_key_id_file = Some(PathBuf::from("relative-access"));
    assert_eq!(
        resolve_s3_credentials_with(&cfg, &MapEnv::new())
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );

    cfg.access_key_id_file = Some(dir.path().to_path_buf());
    assert_eq!(
        resolve_s3_credentials_with(&cfg, &MapEnv::new())
            .unwrap_err()
            .code(),
        ErrorCode::SecretRefInvalid
    );

    cfg.access_key_id_file = Some(access.clone());
    let write_bytes = |bytes: &[u8]| {
        let mut file = OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&access)
            .unwrap();
        file.write_all(bytes).unwrap();
    };
    write_bytes(&vec![b'x'; 16 * 1024 + 1]);
    assert_eq!(
        resolve_s3_credentials_with(&cfg, &MapEnv::new())
            .unwrap_err()
            .code(),
        ErrorCode::SecretRefInvalid
    );
    write_bytes(&[0xff, 0xfe]);
    assert_eq!(
        resolve_s3_credentials_with(&cfg, &MapEnv::new())
            .unwrap_err()
            .code(),
        ErrorCode::SecretRefInvalid
    );
    write_bytes(b"valid-prefix\0hidden");
    assert_eq!(
        resolve_s3_credentials_with(&cfg, &MapEnv::new())
            .unwrap_err()
            .code(),
        ErrorCode::SecretRefInvalid
    );

    cfg.access_key_id_file = None;
    cfg.secret_access_key_file = None;
    cfg.access_key_id_env = Some("ACCESS".to_string());
    cfg.secret_access_key_env = Some("SECRET".to_string());
    let static_env = MapEnv::new()
        .with("ACCESS", "env-access")
        .with("SECRET", "env-secret");
    let creds = resolve_s3_credentials_with(&cfg, &static_env).unwrap();
    assert_eq!(creds.access_key_id().expose(), "env-access");
    assert_eq!(creds.secret_access_key().expose(), "env-secret");

    let mut empty_secret = cfg.clone();
    empty_secret.secret_access_key_env = Some("EMPTY_SECRET".into());
    assert_eq!(
        resolve_s3_credentials_with(
            &empty_secret,
            &MapEnv::new()
                .with("ACCESS", "env-access")
                .with("EMPTY_SECRET", ""),
        )
        .unwrap_err()
        .code(),
        ErrorCode::SecretRefInvalid
    );
    empty_secret.secret_access_key_env = None;
    empty_secret.secret_access_key_file = None;
    assert_eq!(
        resolve_s3_credentials_with(&empty_secret, &MapEnv::new().with("ACCESS", "env-access"))
            .unwrap_err()
            .code(),
        ErrorCode::SecretRefInvalid
    );
}
