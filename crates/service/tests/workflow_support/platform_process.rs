//! Shared real platformd process ownership and private failure evidence for Workflow Gates.

use axum::body::{Body, to_bytes};
use axum::http::Request;
use std::fs;
use std::net::SocketAddr;
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub(crate) type Client =
    hyper_util::client::legacy::Client<hyper_util::client::legacy::connect::HttpConnector, Body>;

pub(crate) async fn response(
    client: &Client,
    address: SocketAddr,
    path: &str,
    method: &str,
) -> Result<axum::http::Response<hyper::body::Incoming>, ()> {
    let request = Request::builder()
        .method(method)
        .uri(format!("http://{address}{path}"))
        .header("host", "workflow.example")
        .header("content-type", "application/json")
        .body(Body::from("{}"))
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), client.request(request))
        .await
        .map_err(|_| ())?
        .map_err(|_| ())
}

pub(crate) async fn tenant_json(
    client: &Client,
    address: SocketAddr,
    path: &str,
) -> serde_json::Value {
    let response = response(client, address, path, "POST").await.unwrap();
    assert_eq!(response.status(), 200);
    let bytes = to_bytes(Body::new(response.into_body()), 65536)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

pub(crate) struct Process(pub(crate) Child, PathBuf, String);
impl Drop for Process {
    fn drop(&mut self) {
        if self.0.try_wait().unwrap().is_none() {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
        // Normal crash cuts deliberately leave orphan recovery to the next
        // platformd. On assertion failure, clean only the formally identified
        // child before retaining the rest of the failure evidence.
        if std::thread::panicking()
            && let Err(error) = open_compute_runtime::recover_orphan_for_test(&self.1, &self.2)
        {
            eprintln!("Workflow Gate orphan cleanup failed: {}", error.code());
        }
    }
}
pub(crate) struct Evidence(pub(crate) Option<tempfile::TempDir>);
impl Drop for Evidence {
    fn drop(&mut self) {
        if std::thread::panicking()
            && let Some(temp) = self.0.take()
        {
            let path = temp.keep();
            let failed = path.parent().unwrap().join("failed");
            let _ = fs::create_dir_all(&failed);
            let _ = fs::rename(&path, failed.join(path.file_name().unwrap()));
        }
    }
}

pub(crate) fn address() -> SocketAddr {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
}

pub(crate) fn spawn(config: &Path, log: &Path) -> Process {
    let parsed =
        open_compute_core::PlatformConfig::from_toml_str(&fs::read_to_string(config).unwrap())
            .unwrap();
    let (lock, _) = open_compute_runtime::embedded_runtime_lock().unwrap();
    let digest = lock.current_target().unwrap().1.binary_sha256.clone();
    let output = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(log)
        .unwrap();
    Process(
        Command::new(env!("CARGO_BIN_EXE_platformd"))
            .args(["run", "--config"])
            .arg(config)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(output)
            .spawn()
            .unwrap(),
        parsed.storage.data_dir.join("runtime/child.lease"),
        digest,
    )
}

pub(crate) async fn ready(client: &Client, admin: SocketAddr, child: &mut Process) {
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "platformd exited before readiness"
        );
        if response(client, admin, "/health/ready", "GET")
            .await
            .is_ok_and(|r| r.status() == 200)
        {
            return;
        }
        if Instant::now() >= deadline {
            let status = response(client, admin, "/health/status", "GET")
                .await
                .unwrap();
            let bytes = to_bytes(Body::new(status.into_body()), 65536)
                .await
                .unwrap();
            panic!(
                "process readiness timed out: {}",
                String::from_utf8_lossy(&bytes)
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

pub(crate) fn config(
    root: &Path,
    data: &Path,
    endpoint: &str,
    public: SocketAddr,
    admin: SocketAddr,
) -> PathBuf {
    let key = root.join("access-key");
    let secret = root.join("secret-key");
    fs::write(&key, "AKIAEXAMPLEKEYID01").unwrap();
    fs::write(&secret, "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY").unwrap();
    for path in [&key, &secret] {
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let config = root.join("process.toml");
    fs::write(
        &config,
        format!(
            r#"
[server]
public_bind = "{public}"
admin_bind = "{admin}"
[storage]
data_dir = "{}"
master_key_file = "{}"
[s3]
endpoint = "{endpoint}"
region = "us-east-1"
bucket = "open-compute"
prefix = "system/"
force_path_style = true
access_key_id_file = "{}"
secret_access_key_file = "{}"
max_retries = 1
[runtime]
startup_timeout_ms = 20000
shutdown_grace_ms = 500
[workflows]
lease_ms = 6000
heartbeat_ms = 1000
dispatch_timeout_ms = 300000
recovery_backoff_ms = 100
"#,
            data.display(),
            data.join("keys/master.key").display(),
            key.display(),
            secret.display(),
        ),
    )
    .unwrap();
    fs::set_permissions(&config, fs::Permissions::from_mode(0o600)).unwrap();
    config
}
