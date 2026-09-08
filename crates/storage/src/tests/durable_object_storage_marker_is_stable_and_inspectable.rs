use super::*;

#[test]
fn durable_object_storage_marker_is_stable_and_inspectable() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let owned = DataDir::acquire(&config).unwrap();
    let path = owned
        .prepare_durable_object_storage("platform-id", "workerd-version")
        .unwrap();
    assert_eq!(
        inspect_durable_object_storage(&root, "platform-id", "workerd-version").unwrap(),
        path
    );
    assert_eq!(
        inspect_durable_object_storage(&root, "other-platform", "workerd-version")
            .unwrap_err()
            .code(),
        ErrorCode::DoStorageUnavailable
    );
    assert_eq!(
        owned
            .prepare_durable_object_storage("platform-id", "other-workerd")
            .unwrap_err()
            .code(),
        ErrorCode::DoStorageUnavailable
    );
}
