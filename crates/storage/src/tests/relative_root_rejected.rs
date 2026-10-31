use super::*;

#[test]
fn relative_root_rejected() {
    let tmp = tempfile::tempdir().expect("tmp");
    let mut relative = storage_config(tmp.path());
    relative.path = PathBuf::from("relative-data");
    let err = DataDir::acquire(&relative).expect_err("relative");
    assert_eq!(err.code(), ErrorCode::PathInvalid);
}
