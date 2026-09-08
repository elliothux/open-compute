use super::*;

#[tokio::test]
async fn reader_panic_is_typed_and_cleans_up() {
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
    set_reader_panic(true);
    let deadline = Duration::from_secs(5);
    let started = std::time::Instant::now();
    let err = compile_static_config(compile_req(
        &runtime,
        &lock_path,
        dir.path(),
        &data,
        &platform,
        &token,
        &redactor,
        deadline,
    ))
    .await
    .unwrap_err();
    let elapsed = started.elapsed();
    clear_io_fail_hooks();
    assert_eq!(err.code(), ErrorCode::RuntimeInvalid);
    assert!(
        elapsed < Duration::from_secs(2),
        "reader panic must not wait out the command deadline: {elapsed:?}"
    );
    let leftovers = leftover_names(&data);
    assert!(
        leftovers
            .iter()
            .all(|n| !n.to_string_lossy().contains("partial")),
        "partials must be removed: {leftovers:?}"
    );
}
