//! Official Cloudflare SDK and extension contract against the live v4 admin router.

use axum::Router;
use open_compute_core::config::{MetricsConfig, SecretReference, ServerConfig, StorageConfig};
use open_compute_service::health::HealthCoordinator;
use open_compute_service::http::{HttpState, admin_router};
use open_compute_service::metrics::MetricsRegistry;
use open_compute_storage::PlatformStorage;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::net::TcpListener;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn official_cloudflare_sdk_matches_live_v4_router_contract() {
    let temp = TempDir::new().expect("tempdir");
    let storage = platform_storage(&temp);
    let server_config = server_config(&temp);
    let metrics = Arc::new(
        MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd")
            .expect("metrics registry"),
    );
    let state = HttpState::new(
        HealthCoordinator::new(),
        metrics,
        false,
        false,
        &server_config,
    )
    .expect("v4 router state")
    .with_platform_storage(storage);
    let app: Router = admin_router(state);
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral admin listener");
    let addr = listener.local_addr().expect("admin listener address");
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve admin router");
    });

    let status = Command::new("bun")
        .arg("tests/live-router.mjs")
        .current_dir(repo_root().join("packages/cloudflare-extension"))
        .env(
            "OPEN_COMPUTE_V4_BASE_URL",
            format!("http://{addr}/client/v4"),
        )
        .env("OPEN_COMPUTE_V4_TOKEN", "admin-secret")
        .status()
        .expect("run official SDK live-router contract");
    server.abort();
    assert!(status.success(), "official SDK live-router contract failed");
}

fn platform_storage(temp: &TempDir) -> Arc<PlatformStorage> {
    let data_dir = temp.path().join("data");
    Arc::new(
        PlatformStorage::bootstrap(
            &StorageConfig {
                data_dir: data_dir.clone(),
                master_key_file: data_dir.join("keys/master.key"),
                master_key_env: None,
                sqlite_busy_timeout_ms: 5_000,
                free_space_soft_bytes: 1_073_741_824,
                free_space_hard_bytes: 268_435_456,
            },
            &open_compute_core::SystemClock,
        )
        .expect("platform storage"),
    )
}

fn server_config(temp: &TempDir) -> ServerConfig {
    ServerConfig {
        admin_auth: SecretReference {
            env: None,
            file: Some(write_secret(
                &temp.path().join("admin.token"),
                "admin-secret",
            )),
        },
        deployer_auth: SecretReference {
            env: None,
            file: Some(write_secret(
                &temp.path().join("deployer.token"),
                "deployer-secret",
            )),
        },
        read_only_auth: SecretReference {
            env: None,
            file: Some(write_secret(
                &temp.path().join("read-only.token"),
                "read-only-secret",
            )),
        },
        ..ServerConfig::default()
    }
}

fn write_secret(path: &Path, value: &str) -> PathBuf {
    fs::write(path, format!("{value}\n")).expect("write bearer token");
    let mut permissions = fs::metadata(path)
        .expect("bearer token metadata")
        .permissions();
    permissions.set_mode(0o600);
    fs::set_permissions(path, permissions).expect("bearer token permissions");
    path.to_owned()
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace crates directory")
        .parent()
        .expect("workspace root")
        .to_owned()
}
