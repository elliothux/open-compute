use super::*;

#[test]
fn master_key_inspection_covers_env_file_utf8_and_mismatch_paths() {
    let (_tmp, root) = unique_root();
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    fs::create_dir(root.join("keys")).unwrap();
    fs::set_permissions(root.join("keys"), fs::Permissions::from_mode(0o700)).unwrap();
    let mut config = storage_config(&root);
    let generated = master_key::resolve(&config).unwrap();
    let encoded = fs::read_to_string(&config.master_key_file).unwrap();
    let env_name = "OPEN_COMPUTE_STORAGE_INSPECT_MASTER_KEY";
    config.master_key_env = Some(env_name.to_owned());
    master_key::set_test_env(env_name, &encoded);
    let both = master_key::inspect_existing(&config).unwrap();
    assert_eq!(both.fingerprint(), generated.fingerprint());

    master_key::set_test_env(
        env_name,
        "ocmk1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
    );
    assert_eq!(
        master_key::inspect_existing(&config).unwrap_err().code(),
        ErrorCode::MasterKeyMismatch
    );
    fs::remove_file(&config.master_key_file).unwrap();
    let env_only = master_key::inspect_existing(&config).unwrap();
    assert_eq!(env_only.bytes().expose().len(), 32);

    config.master_key_env = None;
    let mut invalid_utf8 = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&config.master_key_file)
        .unwrap();
    invalid_utf8.write_all(&[0xff, 0xfe]).unwrap();
    drop(invalid_utf8);
    assert_eq!(
        master_key::inspect_existing(&config).unwrap_err().code(),
        ErrorCode::MasterKeyMismatch
    );
    master_key::clear_test_env();

    let (_tmp2, root2) = unique_root();
    fs::create_dir(&root2).unwrap();
    fs::set_permissions(&root2, fs::Permissions::from_mode(0o700)).unwrap();
    let missing_parent = storage_config(&root2);
    assert_eq!(
        master_key::resolve(&missing_parent).unwrap_err().code(),
        ErrorCode::PathInvalid
    );
}
