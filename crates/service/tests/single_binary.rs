//! The actual executable must work alone, with no checkout, PATH tools, or external runtime.

use open_compute_artifacts::MockS3;
use open_compute_core::config::SecretReference;
use open_compute_core::{
    LocalExtensionConfig, LocalObjectStorageConfig, ObjectStorageConfig, PlatformConfig, S3Config,
};
use open_compute_runtime::{
    embedded_payload_sha256, embedded_runtime_lock, recover_orphan_for_test,
};
use open_compute_service::instance_control::request_shutdown;
use open_compute_service::instance_registry::ServiceScope;
use open_compute_storage::PlatformStorage;
use rustix::process::{Pid, Signal, kill_process};
use std::fs;
use std::net::SocketAddr;
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, SystemTime};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

#[path = "single_binary/package_scope.rs"]
mod package_scope;
#[path = "single_binary/provider_ack.rs"]
mod provider_ack;
use provider_ack::assert_stale_provider_ack_is_scoped;

struct Evidence(Option<TempDir>);

impl Evidence {
    fn new() -> Self {
        // Keep the socket path short; retain failures under the repository .temp tree.
        Self(Some(
            tempfile::Builder::new()
                .prefix("single-")
                .tempdir_in("/tmp")
                .unwrap(),
        ))
    }

    fn path(&self) -> &Path {
        self.0.as_ref().unwrap().path()
    }
}

impl Drop for Evidence {
    fn drop(&mut self) {
        if std::thread::panicking()
            && let Some(temp) = self.0.take()
        {
            let path = temp.keep();
            let failed =
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.temp/single-binary-run/failed");
            if fs::create_dir_all(&failed).is_ok() {
                let destination = failed.join(path.file_name().unwrap());
                if fs::rename(&path, &destination).is_ok() {
                    eprintln!("single-binary failure evidence: {}", destination.display());
                    return;
                }
            }
            eprintln!("single-binary failure evidence: {}", path.display());
        }
    }
}

fn isolated_binary(root: &Path) -> PathBuf {
    let binary = root.join("ocd");
    let source = std::env::var_os("OPEN_COMPUTE_TEST_OCD")
        .map_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_ocd")), PathBuf::from);
    assert!(source.is_absolute());
    fs::copy(source, &binary).unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o500)).unwrap();
    binary
}

fn command(binary: &Path) -> Command {
    let mut command = Command::new(binary);
    command
        .current_dir(binary.parent().unwrap())
        .env_clear()
        .env("PATH", "")
        .env("HOME", binary.parent().unwrap().join("home"))
        // Keep the test's OS temporary root so panic cleanup inspects the same staging root.
        .env("TMPDIR", std::env::temp_dir());
    if !package_scope::enabled() {
        command.env(
            "OPEN_COMPUTE_TEST_OCD_ROOT",
            binary.parent().unwrap().join("test-ocd"),
        );
    }
    // Preserve the harness's output destination without giving the child ambient runtime inputs.
    if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
        command.env("LLVM_PROFILE_FILE", profile);
    }
    command
}

fn successful(binary: &Path, args: &[&str]) -> Output {
    let output = command(binary).args(args).output().unwrap();
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

#[test]
fn readonly_commands_need_only_the_single_executable() {
    let root = Evidence::new();
    let binary = isolated_binary(root.path());
    for args in [
        vec!["--version"],
        vec!["--help"],
        vec!["licenses"],
        vec!["docs", "install-and-first-start"],
    ] {
        assert!(!successful(&binary, &args).stdout.is_empty());
    }
    let data = root.path().join("uninitialized");
    let output = successful(
        &binary,
        &["config", "init", "--data-dir", data.to_str().unwrap()],
    );
    let config =
        PlatformConfig::from_toml_str(std::str::from_utf8(&output.stdout).unwrap()).unwrap();
    assert_eq!(config.data.path, data);
    assert_eq!(config.data.master_key_file, data.join("keys/master.key"));
    let config_path = root.path().join("config.toml");
    fs::write(&config_path, &output.stdout).unwrap();
    fs::set_permissions(&config_path, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(
        !successful(
            &binary,
            &[
                "--config",
                config_path.to_str().unwrap(),
                "capabilities",
                "--json"
            ]
        )
        .stdout
        .is_empty()
    );
    assert!(
        !data.exists(),
        "read-only commands must not initialize a data dir"
    );
    assert!(
        !command(&binary)
            .arg("package-release")
            .output()
            .unwrap()
            .status
            .success()
    );
    let relative = successful(&binary, &["config", "init", "--data-dir", "relative"]);
    let relative_config =
        PlatformConfig::from_toml_str(std::str::from_utf8(&relative.stdout).unwrap()).unwrap();
    assert_eq!(
        relative_config.data.path,
        root.path().canonicalize().unwrap().join("relative")
    );
    let config_path = root.path().join("config.toml");
    fs::write(&config_path, output.stdout).unwrap();
    successful(
        &binary,
        &["--config", config_path.to_str().unwrap(), "config", "check"],
    );
    let scoped_run = command(&binary)
        .args(["run", "--config", config_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!scoped_run.status.success());
    assert!(String::from_utf8_lossy(&scoped_run.stderr).contains("CONFIG_PATH_INVALID"));
    let doctor = command(&binary)
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "doctor",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        !doctor.status.success(),
        "uninitialized data must not look healthy"
    );
    assert!(
        !data.exists(),
        "basic doctor must not initialize data or materialize the runtime"
    );
}

struct Process {
    binary: PathBuf,
    child: Child,
    leases: Vec<PathBuf>,
    gateway_lease: PathBuf,
    digest: String,
    log: PathBuf,
}

impl Process {
    fn spawn(binary: &Path, data: &Path, log: &Path) -> Self {
        let file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(log)
            .unwrap();
        let child = command(binary)
            .arg("run")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(file)
            .spawn()
            .unwrap();
        let (lock, _) = embedded_runtime_lock().unwrap();
        Self {
            binary: binary.to_owned(),
            child,
            leases: vec![data.join("runtime/child.lease")],
            gateway_lease: package_scope::registry(binary.parent().unwrap())
                .root_for(ServiceScope::User)
                .join("gateway/run/caddy.lease"),
            digest: lock.current_target().unwrap().1.binary_sha256.clone(),
            log: log.to_owned(),
        }
    }

    fn with_instance(mut self, data: &Path) -> Self {
        self.leases.push(data.join("runtime/child.lease"));
        self
    }

    async fn ready(&mut self, address: SocketAddr) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
        loop {
            assert!(
                self.child.try_wait().unwrap().is_none(),
                "ocd exited: {}",
                fs::read_to_string(&self.log).unwrap()
            );
            if let Ok(Ok(true)) =
                tokio::time::timeout(Duration::from_secs(1), ready_request(address)).await
                && let Ok(output) = command(&self.binary).args(["instances", "--json"]).output()
                && output.status.success()
                && let Ok(value) = serde_json::from_slice::<serde_json::Value>(&output.stdout)
                && value["instances"].as_array().is_some_and(|instances| {
                    instances
                        .iter()
                        .filter(|instance| instance["state"] == "running")
                        .count()
                        == self.leases.len()
                })
            {
                return;
            }
            if tokio::time::Instant::now() >= deadline {
                let instances = command(&self.binary)
                    .args(["instances", "--json"])
                    .output()
                    .unwrap();
                panic!(
                    "not ready: stderr={} instances={} cli_error={}",
                    fs::read_to_string(&self.log).unwrap(),
                    String::from_utf8_lossy(&instances.stdout),
                    String::from_utf8_lossy(&instances.stderr)
                );
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn stop(&mut self) {
        kill_process(Pid::from_raw(self.child.id() as i32).unwrap(), Signal::TERM).unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(
                    status.success(),
                    "shutdown failed: {}",
                    fs::read_to_string(&self.log).unwrap()
                );
                break;
            }
            assert!(tokio::time::Instant::now() < deadline, "ocd did not stop");
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        for lease in &self.leases {
            assert!(
                !lease.exists(),
                "normal shutdown must remove its child lease"
            );
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        for lease in &self.leases {
            if let Err(error) = recover_orphan_for_test(lease, &self.digest) {
                eprintln!("single-binary fixture cleanup failed: {}", error.code());
            }
        }
        if self.gateway_lease.exists()
            && let Err(error) = open_compute_runtime::PersistentHostProcess::recover_recorded_orphan(
                &self.gateway_lease,
            )
        {
            eprintln!("single-binary gateway cleanup failed: {}", error.code());
        }
    }
}

async fn ready_request(address: SocketAddr) -> std::io::Result<bool> {
    let mut stream = tokio::net::TcpStream::connect(address).await?;
    stream
        .write_all(b"GET /health/ready HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await?;
    let mut bytes = Vec::new();
    stream.take(16384).read_to_end(&mut bytes).await?;
    Ok(bytes.starts_with(b"HTTP/1.1 200"))
}

async fn request_status(address: SocketAddr, host: &str, token: &str, path: &str) -> u16 {
    request_http(address, "GET", host, token, path).await.0
}

async fn request_http(
    address: SocketAddr,
    method: &str,
    host: &str,
    token: &str,
    path: &str,
) -> (u16, String) {
    request_http_body(address, method, host, token, path, "").await
}

async fn request_http_body(
    address: SocketAddr,
    method: &str,
    host: &str,
    token: &str,
    path: &str,
    body: &str,
) -> (u16, String) {
    request_http_body_with_headers(address, method, host, token, path, body, "").await
}

async fn request_http_body_with_headers(
    address: SocketAddr,
    method: &str,
    host: &str,
    token: &str,
    path: &str,
    body: &str,
    extra_headers: &str,
) -> (u16, String) {
    let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
    let content_type = if body.is_empty() || extra_headers.contains("Content-Type:") {
        ""
    } else {
        "Content-Type: application/json\r\n"
    };
    stream
        .write_all(
            format!(
                "{method} {path} HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer {token}\r\n{content_type}{extra_headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len(),
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).await.unwrap();
    let status_line = std::str::from_utf8(&bytes).unwrap().lines().next().unwrap();
    let status = status_line
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let body = std::str::from_utf8(&bytes)
        .unwrap()
        .split_once("\r\n\r\n")
        .unwrap()
        .1
        .to_owned();
    (status, body)
}

async fn assert_asset_upload_token_isolation(address: SocketAddr, alpha: &str, beta: &str) {
    let hash = "4c73266e449fea54bba5a6dea074dbbd";
    let body = format!(r#"{{"manifest":{{"/index.html":{{"hash":"{hash}","size":4}}}}}}"#);
    let mut tokens = Vec::new();
    for (instance, deployer) in [(alpha, "alpha-deployer"), (beta, "beta-deployer")] {
        let path = format!(
            "/client/v4/accounts/{instance}/workers/scripts/shared-worker/assets-upload-session"
        );
        let (status, response) =
            request_http_body(address, "POST", "127.0.0.1", deployer, &path, &body).await;
        assert_eq!(status, 200, "{response}");
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();
        tokens.push(response["result"]["jwt"].as_str().unwrap().to_owned());
    }
    for (instance, foreign_token) in [(alpha, &tokens[1]), (beta, &tokens[0])] {
        let path = format!("/client/v4/accounts/{instance}/workers/assets/upload/{hash}");
        let (status, _) = request_http_body_with_headers(
            address,
            "POST",
            "127.0.0.1",
            foreign_token,
            &path,
            "good",
            "Content-Type: text/html\r\n",
        )
        .await;
        assert_eq!(status, 401, "another instance's upload token was accepted");
    }
}

async fn websocket_handshake_status(address: SocketAddr, path: &str, protocol: &str) -> u16 {
    let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
    stream
        .write_all(
            format!(
                "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n{protocol}\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut bytes = [0_u8; 1024];
    let read = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    std::str::from_utf8(&bytes[..read])
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap()
}

async fn assert_signed_tail_isolation(address: SocketAddr, alpha: &str, beta: &str) {
    let tails = format!("/client/v4/accounts/{alpha}/workers/scripts/shared-worker/tails");
    let (status, body) =
        request_http_body(address, "POST", "127.0.0.1", "alpha-deployer", &tails, "[]").await;
    assert_eq!(status, 200, "{body}");
    let created: serde_json::Value = serde_json::from_str(&body).unwrap();
    let url = url::Url::parse(created["result"]["url"].as_str().unwrap()).unwrap();
    let own = url.path();
    let other = own.replace(&format!("/tails/{alpha}/"), &format!("/tails/{beta}/"));
    assert_ne!(own, other);
    let protocol = "Sec-WebSocket-Protocol: trace-v1\r\n";
    assert_eq!(
        websocket_handshake_status(address, &other, protocol).await,
        404
    );
    assert_eq!(
        websocket_handshake_status(address, own, protocol).await,
        101
    );

    let live = format!("/client/v4/accounts/{alpha}/workers/observability/telemetry/live-tail");
    let (status, body) = request_http_body(
        address,
        "POST",
        "127.0.0.1",
        "alpha-deployer",
        &live,
        r#"{"scriptId":"shared-worker"}"#,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let created: serde_json::Value = serde_json::from_str(&body).unwrap();
    let url = url::Url::parse(created["result"]["wsUrl"].as_str().unwrap()).unwrap();
    let own = url.path();
    let other = own.replace(
        &format!("/live-tails/{alpha}/"),
        &format!("/live-tails/{beta}/"),
    );
    assert_ne!(own, other);
    assert_eq!(websocket_handshake_status(address, &other, "").await, 404);
    assert_eq!(websocket_handshake_status(address, own, "").await, 101);
}

async fn assert_named_resource_isolation(address: SocketAddr, alpha: &str, beta: &str) {
    let namespaces_a = format!("/client/v4/accounts/{alpha}/storage/kv/namespaces");
    let namespaces_b = format!("/client/v4/accounts/{beta}/storage/kv/namespaces");
    let (status_a, created_a) = request_http_body(
        address,
        "POST",
        "127.0.0.1",
        "alpha-deployer",
        &namespaces_a,
        r#"{"title":"shared-name"}"#,
    )
    .await;
    let (status_b, created_b) = request_http_body(
        address,
        "POST",
        "127.0.0.1",
        "beta-deployer",
        &namespaces_b,
        r#"{"title":"shared-name"}"#,
    )
    .await;
    assert_eq!((status_a, status_b), (200, 200), "{created_a} {created_b}");
    let id_a = serde_json::from_str::<serde_json::Value>(&created_a).unwrap()["result"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let id_b = serde_json::from_str::<serde_json::Value>(&created_b).unwrap()["result"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(id_a, id_b);
    assert_ne!(
        request_status(address, "127.0.0.1", "beta-deployer", &namespaces_a).await,
        200
    );
    assert_ne!(
        request_status(address, "127.0.0.1", "alpha-deployer", &namespaces_b).await,
        200
    );
    for (token, path, id) in [
        ("alpha-deployer", &namespaces_a, &id_a),
        ("beta-deployer", &namespaces_b, &id_b),
    ] {
        let (status, body) = request_http(address, "GET", "127.0.0.1", token, path).await;
        assert_eq!(status, 200, "{body}");
        let listed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(listed["result"].as_array().unwrap().len(), 1);
        assert_eq!(listed["result"][0]["id"], *id);
    }
    for (token, path, value) in [
        (
            "alpha-deployer",
            format!("{namespaces_a}/{id_a}/values/shared-key"),
            "alpha-value",
        ),
        (
            "beta-deployer",
            format!("{namespaces_b}/{id_b}/values/shared-key"),
            "beta-value",
        ),
    ] {
        let (status, body) =
            request_http_body(address, "PUT", "127.0.0.1", token, &path, value).await;
        assert_eq!(status, 200, "{body}");
        let (status, body) = request_http(address, "GET", "127.0.0.1", token, &path).await;
        assert_eq!((status, body.as_str()), (200, value));
    }
    for (surface, body) in [
        ("r2/buckets", r#"{"name":"shared-name"}"#),
        ("d1/database", r#"{"name":"shared-name"}"#),
        ("queues", r#"{"queue_name":"shared-name"}"#),
    ] {
        let path_a = format!("/client/v4/accounts/{alpha}/{surface}");
        let path_b = format!("/client/v4/accounts/{beta}/{surface}");
        for (token, path) in [("alpha-deployer", &path_a), ("beta-deployer", &path_b)] {
            let (status, response) =
                request_http_body(address, "POST", "127.0.0.1", token, path, body).await;
            assert!(matches!(status, 200 | 201), "{surface} {status} {response}");
            assert_eq!(
                request_status(address, "127.0.0.1", token, path).await,
                200,
                "{surface} must remain visible to its own instance"
            );
        }
        assert_ne!(
            request_status(address, "127.0.0.1", "beta-deployer", &path_a).await,
            200
        );
        assert_ne!(
            request_status(address, "127.0.0.1", "alpha-deployer", &path_b).await,
            200
        );
    }
    for (token, instance_id, value) in [
        ("alpha-deployer", alpha, "alpha-object"),
        ("beta-deployer", beta, "beta-object"),
    ] {
        let path =
            format!("/client/v4/accounts/{instance_id}/r2/buckets/shared-name/objects/shared-key");
        let (status, body) =
            request_http_body(address, "PUT", "127.0.0.1", token, &path, value).await;
        assert_eq!(status, 200, "{body}");
        let (status, body) = request_http(address, "GET", "127.0.0.1", token, &path).await;
        assert_eq!((status, body.as_str()), (200, value));
    }
    assert_d1_isolation(address, alpha, beta).await;
}

async fn assert_git_isolation(address: SocketAddr, alpha: &str, beta: &str) {
    let mut tokens = Vec::new();
    for (instance_id, deployer) in [(alpha, "alpha-deployer"), (beta, "beta-deployer")] {
        let namespaces = format!("/client/v4/accounts/{instance_id}/artifacts/namespaces");
        let (status, body) = request_http_body(
            address,
            "POST",
            "127.0.0.1",
            deployer,
            &namespaces,
            r#"{"namespace":"apps"}"#,
        )
        .await;
        assert_eq!(status, 200, "{body}");
        let repos = format!("{namespaces}/apps/repos");
        let (status, body) = request_http_body(
            address,
            "POST",
            "127.0.0.1",
            deployer,
            &repos,
            r#"{"name":"shared"}"#,
        )
        .await;
        assert_eq!(status, 200, "{body}");
        let created: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(
            created["result"]["remote"]
                .as_str()
                .unwrap()
                .ends_with(&format!("/git/{instance_id}/apps/shared.git"))
        );
        tokens.push(created["result"]["token"].as_str().unwrap().to_owned());
    }
    for (instance_id, own, other) in [
        (alpha, tokens[0].as_str(), tokens[1].as_str()),
        (beta, tokens[1].as_str(), tokens[0].as_str()),
    ] {
        let path = format!("/git/{instance_id}/apps/shared.git/info/refs?service=git-upload-pack");
        assert_eq!(request_status(address, "127.0.0.1", own, &path).await, 200);
        assert_ne!(
            request_status(address, "127.0.0.1", other, &path).await,
            200
        );
    }
}

async fn assert_d1_isolation(address: SocketAddr, alpha: &str, beta: &str) {
    for (token, instance_id, value) in [
        ("alpha-deployer", alpha, "alpha-row"),
        ("beta-deployer", beta, "beta-row"),
    ] {
        let collection = format!("/client/v4/accounts/{instance_id}/d1/database");
        let (status, body) = request_http(address, "GET", "127.0.0.1", token, &collection).await;
        assert_eq!(status, 200, "{body}");
        let listed: serde_json::Value = serde_json::from_str(&body).unwrap();
        let database_id = listed["result"][0]["uuid"].as_str().unwrap();
        let path = format!("{collection}/{database_id}/query");
        let (status, body) = request_http_body(
            address,
            "POST",
            "127.0.0.1",
            token,
            &path,
            &serde_json::json!({"batch": [
                {"sql": "CREATE TABLE entries(value TEXT)"},
                {"sql": "INSERT INTO entries VALUES (?)", "params": [value]}
            ]})
            .to_string(),
        )
        .await;
        assert_eq!(status, 200, "{body}");
        let (status, body) = request_http_body(
            address,
            "POST",
            "127.0.0.1",
            token,
            &path,
            r#"{"sql":"SELECT value FROM entries"}"#,
        )
        .await;
        assert_eq!(status, 200, "{body}");
        let rows: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            rows["result"][0]["results"],
            serde_json::json!([{"value": value}])
        );
    }
}

async fn queue_public_id(address: SocketAddr, instance_id: &str, token: &str) -> String {
    let path = format!("/client/v4/accounts/{instance_id}/queues");
    let (status, body) = request_http(address, "GET", "127.0.0.1", token, &path).await;
    assert_eq!(status, 200, "{body}");
    let listed: serde_json::Value = serde_json::from_str(&body).unwrap();
    listed["result"][0]["queue_id"].as_str().unwrap().to_owned()
}

fn shared_test_extension(root: &Path) -> PathBuf {
    let extension = root.join("local-files");
    fs::create_dir(&extension).unwrap();
    fs::set_permissions(&extension, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(
        extension.join("extension.toml"),
        "[worker]\nmain = 'facade.js'\n[native]\nexecutable = 'provider'\n",
    )
    .unwrap();
    fs::write(
        extension.join("facade.js"),
        "import { WorkerEntrypoint } from 'cloudflare:workers'; export default class Files extends WorkerEntrypoint { async read(name) { return new Response(this.env.HOST.stream(2, new TextEncoder().encode(`${this.ctx.props.directory}/${name}`))).text(); } }",
    )
    .unwrap();
    let binary = extension.join("provider");
    fs::copy(env!("CARGO_BIN_EXE_host-extension-test-provider"), &binary).unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
    extension
}

async fn interactive_instance_setup(
    binary: &Path,
    config: &Path,
    data: &Path,
    confirm: bool,
) -> Output {
    use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
    let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).unwrap();
    grantpt(&master).unwrap();
    unlockpt(&master).unwrap();
    let slave_path = ptsname(&master, Vec::new()).unwrap();
    let slave = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(slave_path.to_str().unwrap())
        .unwrap();
    let mut process = tokio::process::Command::new(binary);
    process
        .current_dir(binary.parent().unwrap())
        .env_clear()
        .env("PATH", "")
        .env("HOME", binary.parent().unwrap().join("home"))
        .args([
            "instance",
            "setup",
            "--name",
            "pty",
            "--config",
            config.to_str().unwrap(),
            "--data-dir",
            data.to_str().unwrap(),
            "--autostart=false",
            "--start=false",
        ])
        .stdin(Stdio::from(slave))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if !package_scope::enabled() {
        process.env(
            "OPEN_COMPUTE_TEST_OCD_ROOT",
            binary.parent().unwrap().join("test-ocd"),
        );
    }
    let child = process.spawn().unwrap();
    let mut master = fs::File::from(master);
    let answers: &[u8] = if confirm {
        b"\n\n\n\n\nyes\n"
    } else {
        b"\n\n\n\n\nno\n"
    };
    std::io::Write::write_all(&mut master, answers).unwrap();
    tokio::time::timeout(Duration::from_secs(15), child.wait_with_output())
        .await
        .expect("interactive instance setup timed out")
        .unwrap()
}

async fn assert_shared_worker_isolation(address: SocketAddr, alpha: &str, beta: &str) {
    let boundary = "ocd-r1-worker-upload";
    for (instance_id, token, value) in [
        (alpha, "alpha-deployer", "alpha-worker"),
        (beta, "beta-deployer", "beta-worker"),
    ] {
        let metadata = r#"{"main_module":"index.js","compatibility_date":"2026-09-08","bindings":[{"name":"QUEUE","type":"queue","queue_name":"shared-name"},{"name":"STATE","type":"durable_object_namespace","class_name":"State"},{"name":"FILES","type":"service","service":"local-files","props":{"directory":"workspace"}}],"migrations":{"new_tag":"v1","steps":[{"new_sqlite_classes":["State"]}]}}"#;
        let source = format!(
            "import {{ DurableObject }} from 'cloudflare:workers'; export class State extends DurableObject {{ async fetch(request) {{ const input = new URL(request.url).searchParams.get('value'); if (input !== null) this.ctx.storage.kv.put('value', input); return new Response(this.ctx.storage.kv.get('value') ?? 'empty'); }} }} export default {{ async fetch(request, env) {{ const path = new URL(request.url).pathname; if (path === '/enqueue') {{ await env.QUEUE.send('{value}'); return new Response('sent'); }} if (path === '/do') return env.STATE.get(env.STATE.idFromName('shared-object')).fetch(request); if (path === '/provider') return new Response(await env.FILES.read('owner.txt')); return new Response('{value}'); }} }};"
        );
        let body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"metadata\"\r\nContent-Type: application/json\r\n\r\n{metadata}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"index.js\"; filename=\"index.js\"\r\nContent-Type: application/javascript+module\r\n\r\n{source}\r\n--{boundary}--\r\n"
        );
        let (status, response) = request_http_body_with_headers(
            address,
            "PUT",
            "127.0.0.1",
            token,
            &format!("/client/v4/accounts/{instance_id}/workers/scripts/shared-worker"),
            &body,
            &format!("Content-Type: multipart/form-data; boundary={boundary}\r\n"),
        )
        .await;
        assert_eq!(status, 200, "{response}");
    }
    for (instance_id, token, value, other) in [
        (alpha, "alpha-deployer", "alpha-worker", beta),
        (beta, "beta-deployer", "beta-worker", alpha),
    ] {
        let host = format!("shared-worker.{instance_id}.localhost");
        for path in ["/", "/operator/api/instances"] {
            let (status, body) = request_http_body_with_headers(
                address,
                "GET",
                &host,
                token,
                path,
                "",
                &format!("x-open-compute-instance-id: {other}\r\n"),
            )
            .await;
            assert_eq!(status, 200, "{host} {path}: {body}");
            assert!(body.contains(value), "{host} {path}: {body}");
            assert!(!body.contains(other), "forged instance header leaked");
        }
    }
    let queue_a = queue_public_id(address, alpha, "alpha-deployer").await;
    let queue_b = queue_public_id(address, beta, "beta-deployer").await;
    let (status, body) = request_http(
        address,
        "GET",
        &format!("shared-worker.{alpha}.localhost"),
        "alpha-deployer",
        "/enqueue",
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("sent"), "{body}");
    for (instance_id, token, queue_id, count) in [
        (alpha, "alpha-deployer", &queue_a, 1),
        (beta, "beta-deployer", &queue_b, 0),
    ] {
        let path = format!("/client/v4/accounts/{instance_id}/queues/{queue_id}/metrics");
        let (status, body) = request_http(address, "GET", "127.0.0.1", token, &path).await;
        assert_eq!(status, 200, "{body}");
        let metrics: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(metrics["result"]["backlog_count"], count);
    }
    for (instance_id, token, query, expected) in [
        (alpha, "alpha-deployer", "?value=alpha-do", "alpha-do"),
        (beta, "beta-deployer", "", "empty"),
        (beta, "beta-deployer", "?value=beta-do", "beta-do"),
        (alpha, "alpha-deployer", "", "alpha-do"),
    ] {
        let host = format!("shared-worker.{instance_id}.localhost");
        let (status, body) =
            request_http(address, "GET", &host, token, &format!("/do{query}")).await;
        assert_eq!(status, 200, "{host} {body}");
        assert!(body.contains(expected), "{host} {body}");
    }
}

async fn assert_provider_isolation(
    address: SocketAddr,
    alpha: &str,
    beta: &str,
    data_a: &Path,
    data_b: &Path,
) -> (i64, i64) {
    let mut pids = Vec::new();
    for (instance_id, token, data, owner) in [
        (alpha, "alpha-deployer", data_a, "alpha-provider"),
        (beta, "beta-deployer", data_b, "beta-provider"),
    ] {
        let work_dir = data.join("runtime/extensions/local-files");
        assert!(work_dir.is_dir());
        let workspace = work_dir.join("workspace");
        fs::create_dir(&workspace).unwrap();
        fs::set_permissions(&workspace, fs::Permissions::from_mode(0o700)).unwrap();
        let file = workspace.join("owner.txt");
        fs::write(&file, owner).unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
        let host = format!("shared-worker.{instance_id}.localhost");
        let (status, body) = request_http(address, "GET", &host, token, "/provider").await;
        assert_eq!(status, 200, "{host} {body}");
        assert!(body.contains(owner), "{host} {body}");
        let lease: serde_json::Value =
            serde_json::from_slice(&fs::read(work_dir.join("provider.lease")).unwrap()).unwrap();
        pids.push(lease["pid"].as_i64().unwrap());
    }
    assert_ne!(pids[0], pids[1]);
    let crashed = Pid::from_raw(i32::try_from(pids[0]).unwrap()).unwrap();
    kill_process(crashed, Signal::KILL).unwrap();
    let beta_host = format!("shared-worker.{beta}.localhost");
    let (status, body) =
        request_http(address, "GET", &beta_host, "beta-deployer", "/provider").await;
    assert_eq!(status, 200, "B failed after A Provider crashed: {body}");
    assert!(body.contains("beta-provider"));
    let beta_lease: serde_json::Value = serde_json::from_slice(
        &fs::read(data_b.join("runtime/extensions/local-files/provider.lease")).unwrap(),
    )
    .unwrap();
    assert_eq!(beta_lease["pid"], pids[1]);
    let alpha_host = format!("shared-worker.{alpha}.localhost");
    let (status, _) =
        request_http(address, "GET", &alpha_host, "alpha-deployer", "/provider").await;
    assert_ne!(status, 200, "crashed Provider call was silently replayed");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        let (status, body) =
            request_http(address, "GET", &alpha_host, "alpha-deployer", "/provider").await;
        if status == 200 {
            assert!(body.contains("alpha-provider"));
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "A Provider did not recover"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let alpha_lease: serde_json::Value = serde_json::from_slice(
        &fs::read(data_a.join("runtime/extensions/local-files/provider.lease")).unwrap(),
    )
    .unwrap();
    assert_ne!(alpha_lease["pid"], pids[0]);
    (pids[0], pids[1])
}

fn initialized_local_instance(root: &Path, name: &str) -> (PathBuf, PathBuf) {
    let data = root.join(format!("{name}-data"));
    let config_path = root.join(format!("{name}.toml"));
    let mut config = PlatformConfig::local_test_config();
    config.instance.name = Some(name.parse().unwrap());
    config.data.path = data.clone();
    config.data.master_key_file = data.join("keys/master.key");
    config.object_storage = ObjectStorageConfig::Local(LocalObjectStorageConfig {
        path: data.join("objects"),
        ..LocalObjectStorageConfig::default()
    });
    config.runtime.shutdown_grace_ms = 1_000;
    config.runtime.drain_timeout_ms = 1_000;
    config.runtime.kill_timeout_ms = 1_000;
    drop(PlatformStorage::bootstrap(&config.data, &open_compute_core::SystemClock).unwrap());
    let deployer = data.join("keys/deployer.token");
    let read_only = data.join("keys/read-only.token");
    for (path, value) in [
        (&deployer, format!("{name}-deployer")),
        (&read_only, format!("{name}-read-only")),
    ] {
        fs::write(path, value).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    config.auth.deployer_auth = SecretReference {
        env: None,
        file: Some(deployer),
    };
    config.auth.read_only_auth = SecretReference {
        env: None,
        file: Some(read_only),
    };
    fs::write(&config_path, toml::to_string(&config).unwrap()).unwrap();
    fs::set_permissions(&config_path, fs::Permissions::from_mode(0o600)).unwrap();
    (config_path, data)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_daemon_starts_two_isolated_instance_children() {
    let root = Evidence::new();
    let _package_root = package_scope::UserRoot::reserve(root.path());
    let binary = isolated_binary(root.path());
    fs::create_dir(root.path().join("home")).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let https = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let https_addr = https.local_addr().unwrap();
    drop(https);
    let challenge_tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let challenge_addr = challenge_tcp.local_addr().unwrap();
    let challenge_udp = std::net::UdpSocket::bind(challenge_addr).unwrap();
    drop((challenge_tcp, challenge_udp));
    let registry = package_scope::registry(root.path());
    let ocd_root = registry.root_for(ServiceScope::User).to_path_buf();
    fs::create_dir_all(ocd_root.join("keys")).unwrap();
    let admin_token = ocd_root.join("keys/admin.token");
    fs::write(&admin_token, b"shared-admin-token").unwrap();
    fs::set_permissions(&admin_token, fs::Permissions::from_mode(0o600)).unwrap();
    let manifest = ocd_root.join("ocd.toml");
    let operator_caddyfile = ocd_root.join("operator.caddyfile");
    let valid_operator_caddyfile = "(operator-placeholder) {\n  respond \"ok\"\n}\n";
    fs::write(&operator_caddyfile, valid_operator_caddyfile).unwrap();
    fs::set_permissions(&operator_caddyfile, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(
        &manifest,
        format!(
            "[server]\npublic_bind = \"{address}\"\nadmin_auth = {{ file = \"./keys/admin.token\" }}\n"
        ),
    )
    .unwrap();
    fs::set_permissions(&manifest, fs::Permissions::from_mode(0o600)).unwrap();
    let source = fs::read_to_string(&manifest).unwrap();
    fs::write(
        &manifest,
        format!(
            "{source}\n[gateway]\ningress_ipv4 = [\"203.0.113.10\"]\nhttps_listen = \"{https_addr}\"\nchallenge_dns_listen = \"{challenge_addr}\"\ncaddy = [{{ caddy_file = \"./operator.caddyfile\" }}]\n"
        ),
    )
    .unwrap();
    let (config_a, data_a) = initialized_local_instance(root.path(), "alpha");
    let (config_b, data_b) = initialized_local_instance(root.path(), "beta");
    let extension = shared_test_extension(root.path());
    for (config, domain) in [
        (&config_a, "alpha.example.com"),
        (&config_b, "beta.example.net"),
    ] {
        let mut instance: PlatformConfig =
            toml::from_str(&fs::read_to_string(config).unwrap()).unwrap();
        instance.extensions.insert(
            "local-files".to_owned(),
            LocalExtensionConfig {
                path: extension.clone(),
            },
        );
        let source = toml::to_string(&instance).unwrap();
        fs::write(
            config,
            format!("{source}\n[public_gateway]\nbase_domain = \"{domain}\"\n"),
        )
        .unwrap();
    }
    let a = registry
        .register(&config_a, ServiceScope::User, SystemTime::now())
        .unwrap();
    let b = registry
        .register(&config_b, ServiceScope::User, SystemTime::now())
        .unwrap();
    assert_ne!(a.instance_id, b.instance_id);
    let beta_deployer = data_b.join("keys/deployer.token");
    fs::write(&beta_deployer, b"alpha-deployer").unwrap();
    let duplicate = command(&binary).arg("run").output().unwrap();
    assert!(!duplicate.status.success());
    assert!(
        String::from_utf8_lossy(&duplicate.stderr).contains("SECRET_REF_INVALID"),
        "{}",
        String::from_utf8_lossy(&duplicate.stderr)
    );
    fs::write(&beta_deployer, b"shared-admin-token").unwrap();
    let duplicate = command(&binary).arg("run").output().unwrap();
    assert!(!duplicate.status.success());
    assert!(String::from_utf8_lossy(&duplicate.stderr).contains("SECRET_REF_INVALID"));
    fs::write(&beta_deployer, b"beta-deployer").unwrap();
    let (config_c, data_c) = initialized_local_instance(
        &registry.root_for(ServiceScope::User).join("instances"),
        "gamma",
    );
    let source = fs::read_to_string(&config_c).unwrap();
    fs::write(
        &config_c,
        format!("{source}\n[public_gateway]\nbase_domain = \"gamma.example.org\"\n"),
    )
    .unwrap();
    let config_c_before = fs::read(&config_c).unwrap();
    assert_eq!(registry.list_scope(ServiceScope::User).unwrap().len(), 2);
    let cache_a = data_a
        .join("cache/artifacts/sha256/ab")
        .join("ab".repeat(31));
    let cache_b = data_b
        .join("cache/artifacts/sha256/cd")
        .join("cd".repeat(31));
    fs::create_dir_all(cache_a.parent().unwrap()).unwrap();
    fs::create_dir_all(cache_b.parent().unwrap()).unwrap();
    fs::write(&cache_a, b"alpha-cache").unwrap();
    fs::write(&cache_b, b"beta-cache").unwrap();
    let log = root.path().join("multi-instance-stderr.log");
    let mut process = Process::spawn(&binary, &data_a, &log).with_instance(&data_b);
    process.ready(address).await;
    assert!(!data_a.join("runtime/packages").exists());
    assert!(!data_b.join("runtime/packages").exists());
    let managed = fs::read_to_string(
        registry
            .root_for(ServiceScope::User)
            .join("gateway/managed.caddyfile"),
    )
    .unwrap();
    assert_eq!(managed.matches("    admin ").count(), 1);
    assert!(managed.contains("https://*.alpha.example.com"));
    assert!(managed.contains("https://*.beta.example.net"));
    assert!(!data_a.join("gateway").exists());
    assert!(!data_b.join("gateway").exists());
    let shared_caddy_status = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let status =
                String::from_utf8(successful(&binary, &["caddy", "status"]).stdout).unwrap();
            if !status.contains("child_pid=-") {
                break status;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("shared Caddy child readiness");
    let child_pid = |status: &str| {
        status
            .split("child_pid=")
            .nth(1)
            .and_then(|tail| tail.split_whitespace().next())
            .unwrap()
            .to_owned()
    };
    let caddy_pid = child_pid(&shared_caddy_status);
    assert!(caddy_pid.parse::<u32>().is_ok_and(|pid| pid > 1));
    let preview = successful(
        &binary,
        &["--instance", &a.instance_id, "cache", "clean", "--dry-run"],
    );
    assert!(String::from_utf8_lossy(&preview.stdout).contains("bytes=11"));
    assert!(cache_a.exists() && cache_b.exists());
    successful(&binary, &["--instance", &a.instance_id, "cache", "clean"]);
    assert!(!cache_a.exists() && cache_b.exists());
    successful(&binary, &["cache", "clean", "--all"]);
    assert!(!cache_b.exists());
    assert!(
        registry
            .root_for(ServiceScope::User)
            .join("cache/packages")
            .join(embedded_payload_sha256())
            .exists()
    );
    let lease_a: serde_json::Value =
        serde_json::from_slice(&fs::read(data_a.join("runtime/child.lease")).unwrap()).unwrap();
    let lease_b: serde_json::Value =
        serde_json::from_slice(&fs::read(data_b.join("runtime/child.lease")).unwrap()).unwrap();
    assert_ne!(lease_a["pid"], lease_b["pid"]);
    assert!(lease_a["pid"].as_i64().is_some_and(|pid| pid > 1));
    assert!(lease_b["pid"].as_i64().is_some_and(|pid| pid > 1));
    assert!(process.child.try_wait().unwrap().is_none());
    let status = "/client/v4/open-compute/system/status";
    let host_a = format!("{}.localhost", a.instance_id);
    let host_b = format!("{}.localhost", b.instance_id);
    assert_eq!(
        request_status(address, &host_a, "alpha-deployer", status).await,
        200
    );
    assert_eq!(
        request_status(address, &host_b, "beta-deployer", status).await,
        200
    );
    fs::write(&operator_caddyfile, "invalid {\n").unwrap();
    let failed_reload = command(&binary).args(["caddy", "reload"]).output().unwrap();
    assert!(!failed_reload.status.success());
    let failed_status =
        String::from_utf8(successful(&binary, &["caddy", "status"]).stdout).unwrap();
    assert_eq!(child_pid(&failed_status), caddy_pid);
    assert!(
        failed_status.contains("last_reload=failed"),
        "{failed_status}"
    );
    assert_eq!(
        fs::read_to_string(
            registry
                .root_for(ServiceScope::User)
                .join("gateway/managed.caddyfile")
        )
        .unwrap(),
        managed
    );
    assert_eq!(
        request_status(address, &host_a, "alpha-deployer", status).await,
        200
    );
    assert_eq!(
        request_status(address, &host_b, "beta-deployer", status).await,
        200
    );
    fs::write(&operator_caddyfile, valid_operator_caddyfile).unwrap();
    successful(&binary, &["caddy", "reload"]);
    assert_eq!(
        request_status(address, &host_a, "shared-admin-token", status).await,
        200
    );
    assert_eq!(
        request_status(address, &host_b, "shared-admin-token", status).await,
        200
    );
    assert_ne!(
        request_status(address, &host_a, "beta-deployer", status).await,
        200
    );
    assert_ne!(
        request_status(address, &host_b, "alpha-deployer", status).await,
        200
    );
    let session_path = "/operator/session";
    let mut sessions = Vec::new();
    for host in [&host_a, &host_b] {
        let (status, body) = request_http_body_with_headers(
            address,
            "POST",
            host,
            "shared-admin-token",
            session_path,
            "{}",
            "Sec-Fetch-Site: same-origin\r\n",
        )
        .await;
        assert_eq!(status, 200, "{body}");
        sessions.push(
            serde_json::from_str::<serde_json::Value>(&body).unwrap()["session_token"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
    }
    assert_eq!(
        request_status(address, &host_a, &sessions[0], status).await,
        200
    );
    assert_eq!(
        request_status(address, &host_b, &sessions[1], status).await,
        200
    );
    assert_eq!(
        request_status(address, &host_a, &sessions[1], status).await,
        200
    );
    assert_eq!(
        request_status(address, &host_b, &sessions[0], status).await,
        200
    );
    assert_ne!(
        request_status(
            address,
            &format!("app.{host_a}"),
            "shared-admin-token",
            status
        )
        .await,
        200
    );
    let (_, initial_list) = request_http(
        address,
        "GET",
        "127.0.0.1",
        "shared-admin-token",
        "/operator/api/instances",
    )
    .await;
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&initial_list).unwrap()["instances"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let (http_status, accounts) = request_http(
        address,
        "GET",
        "127.0.0.1",
        "shared-admin-token",
        "/client/v4/accounts",
    )
    .await;
    assert_eq!(http_status, 200);
    let accounts: serde_json::Value = serde_json::from_str(&accounts).unwrap();
    let discovered = accounts["result"].as_array().unwrap();
    assert_eq!(discovered.len(), 2);
    assert!(discovered.iter().any(|item| item["id"] == a.instance_id));
    assert!(discovered.iter().any(|item| item["id"] == b.instance_id));
    let (http_status, accounts) = request_http(
        address,
        "GET",
        "127.0.0.1",
        "alpha-deployer",
        "/client/v4/accounts",
    )
    .await;
    assert_eq!(http_status, 200);
    let accounts: serde_json::Value = serde_json::from_str(&accounts).unwrap();
    assert_eq!(accounts["result"].as_array().unwrap().len(), 1);
    assert_eq!(accounts["result"][0]["id"], a.instance_id);
    assert_named_resource_isolation(address, &a.instance_id, &b.instance_id).await;
    assert_asset_upload_token_isolation(address, &a.instance_id, &b.instance_id).await;
    assert_git_isolation(address, &a.instance_id, &b.instance_id).await;
    assert_shared_worker_isolation(address, &a.instance_id, &b.instance_id).await;
    assert_signed_tail_isolation(address, &a.instance_id, &b.instance_id).await;
    let (_, beta_provider_pid) =
        assert_provider_isolation(address, &a.instance_id, &b.instance_id, &data_a, &data_b).await;
    assert_stale_provider_ack_is_scoped(address, &a.instance_id, &b.instance_id, &data_a, &data_b)
        .await;
    let cli_list: serde_json::Value =
        serde_json::from_slice(&successful(&binary, &["instances", "--json"]).stdout).unwrap();
    let listed = cli_list["instances"].as_array().unwrap();
    assert_eq!(listed.len(), 2);
    assert!(listed.iter().all(|instance| instance["state"] == "running"));
    let daemon_status: serde_json::Value =
        serde_json::from_slice(&successful(&binary, &["status", "--json"]).stdout).unwrap();
    assert_eq!(daemon_status["state"], "running");
    let manifest_before_add = fs::read(&manifest).unwrap();
    fs::write(data_c.join("keys/deployer.token"), b"alpha-deployer").unwrap();
    let duplicate_token_add = command(&binary)
        .args(["instance", "add", "--config", config_c.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!duplicate_token_add.status.success());
    assert!(String::from_utf8_lossy(&duplicate_token_add.stderr).contains("SECRET_REF_INVALID"));
    assert_eq!(fs::read(&manifest).unwrap(), manifest_before_add);
    fs::write(data_c.join("keys/deployer.token"), b"gamma-deployer").unwrap();
    let cli_add = successful(
        &binary,
        &["instance", "add", "--config", config_c.to_str().unwrap()],
    );
    assert!(!cli_add.stdout.is_empty());
    assert!(
        fs::read_to_string(
            registry
                .root_for(ServiceScope::User)
                .join("gateway/managed.caddyfile")
        )
        .unwrap()
        .contains("https://*.gamma.example.org")
    );
    let gamma = registry
        .list_scope(ServiceScope::User)
        .unwrap()
        .into_iter()
        .find(|record| record.name.as_deref() == Some("gamma"))
        .unwrap();
    let host_c = format!("{}.localhost", gamma.instance_id);
    assert_eq!(
        request_status(address, &host_c, "gamma-deployer", status).await,
        200
    );
    assert!(data_c.join("runtime/child.lease").exists());
    assert_ne!(
        request_status(address, &host_b, "gamma-deployer", status).await,
        200
    );
    let mut control = std::os::unix::net::UnixStream::connect(
        registry
            .root_for(ServiceScope::User)
            .join("run/control.sock"),
    )
    .unwrap();
    std::io::Write::write_all(
        &mut control,
        format!(
            "{{\"op\":\"remove\",\"instance_id\":\"{}\"}}\n",
            gamma.instance_id
        )
        .as_bytes(),
    )
    .unwrap();
    let mut denied = Vec::new();
    std::io::Read::read_to_end(&mut control, &mut denied).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&denied).unwrap()["error"],
        "INSTANCE_REGISTRY_INVALID"
    );
    assert!(data_c.join("runtime/child.lease").exists());
    assert!(
        !command(&binary)
            .args(["instance", "add", "--config", config_c.to_str().unwrap()])
            .output()
            .unwrap()
            .status
            .success()
    );
    assert_eq!(
        request_http(
            address,
            "POST",
            "127.0.0.1",
            "shared-admin-token",
            &format!("/operator/api/instances/{}/remove", gamma.instance_id),
        )
        .await
        .0,
        404
    );
    let cli_remove = successful(&binary, &["instance", "remove", "gamma"]);
    assert!(String::from_utf8_lossy(&cli_remove.stdout).contains(&gamma.instance_id));
    assert!(
        !fs::read_to_string(
            registry
                .root_for(ServiceScope::User)
                .join("gateway/managed.caddyfile")
        )
        .unwrap()
        .contains("gamma.example.org")
    );
    assert!(!data_c.join("runtime/child.lease").exists());
    assert!(data_c.join("control.sqlite").exists());
    assert_eq!(fs::read(&config_c).unwrap(), config_c_before);
    assert_eq!(registry.list_scope(ServiceScope::User).unwrap().len(), 2);
    assert_ne!(
        request_status(
            address,
            "127.0.0.1",
            "gamma-deployer",
            "/client/v4/accounts",
        )
        .await,
        200
    );
    let created_config = registry
        .root_for(ServiceScope::User)
        .join("instances/delta/compute.toml");
    let created_data = root.path().join("delta-external-data");
    let config_arg = created_config.strip_prefix("/").unwrap().to_str().unwrap();
    let data_arg = created_data.strip_prefix("/").unwrap().to_str().unwrap();
    let setup_args = [
        "instance",
        "setup",
        "--name",
        "delta",
        "--config",
        config_arg,
        "--data-dir",
        data_arg,
        "--autostart=false",
        "--start=false",
    ];
    let non_interactive = command(&binary)
        .current_dir("/")
        .args(setup_args)
        .output()
        .unwrap();
    assert!(!non_interactive.status.success());
    assert!(String::from_utf8_lossy(&non_interactive.stderr).contains("CONFIG_INVALID"));
    assert!(!created_config.exists() && !created_data.exists());
    let created = command(&binary)
        .current_dir("/")
        .args(setup_args)
        .arg("--yes")
        .output()
        .unwrap();
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    assert!(String::from_utf8_lossy(&created.stdout).contains("INSTANCE_CREATED"));
    let delta = registry
        .list_scope(ServiceScope::User)
        .unwrap()
        .into_iter()
        .find(|record| record.name.as_deref() == Some("delta"))
        .unwrap();
    assert!(!delta.autostart);
    assert_eq!(
        delta.data_path,
        created_data.canonicalize().unwrap().to_string_lossy()
    );
    assert!(created_data.join("control.sqlite").exists());
    assert!(!created_config.parent().unwrap().join("data").exists());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(
            &successful(&binary, &["instances", "--json"]).stdout
        )
        .unwrap()["instances"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["instance_id"] == delta.instance_id)
            .unwrap()["state"],
        "stopped"
    );
    successful(&binary, &["instance", "remove", "delta"]);
    assert!(created_config.exists() && created_data.join("control.sqlite").exists());
    assert_eq!(registry.list_scope(ServiceScope::User).unwrap().len(), 2);
    let original_manifest = fs::read(&manifest).unwrap();
    let mut manual_manifest = original_manifest.clone();
    manual_manifest.extend_from_slice(b"\n# manual edit while daemon is running\n");
    fs::write(&manifest, &manual_manifest).unwrap();
    let conflicted_add = command(&binary)
        .args(["instance", "add", "--config", config_c.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!conflicted_add.status.success());
    assert!(String::from_utf8_lossy(&conflicted_add.stderr).contains("INSTANCE_REGISTRY_INVALID"));
    assert_eq!(fs::read(&manifest).unwrap(), manual_manifest);
    fs::write(&manifest, original_manifest).unwrap();
    assert_eq!(
        request_status(address, &host_a, "alpha-deployer", status).await,
        200
    );
    assert_eq!(
        request_status(address, &host_b, "beta-deployer", status).await,
        200
    );
    request_shutdown(
        &registry
            .root_for(ServiceScope::User)
            .join("run")
            .join(&a.instance_id),
    )
    .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while data_a.join("runtime/child.lease").exists()
        || data_a
            .join("runtime/extensions/local-files/provider.lease")
            .exists()
    {
        assert!(tokio::time::Instant::now() < deadline, "A did not stop");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let beta_provider: serde_json::Value = serde_json::from_slice(
        &fs::read(data_b.join("runtime/extensions/local-files/provider.lease")).unwrap(),
    )
    .unwrap();
    assert_eq!(beta_provider["pid"], beta_provider_pid);
    let (provider_status, provider_body) = request_http(
        address,
        "GET",
        &format!("shared-worker.{}.localhost", b.instance_id),
        "beta-deployer",
        "/provider",
    )
    .await;
    assert_eq!(provider_status, 200, "{provider_body}");
    assert!(provider_body.contains("beta-provider"), "{provider_body}");
    assert_ne!(
        request_status(address, &host_a, "shared-admin-token", status).await,
        200
    );
    assert_eq!(
        request_status(address, &host_b, "beta-deployer", status).await,
        200
    );
    let still_running_b: serde_json::Value =
        serde_json::from_slice(&fs::read(data_b.join("runtime/child.lease")).unwrap()).unwrap();
    assert_eq!(still_running_b["pid"], lease_b["pid"]);
    assert!(process.child.try_wait().unwrap().is_none());
    let list_path = "/operator/api/instances";
    assert_eq!(
        request_status(address, "127.0.0.1", "alpha-deployer", list_path).await,
        401
    );
    assert_eq!(
        request_status(
            address,
            &format!("app.{host_b}"),
            "shared-admin-token",
            list_path
        )
        .await,
        404
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let (list_status, body) =
            request_http(address, "GET", "127.0.0.1", "shared-admin-token", list_path).await;
        assert_eq!(list_status, 200);
        let listing: serde_json::Value = serde_json::from_str(&body).unwrap();
        let instances = listing["instances"].as_array().unwrap();
        assert_eq!(instances.len(), 2);
        if instances.iter().any(|instance| {
            instance["instance_id"] == a.instance_id && instance["state"] == "stopped"
        }) {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "A did not reach stopped state"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    for token in ["alpha-deployer", "alpha-read-only"] {
        let (status_code, body) =
            request_http(address, "GET", "127.0.0.1", token, "/client/v4/accounts").await;
        assert_eq!(status_code, 200);
        let accounts: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(accounts["result"].as_array().unwrap().len(), 1);
        assert_eq!(accounts["result"][0]["id"], a.instance_id);
    }
    let (status_code, body) = request_http(
        address,
        "GET",
        "127.0.0.1",
        "alpha-read-only",
        "/client/v4/memberships",
    )
    .await;
    assert_eq!(status_code, 200);
    let memberships: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(memberships["result"][0]["account"]["id"], a.instance_id);
    let start_a = format!("/operator/api/instances/{}/start", a.instance_id);
    assert_eq!(
        request_http(address, "POST", "127.0.0.1", "shared-admin-token", &start_a)
            .await
            .0,
        202
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        if data_a.join("runtime/child.lease").exists()
            && request_status(address, &host_a, "alpha-deployer", status).await == 200
        {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "A did not restart");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let restarted_a: serde_json::Value =
        serde_json::from_slice(&fs::read(data_a.join("runtime/child.lease")).unwrap()).unwrap();
    assert_ne!(restarted_a["pid"], lease_a["pid"]);
    assert_eq!(
        request_http(address, "POST", "127.0.0.1", "alpha-deployer", &start_a)
            .await
            .0,
        401
    );
    let stop_a = format!("/operator/api/instances/{}/stop", a.instance_id);
    assert_eq!(
        request_http(address, "POST", "127.0.0.1", "shared-admin-token", &stop_a)
            .await
            .0,
        202
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while data_a.join("runtime/child.lease").exists() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "A did not stop again"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let current_caddy_status =
        String::from_utf8(successful(&binary, &["caddy", "status"]).stdout).unwrap();
    assert_eq!(child_pid(&current_caddy_status), caddy_pid);
    assert_eq!(
        request_status(address, &host_b, "beta-deployer", status).await,
        200
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let (_, body) =
            request_http(address, "GET", "127.0.0.1", "shared-admin-token", list_path).await;
        let listing: serde_json::Value = serde_json::from_str(&body).unwrap();
        if listing["instances"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["instance_id"] == a.instance_id && entry["state"] == "stopped")
        {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "A did not stop");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let cli_start = successful(&binary, &["instance", "start", "alpha"]);
    assert!(String::from_utf8_lossy(&cli_start.stdout).contains(&a.instance_id));
    let cli_restart = successful(&binary, &["instance", "restart", &a.instance_id]);
    assert!(String::from_utf8_lossy(&cli_restart.stdout).contains(&a.instance_id));
    let cli_stop = successful(&binary, &["instance", "stop", "alpha"]);
    assert!(String::from_utf8_lossy(&cli_stop.stdout).contains(&a.instance_id));
    assert_conflicting_data_rejected_on_start(address, &config_a, &data_b, &a.instance_id).await;
    assert_eq!(
        request_status(address, &host_b, "beta-deployer", status).await,
        200
    );
    assert_ne!(
        request_status(address, &host_a, "alpha-deployer", status).await,
        200
    );
    let entries = registry.list_scope(ServiceScope::User).unwrap();
    assert!(entries.iter().all(|entry| entry.autostart));
    let pty_config = root.path().join("pty.toml");
    let pty_data = root.path().join("pty-data");
    let before_pty = fs::read(&manifest).unwrap();
    let cancelled = interactive_instance_setup(&binary, &pty_config, &pty_data, false).await;
    assert!(!cancelled.status.success());
    assert_eq!(fs::read(&manifest).unwrap(), before_pty);
    assert!(!pty_config.exists() && !pty_data.exists());
    let created = interactive_instance_setup(&binary, &pty_config, &pty_data, true).await;
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    let pty: PlatformConfig = toml::from_str(&fs::read_to_string(&pty_config).unwrap()).unwrap();
    assert_eq!(pty.data.path, pty_data.canonicalize().unwrap());
    let preview = String::from_utf8_lossy(&created.stdout);
    assert!(preview.contains(&format!(
        "config={}",
        pty_config.canonicalize().unwrap().display()
    )));
    assert!(preview.contains(&format!("data_dir={}", pty.data.path.display())));
    assert!(preview.contains("autostart=false start=false"));
    assert!(pty_data.join("control.sqlite").exists());
    assert!(
        registry
            .list_scope(ServiceScope::User)
            .unwrap()
            .iter()
            .any(|entry| entry.name.as_deref() == Some("pty") && !entry.autostart)
    );
    let mut process = restart_two_instances_for_crash(process, &data_a, &data_b, address).await;
    kill_process(
        Pid::from_raw(process.child.id() as i32).unwrap(),
        Signal::KILL,
    )
    .unwrap();
    assert!(!process.child.wait().unwrap().success());
    assert!(data_b.join("runtime/child.lease").exists());
    let mut manifest_value: toml::Value =
        toml::from_str(&fs::read_to_string(&manifest).unwrap()).unwrap();
    let registrations = manifest_value["instances"].as_array_mut().unwrap();
    let alpha_config = config_a.canonicalize().unwrap();
    let alpha_registration = registrations
        .iter_mut()
        .find(|entry| entry["config"].as_str() == alpha_config.to_str())
        .unwrap();
    alpha_registration["autostart"] = toml::Value::Boolean(false);
    fs::write(&manifest, toml::to_string_pretty(&manifest_value).unwrap()).unwrap();
    let mut recovered = Process::spawn(&binary, &data_b, &log);
    recovered.ready(address).await;
    assert_ne!(
        request_status(address, &host_a, "alpha-deployer", status).await,
        200
    );
    assert_eq!(
        request_status(address, &host_b, "beta-deployer", status).await,
        200
    );
    let recovered_b: serde_json::Value =
        serde_json::from_slice(&fs::read(data_b.join("runtime/child.lease")).unwrap()).unwrap();
    assert_ne!(recovered_b["pid"], still_running_b["pid"]);
    let (provider_status, provider_body) = request_http(
        address,
        "GET",
        &format!("shared-worker.{}.localhost", b.instance_id),
        "beta-deployer",
        "/provider",
    )
    .await;
    assert_eq!(provider_status, 200, "{provider_body}");
    assert!(provider_body.contains("beta-provider"), "{provider_body}");
    let recovered_provider: serde_json::Value = serde_json::from_slice(
        &fs::read(data_b.join("runtime/extensions/local-files/provider.lease")).unwrap(),
    )
    .unwrap();
    assert_ne!(recovered_provider["pid"], beta_provider_pid);
    assert!(!data_a.join("runtime/child.lease").exists());
    recovered.stop().await;
    let gateway_storage = registry
        .root_for(ServiceScope::User)
        .join("gateway/storage");
    fs::remove_dir_all(&gateway_storage).unwrap();
    let mut failed = Process::spawn(&binary, &data_a, &log).with_instance(&data_b);
    let exit = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(exit) = failed.child.try_wait().unwrap() {
                break exit;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("Gateway state loss must reject startup");
    assert!(!exit.success());
    assert!(
        fs::read_to_string(&log)
            .unwrap()
            .contains("Gateway ACME storage is missing or incomplete")
    );
    assert!(!gateway_storage.exists());
}

async fn restart_two_instances_for_crash(
    mut process: Process,
    data_a: &Path,
    data_b: &Path,
    address: SocketAddr,
) -> Process {
    let binary = process.binary.clone();
    let log = process.log.clone();
    process.stop().await;
    drop(process);
    let mut process = Process::spawn(&binary, data_a, &log).with_instance(data_b);
    process.ready(address).await;
    successful(&binary, &["instance", "stop", "alpha"]);
    assert!(!data_a.join("runtime/child.lease").exists());
    assert!(data_b.join("runtime/child.lease").exists());
    process
}

async fn assert_conflicting_data_rejected_on_start(
    address: SocketAddr,
    config_a: &Path,
    data_b: &Path,
    instance_a: &str,
) {
    let original = fs::read(config_a).unwrap();
    let mut changed: toml::Value = toml::from_str(std::str::from_utf8(&original).unwrap()).unwrap();
    changed["data"]["path"] = toml::Value::String(data_b.display().to_string());
    fs::write(config_a, toml::to_string(&changed).unwrap()).unwrap();
    let path = format!("/operator/api/instances/{instance_a}/start");
    assert_eq!(
        request_http(address, "POST", "127.0.0.1", "shared-admin-token", &path)
            .await
            .0,
        409
    );
    fs::write(config_a, original).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn single_file_first_start_restart_orphan_recovery_and_corruption_failure() {
    let root = Evidence::new();
    let _package_root = package_scope::UserRoot::reserve(root.path());
    let binary = isolated_binary(root.path());
    let mock = MockS3::spawn("open-compute").await;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let data = root.path().join("data");
    let config_path = root.path().join("config.toml");
    let key = root.path().join("access-key");
    let secret = root.path().join("secret-key");
    fs::write(&key, "AKIAEXAMPLEKEYID01").unwrap();
    fs::write(&secret, "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY").unwrap();
    for path in [&key, &secret] {
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let admin_token = root.path().join("admin.token");
    let deployer_token = root.path().join("deployer.token");
    let read_only_token = root.path().join("read-only.token");
    fs::write(&admin_token, b"single-binary-admin\n").unwrap();
    fs::write(&deployer_token, b"single-binary-deployer\n").unwrap();
    fs::write(&read_only_token, b"single-binary-read-only\n").unwrap();
    fs::set_permissions(&admin_token, fs::Permissions::from_mode(0o600)).unwrap();
    fs::set_permissions(&deployer_token, fs::Permissions::from_mode(0o600)).unwrap();
    fs::set_permissions(&read_only_token, fs::Permissions::from_mode(0o600)).unwrap();
    let mut config = PlatformConfig::local_test_config();
    config.auth.deployer_auth = SecretReference {
        env: None,
        file: Some(deployer_token),
    };
    config.auth.read_only_auth = SecretReference {
        env: None,
        file: Some(read_only_token),
    };
    config.data.path = data.clone();
    config.data.master_key_file = data.join("keys/master.key");
    config.object_storage = ObjectStorageConfig::S3(S3Config {
        endpoint: mock.endpoint.clone(),
        region: "us-east-1".to_owned(),
        access_key_id_env: None,
        secret_access_key_env: None,
        access_key_id_file: Some(key),
        secret_access_key_file: Some(secret),
        ..S3Config::default()
    });
    config.runtime.shutdown_grace_ms = 1000;
    config.runtime.drain_timeout_ms = 1000;
    config.runtime.kill_timeout_ms = 1000;
    fs::write(&config_path, toml::to_string(&config).unwrap()).unwrap();
    fs::create_dir(root.path().join("home")).unwrap();
    drop(PlatformStorage::bootstrap(&config.data, &open_compute_core::SystemClock).unwrap());
    let registry = package_scope::registry(root.path());
    let ocd_root = registry.root_for(ServiceScope::User).to_path_buf();
    fs::create_dir_all(&ocd_root).unwrap();
    let manifest = ocd_root.join("ocd.toml");
    fs::write(
        &manifest,
        format!(
            "[server]\npublic_bind = \"{address}\"\nadmin_auth = {{ file = {:?} }}\n",
            admin_token.display().to_string()
        ),
    )
    .unwrap();
    fs::set_permissions(&manifest, fs::Permissions::from_mode(0o600)).unwrap();
    let package = ocd_root
        .join("cache/packages")
        .join(embedded_payload_sha256());
    registry
        .register(&config_path, ServiceScope::User, SystemTime::now())
        .unwrap();
    let log = root.path().join("stderr.log");
    let mut process = Process::spawn(&binary, &data, &log);
    process.ready(address).await;
    assert!(package.join("workerd").is_file());
    assert!(package.join("runtime/dist/manifest.json").is_file());
    let modified = fs::metadata(package.join("workerd"))
        .unwrap()
        .modified()
        .unwrap();
    let master_key = fs::read(data.join("keys/master.key")).unwrap();
    let competitor = command(&binary).arg("run").output().unwrap();
    assert!(!competitor.status.success());
    assert!(
        String::from_utf8_lossy(&competitor.stderr).contains("INSTANCE_REGISTRY_INVALID"),
        "{}",
        String::from_utf8_lossy(&competitor.stderr)
    );
    process.stop().await;

    let mut process = Process::spawn(&binary, &data, &log);
    process.ready(address).await;
    assert_eq!(
        fs::metadata(package.join("workerd"))
            .unwrap()
            .modified()
            .unwrap(),
        modified
    );
    assert_eq!(fs::read(data.join("keys/master.key")).unwrap(), master_key);
    // Leave the authenticated child orphan for the next production startup to recover.
    process.child.kill().unwrap();
    process.child.wait().unwrap();
    let mut recovered = Process::spawn(&binary, &data, &log);
    recovered.ready(address).await;
    recovered.stop().await;
    drop(process);
    drop(recovered);
    let observability_path = data.join("observability.sqlite");
    let observability_db = rusqlite::Connection::open(&observability_path).unwrap();
    let instance_id: String = observability_db
        .query_row(
            "SELECT instance_id FROM observability_identity WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    observability_db
        .execute(
            "UPDATE observability_identity SET instance_id=?1 WHERE singleton=1",
            [open_compute_core::InstanceId::generate().to_string()],
        )
        .unwrap();
    drop(observability_db);
    let identity_log = root.path().join("mismatched-observability.log");
    let mut rejected = Process::spawn(&binary, &data, &identity_log);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !fs::read_to_string(&identity_log)
        .unwrap_or_default()
        .contains("PLATFORM_UNAVAILABLE")
    {
        assert!(
            tokio::time::Instant::now() < deadline,
            "identity mismatch was not reported"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(ready_request(address).await.unwrap());
    rejected.stop().await;
    rusqlite::Connection::open(&observability_path)
        .unwrap()
        .execute(
            "UPDATE observability_identity SET instance_id=?1 WHERE singleton=1",
            [instance_id],
        )
        .unwrap();
    let asset = package.join("runtime/config.capnp");
    fs::set_permissions(&asset, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&asset, "corrupt").unwrap();
    let failed_log = root.path().join("failed-startup.log");
    let mut failed_instance = Process::spawn(&binary, &data, &failed_log);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = failed_instance.child.try_wait().unwrap() {
            break status;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "corrupt shared package did not fail startup"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    assert!(!status.success());
    assert!(
        fs::read_to_string(&failed_log)
            .unwrap_or_default()
            .contains("RUNTIME_INVALID"),
        "corrupt shared package must report RUNTIME_INVALID"
    );
    assert!(!ready_request(address).await.unwrap_or(false));
    assert_eq!(
        fs::read(&asset).unwrap(),
        b"corrupt",
        "corrupt cache must not be repaired silently"
    );
    assert_eq!(fs::read_dir(package.parent().unwrap()).unwrap().count(), 1);
    assert!(!data.join("runtime/packages").exists());
    let mut keys = mock.keys();
    keys.sort();
    assert_eq!(
        keys,
        vec![
            "system/authority/v1.json".to_owned(),
            "tenant/r2/authority/v1.json".to_owned(),
        ],
        "startup may retain only the immutable authority markers"
    );
}
