use super::*;

#[test]
fn master_key_inspection_rejects_missing_malformed_and_ambiguous_sources() {
    let (_tmp, root) = unique_root();
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let keys = root.join("keys");
    fs::create_dir(&keys).unwrap();
    fs::set_permissions(&keys, fs::Permissions::from_mode(0o700)).unwrap();
    let mut config = storage_config(&root);
    assert_eq!(
        master_key::inspect_existing(&config).unwrap_err().code(),
        ErrorCode::MasterKeyMismatch
    );
    let key = master_key::resolve(&config).unwrap();
    assert_eq!(key.bytes().expose().len(), 32);
    assert_eq!(key.fingerprint().len(), 64);
    assert!(!format!("{key:?}").contains("ocmk1:"));
    master_key::inspect_existing(&config).unwrap();

    for value in [
        "bad-prefix",
        "ocmk1:not-valid!!!",
        "ocmk1:AA",
        "ocmk1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\n",
    ] {
        fs::write(&config.master_key_file, value).unwrap();
        fs::set_permissions(&config.master_key_file, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            master_key::inspect_existing(&config).unwrap_err().code(),
            ErrorCode::MasterKeyMismatch
        );
    }

    fs::remove_file(&config.master_key_file).unwrap();
    fs::create_dir(&config.master_key_file).unwrap();
    assert!(master_key::inspect_existing(&config).is_err());
    fs::remove_dir(&config.master_key_file).unwrap();
    let outside = root.join("outside");
    fs::write(
        &outside,
        "ocmk1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
    )
    .unwrap();
    fs::set_permissions(&outside, fs::Permissions::from_mode(0o600)).unwrap();
    std::os::unix::fs::symlink(&outside, &config.master_key_file).unwrap();
    assert!(master_key::inspect_existing(&config).is_err());
    fs::remove_file(&config.master_key_file).unwrap();

    let env_name = "OPEN_COMPUTE_STORAGE_EMPTY_MASTER_KEY";
    config.master_key_env = Some(env_name.to_owned());
    master_key::set_test_env(env_name, "");
    assert_eq!(
        master_key::inspect_existing(&config).unwrap_err().code(),
        ErrorCode::MasterKeyMismatch
    );
    master_key::clear_test_env();

    config.master_key_file = PathBuf::from("relative-key");
    config.master_key_env = None;
    assert_eq!(
        master_key::resolve(&config).unwrap_err().code(),
        ErrorCode::PathInvalid
    );
}
