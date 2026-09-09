use super::*;

#[tokio::test]
async fn symlink_ancestor_assets_rejected() {
    let dir = TempDir::new().unwrap();
    copy_formal_assets(dir.path());
    let real = dir.path().join("real-assets");
    fs::rename(dir.path().join("config.capnp"), real.join("config.capnp")).ok();
    let assets = dir.path().join("assets");
    fs::create_dir_all(dir.path().join("target")).unwrap();
    copy_formal_assets(&dir.path().join("target"));
    symlink(dir.path().join("target"), &assets).unwrap();
    let bin = dir.path().join("workerd");
    write_exec(&bin, &version_script(None));
    let lock_path = write_lock(dir.path(), &sha256_file(&bin));
    let runtime = verify_ok(&lock_path, &bin).await;
    let data = dir.path().join("data");
    fs::create_dir(&data).unwrap();
    let token = SecretString::new(TOKEN);
    let redactor = redactor_with_token();
    let platform = platform_meta();
    let err = compile_static_config(compile_req(
        &runtime,
        &lock_path,
        &assets,
        &data,
        &platform,
        &token,
        &redactor,
        Duration::from_secs(5),
    ))
    .await
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::PathInvalid);
}
