use super::*;

#[test]
fn asset_walk_bounds_zero_length_file_fanout() {
    let dir = TempDir::new().unwrap();
    fs::create_dir(dir.path().join("dist")).unwrap();
    fs::write(dir.path().join("config.capnp"), b"template").unwrap();
    for i in 0..3 {
        fs::write(dir.path().join("dist").join(format!("w{i}.js")), b"").unwrap();
    }
    set_test_max_asset_files(Some(2));
    let err = load_assets(dir.path()).unwrap_err();
    set_test_max_asset_files(None);
    assert_eq!(err.code(), ErrorCode::PathInvalid);

    let dir = TempDir::new().unwrap();
    fs::create_dir(dir.path().join("dist")).unwrap();
    fs::write(dir.path().join("config.capnp"), b"template").unwrap();
    fs::create_dir(dir.path().join("dist").join("a")).unwrap();
    fs::create_dir(dir.path().join("dist").join("b")).unwrap();
    set_test_max_asset_entries(Some(1));
    let err = load_assets(dir.path()).unwrap_err();
    set_test_max_asset_entries(None);
    assert_eq!(err.code(), ErrorCode::PathInvalid);

    let dir = TempDir::new().unwrap();
    fs::create_dir(dir.path().join("dist")).unwrap();
    fs::write(dir.path().join("config.capnp"), b"template").unwrap();
    for i in 0..9 {
        fs::write(
            dir.path().join("dist").join(format!("w{i}.js")),
            vec![b'x'; 1024 * 1024],
        )
        .unwrap();
    }
    assert_eq!(
        load_assets(dir.path()).unwrap_err().code(),
        ErrorCode::PathInvalid
    );
}
