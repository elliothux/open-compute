use axum::body::{Body, to_bytes};
use axum::http::{HeaderMap, Request};
use serde_json::Value;
use std::path::Path;
use std::time::Duration;
use tokio::time::{Instant, timeout_at};

pub(super) type Client = super::platform_process::Client;

pub(super) async fn request(
    client: &Client,
    address: &str,
    host: &str,
    path: &str,
    method: &str,
    authorization: Option<&str>,
    media: &str,
    body: Vec<u8>,
) -> (u16, HeaderMap, Vec<u8>) {
    let mut request = Request::builder()
        .method(method)
        .uri(format!("http://{address}{path}"))
        .header("host", host)
        .header("content-type", media);
    if let Some(authorization) = authorization {
        request = request.header("authorization", authorization);
    }
    let started = Instant::now();
    let deadline = started + Duration::from_secs(180);
    let response = timeout_at(
        deadline,
        client.request(request.body(Body::from(body)).unwrap()),
    )
    .await
    .unwrap_or_else(|_| {
        panic!(
            "Container HTTP headers timed out: elapsed_ms={}",
            started.elapsed().as_millis()
        )
    })
    .unwrap_or_else(|_| panic!("Container HTTP headers request failed"));
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let body = timeout_at(
        deadline,
        to_bytes(Body::new(response.into_body()), 16 << 20),
    )
    .await
    .unwrap_or_else(|_| {
        panic!(
            "Container HTTP body timed out: elapsed_ms={}",
            started.elapsed().as_millis()
        )
    })
    .unwrap_or_else(|_| panic!("Container HTTP bounded body read failed"));
    (status, headers, body.to_vec())
}

pub(super) async fn json(
    client: &Client,
    address: &str,
    host: &str,
    path: &str,
    method: &str,
    authorization: Option<&str>,
    body: Value,
) -> Value {
    let bytes = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(&body).unwrap()
    };
    let (status, _, body) = request(
        client,
        address,
        host,
        path,
        method,
        authorization,
        "application/json",
        bytes,
    )
    .await;
    assert!(
        (200..300).contains(&status),
        "HTTP {method} {path} failed with {status}: {}",
        String::from_utf8_lossy(&body)
    );
    serde_json::from_slice(&body).unwrap()
}

pub(super) async fn cf(project: &Path, state: &Path, address: &str, account: &str, args: &[&str]) {
    let root = super::repo();
    let package: Value =
        serde_json::from_slice(&std::fs::read(root.join("node_modules/cf/package.json")).unwrap())
            .unwrap();
    assert_eq!(package["version"], "1.0.0-beta.12");
    let output = tokio::time::timeout(
        Duration::from_secs(60),
        tokio::process::Command::new("node")
            .arg(root.join("node_modules/cf/bin/cf"))
            .args(args)
            .current_dir(project)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env(
                "CLOUDFLARE_API_BASE_URL",
                format!("http://{address}/client/v4"),
            )
            .env("CLOUDFLARE_API_TOKEN", "workflow-deployer")
            .env("CLOUDFLARE_ACCOUNT_ID", account)
            .env("XDG_CONFIG_HOME", state)
            .env("CF_SEND_TELEMETRY", "false")
            .env("DO_NOT_TRACK", "1")
            .env("CI", "true")
            .env("NO_COLOR", "1")
            .kill_on_drop(true)
            .output(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        output.status.success(),
        "pinned cf {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    for secret in ["workflow-deployer", "workflow-admin", "p23-owned-startup"] {
        assert!(!String::from_utf8_lossy(&output.stdout).contains(secret));
        assert!(!String::from_utf8_lossy(&output.stderr).contains(secret));
    }
}
