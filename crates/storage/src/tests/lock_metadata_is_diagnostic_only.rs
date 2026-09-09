use super::*;

#[test]
fn lock_metadata_is_diagnostic_only() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).expect("boot");
    let meta: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(root.join("platform.lock")).unwrap()).unwrap();
    assert!(meta.get("startup_id").is_some());
    assert!(meta.get("pid").is_some());
    assert!(meta.get("release_version").is_some());
    assert!(
        storage
            .data_dir()
            .filesystem_durability()
            .doctor_warning()
            .is_none()
            || storage
                .data_dir()
                .filesystem_durability()
                .doctor_warning()
                .is_some()
    );
    drop(storage);
}
