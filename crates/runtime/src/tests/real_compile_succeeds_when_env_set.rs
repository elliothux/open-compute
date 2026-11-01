use super::*;

#[tokio::test]
async fn real_compile_succeeds_when_env_set() {
    let path = std::env::var_os("OPEN_COMPUTE_TEST_WORKERD")
        .expect("OPEN_COMPUTE_TEST_WORKERD must name the verified pinned runtime");
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
        &redactor,
        Duration::from_secs(20),
    ))
    .await
    .expect("real compile");
    let mut file = compiled.open().unwrap();
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut file, &mut bytes).unwrap();
    assert!(!bytes.is_empty());
    assert!(
        bytes
            .windows(b"do-storage".len())
            .any(|window| window == b"do-storage")
    );
    let mut prepare_request = compile_req(
        &runtime,
        &lock_path,
        &assets,
        &data,
        &platform,
        &redactor,
        Duration::from_secs(20),
    );
    prepare_request.role = crate::ConfigRole::PythonPreparation;
    let prepared = compile_static_config(prepare_request)
        .await
        .expect("real prepare compile");
    let prepared_bytes = prepared.read_bytes().unwrap();
    assert_ne!(prepared.digest(), compiled.digest());
    assert_ne!(prepared_bytes, bytes);
    assert!(
        !prepared_bytes
            .windows(b"do-storage".len())
            .any(|window| window == b"do-storage")
    );
    assert!(
        prepared_bytes
            .windows(b"do-router".len())
            .any(|window| window == b"do-router")
    );
    let broker_socket = dir.path().join("broker.sock");
    let provider = crate::ContainerProviderConfig::new(
        broker_socket.clone(),
        format!("sha256:{}", "a".repeat(64)),
    )
    .unwrap();
    let socket_address = format!("unix:{}", broker_socket.to_str().unwrap());
    for (role, unconfigured, unconfigured_bytes) in [
        (crate::ConfigRole::Runtime, &compiled, &bytes),
        (
            crate::ConfigRole::PythonPreparation,
            &prepared,
            &prepared_bytes,
        ),
    ] {
        let mut request = compile_req(
            &runtime,
            &lock_path,
            &assets,
            &data,
            &platform,
            &redactor,
            Duration::from_secs(20),
        );
        request.role = role;
        request.container_provider = Some(&provider);
        let configured = compile_static_config(request)
            .await
            .expect("real compile with Container provider");
        let configured_bytes = configured.read_bytes().unwrap();
        assert_ne!(configured.digest(), unconfigured.digest());
        match role {
            crate::ConfigRole::Runtime => {
                assert_ne!(&configured_bytes, unconfigured_bytes);
                assert!(
                    configured_bytes
                        .windows(socket_address.len())
                        .any(|window| window == socket_address.as_bytes())
                );
            }
            crate::ConfigRole::PythonPreparation => {
                assert_eq!(&configured_bytes, unconfigured_bytes);
            }
        }
    }
    assert!(!broker_socket.exists());
    assert_eq!(compiled.read_bytes().unwrap(), bytes);
    assert!(!format!("{compiled:?}").contains(TOKEN));
    assert!(!compiled.to_string().contains(TOKEN));
    let err = open_compute_core::PlatformError::new(
        ErrorCode::ConfigCompileFailed,
        "workerd compile exited unsuccessfully",
    );
    assert!(!err.to_string().contains(TOKEN));
    assert!(!format!("{err:?}").contains(TOKEN));
}
