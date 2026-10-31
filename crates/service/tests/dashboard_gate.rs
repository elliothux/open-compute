//! Embedded dashboard and Cloudflare v4 boundary gate.

use axum::body::to_bytes;
use axum::http::{Request, StatusCode, header};
use open_compute_core::config::InstanceAuthConfig;
use open_compute_core::{DataConfig, SystemClock};
use open_compute_service::embedded_dashboard_files;
use open_compute_service::health::HealthCoordinator;
use open_compute_service::http::{HttpState, admin_router};
use open_compute_service::metrics::MetricsRegistry;
use open_compute_storage::PlatformStorage;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tower::ServiceExt;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dashboard_serves_embedded_spa_assets_and_cloudflare_v4_api() {
    let temp = tempfile::tempdir().unwrap();
    let storage = Arc::new(
        PlatformStorage::bootstrap(&storage_config(&temp.path().join("data")), &SystemClock)
            .unwrap(),
    );
    let admin_token = write_admin_secret(&temp.path().join("admin.token"));
    let metrics = Arc::new(
        MetricsRegistry::new(
            &open_compute_core::config::MetricsConfig::default(),
            "test",
            "workerd",
        )
        .unwrap(),
    );
    let server = open_compute_core::DaemonServerConfig {
        admin_auth: open_compute_core::config::SecretReference {
            env: None,
            file: Some(admin_token),
        },
        ..open_compute_core::DaemonServerConfig::default()
    };
    let auth = InstanceAuthConfig {
        deployer_auth: open_compute_core::config::SecretReference {
            env: None,
            file: Some(write_admin_secret_with_value(
                &temp.path().join("deployer.token"),
                "dashboard-gate-deployer",
            )),
        },
        read_only_auth: open_compute_core::config::SecretReference {
            env: None,
            file: Some(write_admin_secret_with_value(
                &temp.path().join("read-only.token"),
                "dashboard-gate-read-only",
            )),
        },
    };
    let state = HttpState::new(
        HealthCoordinator::new(),
        metrics,
        true,
        true,
        &server,
        &auth,
    )
    .expect("dashboard gate HTTP state")
    .with_platform_storage(storage.clone());
    let router = admin_router(state);

    let home = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/operator/")
                .header(header::HOST, "localhost")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(home.status(), StatusCode::OK);
    assert_eq!(
        home.headers()
            .get(header::CONTENT_SECURITY_POLICY)
            .and_then(|v| v.to_str().ok()),
        Some(
            "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src 'self'; base-uri 'self'; form-action 'self'; frame-ancestors 'none'; object-src 'none'"
        ),
        "dashboard shell must ship strict CSP"
    );
    assert_eq!(
        home.headers()
            .get(header::X_CONTENT_TYPE_OPTIONS)
            .and_then(|v| v.to_str().ok()),
        Some("nosniff")
    );
    assert!(
        home.headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/html")),
        "dashboard root must serve HTML"
    );
    let home_body = to_bytes(home.into_body(), 256 * 1024).await.unwrap();
    assert!(
        home_body
            .windows(b"<title>open-compute dashboard</title>".len())
            .any(|window| window == b"<title>open-compute dashboard</title>"),
        "dashboard root must serve the embedded SPA shell"
    );
    assert_dashboard_surface_excludes_admin_token(&home_body, "dashboard-gate-admin");
    assert_dashboard_asset_is_release_safe(&home_body);

    let deep_link = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/operator/login")
                .header(header::HOST, "localhost")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(deep_link.status(), StatusCode::OK);
    assert!(
        deep_link
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/html")),
        "client-side routes must fall back to the SPA shell"
    );
    let deep_link_body = to_bytes(deep_link.into_body(), 256 * 1024).await.unwrap();
    assert_dashboard_surface_excludes_admin_token(&deep_link_body, "dashboard-gate-admin");
    assert_dashboard_asset_is_release_safe(&deep_link_body);

    let asset_path = sample_asset_url_path();
    let asset = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(&asset_path)
                .header(header::HOST, "localhost")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(asset.status(), StatusCode::OK, "asset path={asset_path}");
    assert!(
        asset
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                value.starts_with("text/javascript") || value.starts_with("application/javascript")
            }),
        "hashed dashboard assets must be served from axum"
    );
    let asset_body = to_bytes(asset.into_body(), 2 * 1024 * 1024).await.unwrap();
    assert_dashboard_surface_excludes_admin_token(&asset_body, "dashboard-gate-admin");
    assert_dashboard_asset_is_release_safe(&asset_body);

    let meta = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/client/v4/open-compute/capabilities")
                .header(header::HOST, "localhost")
                .header(header::AUTHORIZATION, "Bearer dashboard-gate-admin")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(meta.status(), StatusCode::OK);
    assert!(
        meta.headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.contains("application/json")),
        "Cloudflare v4 API must not be handled by the dashboard SPA"
    );
    let meta_body = to_bytes(meta.into_body(), 64 * 1024).await.unwrap();
    let meta_json: serde_json::Value = serde_json::from_slice(&meta_body).unwrap();
    assert_eq!(meta_json["success"], true);
    assert_eq!(meta_json["result"]["cf_version"], "1.0.0-beta.12");
    assert_dashboard_surface_excludes_admin_token(&meta_body, "dashboard-gate-admin");

    let unauthorized = router
        .oneshot(
            Request::builder()
                .uri("/client/v4/accounts")
                .header(header::HOST, "localhost")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
}

fn sample_asset_url_path() -> String {
    let (path, _) = embedded_dashboard_files()
        .iter()
        .find(|(path, _)| path.starts_with("assets/") && path.ends_with(".js"))
        .expect("embedded dashboard must include a hashed JS asset");
    format!("/operator/{path}")
}

fn assert_dashboard_surface_excludes_admin_token(body: &[u8], token: &str) {
    let rendered = String::from_utf8_lossy(body).to_ascii_lowercase();
    for forbidden in [token, "bearer ", "authorization:"] {
        assert!(
            !rendered.contains(&forbidden.to_ascii_lowercase()),
            "dashboard surface leaked forbidden material: {forbidden}"
        );
    }
}

fn assert_dashboard_asset_is_release_safe(body: &[u8]) {
    let rendered = String::from_utf8_lossy(body).to_ascii_lowercase();
    for forbidden in ["localstorage", "sourcemappingurl", ".js.map"] {
        assert!(
            !rendered.contains(forbidden),
            "dashboard asset leaked forbidden material: {forbidden}"
        );
    }
}

fn write_admin_secret(path: &Path) -> PathBuf {
    write_admin_secret_with_value(path, "dashboard-gate-admin")
}

fn write_admin_secret_with_value(path: &Path, value: &str) -> PathBuf {
    std::fs::write(path, value).unwrap();
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o600);
    std::fs::set_permissions(path, permissions).unwrap();
    path.to_owned()
}

fn storage_config(root: &Path) -> DataConfig {
    DataConfig {
        path: root.to_owned(),
        master_key_file: root.join("keys/master.key"),
        master_key_env: None,
        sqlite_busy_timeout_ms: 5_000,
        free_space_soft_bytes: 1_073_741_824,
        free_space_hard_bytes: 268_435_456,
    }
}
