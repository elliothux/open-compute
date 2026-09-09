use super::*;

#[tokio::test]
async fn real_compile_succeeds_when_env_set() {
    let path = std::env::var_os("OPEN_COMPUTE_TEST_WORKERD")
        .expect("OPEN_COMPUTE_TEST_WORKERD must name the verified stock runtime");
    let binary = PathBuf::from(path);
    let assets = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/runtime")
        .canonicalize()
        .unwrap();
    let lock_path = assets.join("workerd.lock.json");
    let runtime = verify_runtime_binary(
        &lock_path,
        &binary,
        Duration::from_secs(10),
        &Redactor::new(),
    )
    .await
    .unwrap();
    let dir = TempDir::new().unwrap();
    let data = dir.path().join("runtime");
    fs::create_dir(&data).unwrap();
    let token = SecretString::new(TOKEN);
    let mut redactor = Redactor::new();
    redactor.register_secret_string(&token);
    let platform = platform_meta();
    let compiled = compile_static_config(compile_req(
        &runtime,
        &lock_path,
        &assets,
        &data,
        &platform,
        &token,
        &redactor,
        Duration::from_secs(20),
    ))
    .await
    .expect("real compile");
    let mut file = compiled.open().unwrap();
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut file, &mut bytes).unwrap();
    assert!(!bytes.is_empty());
    assert!(!format!("{compiled:?}").contains(TOKEN));
    assert!(!compiled.to_string().contains(TOKEN));
    let err = open_compute_core::PlatformError::new(
        ErrorCode::ConfigCompileFailed,
        "workerd compile exited unsuccessfully",
    );
    assert!(!err.to_string().contains(TOKEN));
    assert!(!format!("{err:?}").contains(TOKEN));
}
