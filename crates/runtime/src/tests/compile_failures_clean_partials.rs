use super::*;

#[tokio::test]
async fn compile_failures_clean_partials() {
    let dir = TempDir::new().unwrap();
    copy_formal_assets(dir.path());
    let counter = dir.path().join("count");
    let args = dir.path().join("args");
    let bin = dir.path().join("workerd");
    write_exec(&bin, &compile_script(&counter, &args, "", "exit 9"));
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
        dir.path(),
        &data,
        &platform,
        &token,
        &redactor,
        Duration::from_secs(5),
    ))
    .await
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::ConfigCompileFailed);
    let leftovers: Vec<_> = fs::read_dir(&data)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name())
        .collect();
    assert!(
        leftovers
            .iter()
            .all(|n| !n.to_string_lossy().contains("partial")
                && !n.to_string_lossy().contains("compile")),
        "partials must be removed: {leftovers:?}"
    );

    let sleepy = dir.path().join("sleepy");
    write_exec(
        &sleepy,
        &format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo '{VERSION}'; exit 0; fi\nsleep 30\n"
        ),
    );
    let slock_path = dir.path().join("sleepy.lock.json");
    fs::write(&slock_path, lock_json(&sha256_file(&sleepy), "")).unwrap();
    let srt = verify_ok(&slock_path, &sleepy).await;
    let err = compile_static_config(compile_req(
        &srt,
        &slock_path,
        dir.path(),
        &data,
        &platform,
        &token,
        &redactor,
        Duration::from_millis(300),
    ))
    .await
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::ConfigCompileFailed);

    let big = dir.path().join("bigc");
    write_exec(
        &big,
        &format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo '{VERSION}'; exit 0; fi\ndd if=/dev/zero bs=1048576 count=18 2>/dev/null\n"
        ),
    );
    let block = dir.path().join("big.lock.json");
    fs::write(&block, lock_json(&sha256_file(&big), "")).unwrap();
    let brt = verify_ok(&block, &big).await;
    let err = compile_static_config(compile_req(
        &brt,
        &block,
        dir.path(),
        &data,
        &platform,
        &token,
        &redactor,
        Duration::from_secs(5),
    ))
    .await
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::ConfigCompileFailed);
}
