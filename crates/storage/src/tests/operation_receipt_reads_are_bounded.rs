use super::*;

#[test]
fn operation_receipt_reads_are_bounded() {
    let (_tmp, root) = unique_root();
    let config = storage_config(&root);
    let storage = PlatformStorage::bootstrap(&config, &SystemClock).unwrap();
    storage
        .data_dir()
        .write_operation_receipt("last-restore.json", b"bounded")
        .unwrap();
    assert_eq!(
        storage
            .data_dir()
            .read_operation_receipt("last-restore.json", 7)
            .unwrap(),
        b"bounded"
    );
    assert_eq!(
        storage
            .data_dir()
            .read_operation_receipt("../control.sqlite", 64)
            .unwrap_err()
            .code(),
        ErrorCode::PathInvalid
    );

    let receipt = root.join("operations/last-restore.json");
    fs::write(&receipt, vec![0_u8; 65]).unwrap();
    assert_eq!(
        storage
            .data_dir()
            .read_operation_receipt("last-restore.json", 64)
            .unwrap_err()
            .code(),
        ErrorCode::LimitInvalid
    );
}
