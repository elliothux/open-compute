use super::*;

#[test]
fn readonly_root_rejects_mutation() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let owned = DataDir::acquire(&config).expect("create");
    drop(owned);
    let keys = root.join("keys");
    fs::set_permissions(&keys, fs::Permissions::from_mode(0o500)).unwrap();
    let dest = keys.join("x");
    let result = atomic_write(&dest, b"nope");
    restore_writable(&keys);
    restore_writable(&root);
    assert!(result.is_err());
}
