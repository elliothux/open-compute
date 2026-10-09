//! Explicit headless-shell installation and owned native CDP fixture.
use open_compute_core::Redactor;
use open_compute_runtime::browser::BrowserInstallation;
use open_compute_runtime::{PersistentHostProcess, PersistentHostProcessSpec};
use std::path::{Path, PathBuf};
use std::time::Duration;
pub(super) async fn native_fixture(workspace: &Path) -> (PersistentHostProcess, String) {
    let executable = PathBuf::from(
        std::env::var_os("OPEN_COMPUTE_TEST_BROWSER")
            .expect("explicit chrome-headless-shell fixture required"),
    );
    let installation = BrowserInstallation::open(&executable, workspace)
        .await
        .unwrap();
    let profile = workspace.join("profile");
    let process = PersistentHostProcess::spawn(
        installation.launch_image_for_test(),
        PersistentHostProcessSpec {
            args: vec![
                "--remote-debugging-address=127.0.0.1".into(),
                "--remote-debugging-port=0".into(),
                "--site-per-process".into(),
                "--disable-gpu".into(),
                "--no-first-run".into(),
                "--disable-background-networking".into(),
                format!("--user-data-dir={}", profile.display()).into(),
                "about:blank".into(),
            ],
            environment: vec![("HOME".into(), workspace.as_os_str().to_owned())],
            working_directory: workspace.to_owned(),
            private_fds: Vec::new(),
            lease_path: workspace.join("chrome.lease"),
            binary_sha256: installation.binary_sha256.clone(),
            redactor: Redactor::new(),
        },
    )
    .unwrap();
    let endpoint = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(text) = tokio::fs::read_to_string(profile.join("DevToolsActivePort")).await {
                let mut lines = text.lines();
                if let (Some(port), Some(path)) = (lines.next(), lines.next()) {
                    let port: u16 = port.parse().unwrap();
                    break format!("ws://127.0.0.1:{port}{path}");
                }
            }
            assert!(process.is_running());
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    (process, endpoint)
}

/// Exercise the fixed Puppeteer frame and frameElement APIs on a genuine cross-site iframe.
pub(super) async fn fixed_client_frames(
    client: &super::platform_process::Client,
    address: &str,
    host: &str,
    disconnect: bool,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let mut request = [0; 8192];
                let length = socket.read(&mut request).await.unwrap();
                let body = if request[..length].starts_with(b"GET /frame ") {
                    "<!doctype html><h1>Owned cross-site iframe</h1>".to_owned()
                } else {
                    format!(
                        "<!doctype html><h1>Parent</h1><iframe src=\"http://localhost:{port}/frame\"></iframe>"
                    )
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
            });
        }
    });
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("frame", &format!("http://127.0.0.1:{port}/"))
        .append_pair("disconnect", if disconnect { "true" } else { "false" })
        .finish();
    let (status, _, bytes) = super::request(
        client,
        address,
        host,
        &format!("/puppeteer?{query}"),
        "GET",
        None,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(
        status,
        200,
        "iframe client: {}",
        String::from_utf8_lossy(&bytes)
    );
    let result: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(result["text"], "Owned cross-site iframe");
    assert_eq!(result["element"], "IFRAME");
    server.abort();
    let _ = server.await;
}
