use super::*;
use crate::config_load::LoadedConfig;
use open_compute_core::PlatformError;

async fn startup_error(loaded: LoadedConfig) -> PlatformError {
    Box::pin(run_platform_with(loaded, RunOptions::default()))
        .await
        .unwrap_err()
}

#[tokio::test]
async fn run_startup_failure_matrix_releases_owned_resources() {
    let (dir, path, mock) = initialized_doctor_fixture().await;
    let base = load_fixture_platform_config(&path);

    let mut loaded = base.clone();
    loaded.config.metrics.max_series = 1;
    assert_eq!(startup_error(loaded).await.code(), ErrorCode::LimitInvalid);

    let mut loaded = base.clone();
    loaded.config.data.path = dir.path().join("not-a-directory");
    fs::write(&loaded.config.data.path, b"file").unwrap();
    assert_eq!(startup_error(loaded).await.code(), ErrorCode::PathInvalid);

    let package =
        open_compute_runtime::materialize_embedded_runtime(&base.config.data.path.join("runtime"))
            .unwrap();
    let asset = package.assets_dir().join("config.capnp");
    let original_runtime = fs::read(&asset).unwrap();
    fs::set_permissions(&asset, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&asset, b"tampered").unwrap();
    assert_eq!(
        startup_error(base.clone()).await.code(),
        ErrorCode::RuntimeInvalid
    );
    fs::write(&asset, original_runtime).unwrap();
    fs::set_permissions(&asset, fs::Permissions::from_mode(0o400)).unwrap();

    let mut loaded = base.clone();
    let s3 = loaded.config.object_storage.as_s3_mut().expect("S3 config");
    s3.access_key_id_env = Some(format!(
        "OPEN_COMPUTE_MISSING_RUN_KEY_{}",
        std::process::id()
    ));
    s3.access_key_id_file = None;
    assert_eq!(
        startup_error(loaded).await.code(),
        ErrorCode::SecretRefInvalid
    );

    let mut loaded = base.clone();
    loaded
        .config
        .object_storage
        .as_s3_mut()
        .expect("S3 config")
        .verify_tls = false;
    assert_eq!(startup_error(loaded).await.code(), ErrorCode::ConfigInvalid);

    mock.set_fault(open_compute_artifacts::Fault::Permission);
    let _ = startup_error(base.clone()).await;
    mock.set_fault(open_compute_artifacts::Fault::None);

    let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let occupied_addr = occupied.local_addr().unwrap();
    let mut loaded = base.clone();
    loaded.config.server.public_bind = occupied_addr.to_string();
    loaded.config.server.admin_bind = Some(occupied_addr.to_string());
    assert_eq!(startup_error(loaded).await.code(), ErrorCode::ConfigInvalid);
    drop(occupied);

    let occupied_admin = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut loaded = base;
    loaded.config.server.public_bind = "127.0.0.1:0".to_owned();
    loaded.config.server.admin_bind = Some(occupied_admin.local_addr().unwrap().to_string());
    assert_eq!(startup_error(loaded).await.code(), ErrorCode::ConfigInvalid);

    open_compute_storage::PlatformStorage::bootstrap(
        &load_fixture_platform_config(&path).config.data,
        &open_compute_core::SystemClock,
    )
    .expect("all startup failures released the data-dir lock");
}
