use super::*;

#[tokio::test]
async fn corrupt_sidecar_fails_closed() {
    let dir = TempDir::new().unwrap();
    copy_formal_assets(dir.path());
    let counter = dir.path().join("count");
    let args = dir.path().join("args");
    let bin = dir.path().join("workerd");
    write_exec(&bin, &compile_script(&counter, &args, "COMPILED-BYTES", ""));
    let lock_path = write_lock(dir.path(), &sha256_file(&bin));
    let runtime = verify_ok(&lock_path, &bin).await;
    let data = dir.path().join("data");
    fs::create_dir(&data).unwrap();
    let token = SecretString::new(TOKEN);
    let redactor = redactor_with_token();
    let platform = platform_meta();
    let compiled = compile_static_config(compile_req(
        &runtime,
        &lock_path,
        dir.path(),
        &data,
        &platform,
        &token,
        &redactor,
        Duration::from_secs(5),
    ))
    .await
    .unwrap();
    let sidecar = compiled.path().with_extension("bin.digest");
    fs::write(&sidecar, b"nope\n").unwrap();
    assert!(compiled.open().is_err());
    let rebuilt = compile_static_config(compile_req(
        &runtime,
        &lock_path,
        dir.path(),
        &data,
        &platform,
        &token,
        &redactor,
        Duration::from_secs(5),
    ))
    .await
    .expect("corrupt sidecar must rebuild, not skip");
    rebuilt.open().unwrap();
}
