use super::*;

#[tokio::test]
async fn concurrent_compiles_do_not_clobber_workspaces() {
    let dir = TempDir::new().unwrap();
    copy_formal_assets(dir.path());
    let counter = dir.path().join("count");
    let args = dir.path().join("args");
    let bin = dir.path().join("workerd");
    write_exec(
        &bin,
        &compile_script(&counter, &args, "COMPILED-BYTES", "sleep 0.2"),
    );
    let lock_path = write_lock(dir.path(), &sha256_file(&bin));
    let runtime = verify_ok(&lock_path, &bin).await;
    let data = dir.path().join("data");
    fs::create_dir(&data).unwrap();
    let token = SecretString::new(TOKEN);
    let redactor = redactor_with_token();
    let platform = platform_meta();
    let a = compile_static_config(compile_req(
        &runtime,
        &lock_path,
        dir.path(),
        &data,
        &platform,
        &token,
        &redactor,
        Duration::from_secs(5),
    ));
    let b = compile_static_config(compile_req(
        &runtime,
        &lock_path,
        dir.path(),
        &data,
        &platform,
        &token,
        &redactor,
        Duration::from_secs(5),
    ));
    let (ra, rb) = tokio::join!(a, b);
    let ca = ra.expect("first concurrent compile");
    let cb = rb.expect("second concurrent compile");
    assert_eq!(ca.digest(), cb.digest());
    ca.open().expect("winner must revalidate");
    cb.open().expect("winner must revalidate");
}
