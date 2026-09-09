use super::*;

#[test]
fn master_key_modes_and_failures() {
    let (_tmp, root) = unique_root();
    let mut config = storage_config(&root);
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).expect("auto");
    let fp = storage.identity().master_key_id.clone();
    let key_bytes = fs::read_to_string(&config.master_key_file).unwrap();
    assert!(key_bytes.starts_with("ocmk1:"));
    drop(storage);

    let env_name = "PLATFORM_STORAGE_TEST_MASTER_KEY";
    master_key::set_test_env(env_name, key_bytes.trim());
    config.master_key_env = Some(env_name.to_string());
    let both = PlatformStorage::bootstrap(&config, &SystemClock).expect("both");
    assert_eq!(both.identity().master_key_id, fp);
    drop(both);

    master_key::set_test_env(
        env_name,
        "ocmk1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
    );
    let err = PlatformStorage::bootstrap(&config, &SystemClock).expect_err("mismatch both");
    assert_eq!(err.code(), ErrorCode::MasterKeyMismatch);

    let (_t2, root2) = unique_root();
    let mut env_only = storage_config(&root2);
    env_only.master_key_env = Some(env_name.to_string());
    fs::create_dir_all(root2.join("keys")).unwrap();
    fs::set_permissions(root2.join("keys"), fs::Permissions::from_mode(0o700)).unwrap();
    master_key::set_test_env(env_name, key_bytes.trim());
    let env_boot = PlatformStorage::bootstrap(&env_only, &SystemClock).expect("env only");
    assert!(
        !env_only.master_key_file.exists(),
        "env-only must not persist plaintext"
    );
    drop(env_boot);
    master_key::clear_test_env();

    let mut loose = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o644)
        .open(&config.master_key_file)
        .unwrap();
    loose.write_all(key_bytes.as_bytes()).unwrap();
    drop(loose);
    fs::set_permissions(&config.master_key_file, fs::Permissions::from_mode(0o644)).unwrap();
    config.master_key_env = None;
    let err = PlatformStorage::bootstrap(&config, &SystemClock).expect_err("loose");
    assert_eq!(err.code(), ErrorCode::PathInvalid);
    fs::set_permissions(&config.master_key_file, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&config.master_key_file, b"ocmk1:not-valid!!!").unwrap();
    let err = PlatformStorage::bootstrap(&config, &SystemClock).expect_err("corrupt");
    assert_eq!(err.code(), ErrorCode::MasterKeyMismatch);
}
