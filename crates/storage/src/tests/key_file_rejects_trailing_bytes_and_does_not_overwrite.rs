use super::*;

#[test]
fn key_file_rejects_trailing_bytes_and_does_not_overwrite() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let _owned = DataDir::acquire(&config).unwrap();
    drop(_owned);
    let first = master_key::resolve(&config).expect("generate");
    let original = fs::read(&config.master_key_file).unwrap();
    drop(first);

    let extra = {
        let mut v = original.clone();
        v.push(b'\n');
        v
    };
    fs::write(&config.master_key_file, &extra).unwrap();
    fs::set_permissions(&config.master_key_file, fs::Permissions::from_mode(0o600)).unwrap();
    let err = master_key::resolve(&config).expect_err("newline");
    assert_eq!(err.code(), ErrorCode::MasterKeyMismatch);
    assert_eq!(fs::read(&config.master_key_file).unwrap(), extra);

    fs::write(&config.master_key_file, &original).unwrap();
    let raced = master_key::resolve(&config).expect("existing wins");
    assert_eq!(fs::read(&config.master_key_file).unwrap(), original);
    drop(raced);

    let leftover = root.join("keys").join(".tmp-master-partial");
    fs::write(&leftover, b"partial").unwrap();
    fs::set_permissions(&leftover, fs::Permissions::from_mode(0o600)).unwrap();
    master_key::resolve(&config).expect("ignores leftover temp");
    assert_eq!(fs::read(&config.master_key_file).unwrap(), original);
    crate::fs::fsync_dir(&root.join("keys")).expect("dir fsync path");
}
