use super::*;

#[tokio::test]
async fn cache_reuse_and_corrupt_rebuild() {
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
    let mut redactor = redactor_with_token();
    redactor.register_secret_string(&token);
    let platform = platform_meta();

    let first_request = compile_req(
        &runtime,
        &lock_path,
        dir.path(),
        &data,
        &platform,
        &redactor,
        Duration::from_secs(5),
    );
    let request_debug = format!("{first_request:?}");
    assert!(request_debug.contains("CompileRequest"));
    assert!(!request_debug.contains(TOKEN));
    let first = compile_static_config(first_request).await.expect("compile");
    assert_eq!(first.role(), crate::ConfigRole::Runtime);
    let n1 = fs::read(&counter).unwrap().len();
    assert!(n1 >= 1);
    let debug = format!("{first:?}");
    assert!(!debug.contains(TOKEN));
    assert!(!debug.contains(first.path().to_string_lossy().as_ref()));
    let args_text = fs::read_to_string(&args).unwrap();
    assert!(!args_text.contains(TOKEN));
    assert_eq!(
        args_text.lines().rev().take(2).collect::<Vec<_>>(),
        ["--config-only", "config"]
    );
    first.open().expect("revalidate compiled handle");

    let _second = compile_static_config(compile_req(
        &runtime,
        &lock_path,
        dir.path(),
        &data,
        &platform,
        &redactor,
        Duration::from_secs(5),
    ))
    .await
    .expect("reuse");
    let n2 = fs::read(&counter).unwrap().len();
    assert_eq!(n2, n1, "valid cache must not spawn compiler");

    fs::write(first.path(), b"corrupt").unwrap();
    assert!(first.open().is_err());
    let rebuilt = compile_static_config(compile_req(
        &runtime,
        &lock_path,
        dir.path(),
        &data,
        &platform,
        &redactor,
        Duration::from_secs(5),
    ))
    .await
    .expect("rebuild");
    let n3 = fs::read(&counter).unwrap().len();
    assert!(n3 > n2);

    let mut request = compile_req(
        &runtime,
        &lock_path,
        dir.path(),
        &data,
        &platform,
        &redactor,
        Duration::from_secs(5),
    );
    request.role = crate::ConfigRole::PythonPreparation;
    let prepared = compile_static_config(request).await.unwrap();
    assert_eq!(prepared.role(), crate::ConfigRole::PythonPreparation);
    assert_ne!(prepared.digest(), rebuilt.digest());
    assert_ne!(prepared.path(), rebuilt.path());
    let args_text = fs::read_to_string(&args).unwrap();
    assert_eq!(
        args_text.lines().rev().take(2).collect::<Vec<_>>(),
        ["--config-only", "prepareConfig"]
    );
    let count = fs::read(&counter).unwrap().len();
    let mut request = compile_req(
        &runtime,
        &lock_path,
        dir.path(),
        &data,
        &platform,
        &redactor,
        Duration::from_secs(5),
    );
    request.role = crate::ConfigRole::PythonPreparation;
    let reused = compile_static_config(request).await.unwrap();
    assert_eq!(reused.role(), crate::ConfigRole::PythonPreparation);
    assert_eq!(reused.digest(), prepared.digest());
    assert_eq!(fs::read(&counter).unwrap().len(), count);
    rebuilt.open().unwrap();
}
