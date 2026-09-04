//! Shared real ocd process ownership and private failure evidence for Workflow Gates.

use axum::body::{Body, to_bytes};
use axum::http::Request;
use rustix::process::{Pid, Signal, kill_process, test_kill_process};
use std::fs;
use std::net::SocketAddr;
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

#[allow(dead_code)] // consumed by p2_exit_gate; other test binaries share this module
pub(crate) const ADMIN_TOKEN: &str = "workflow-admin";

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

async fn admin_response(
    client: &Client,
    address: SocketAddr,
    path: &str,
) -> Result<axum::http::Response<hyper::body::Incoming>, ()> {
    let request = Request::builder()
        .method("GET")
        .uri(format!("http://{address}{path}"))
        .header("host", "workflow.example")
        .header("authorization", format!("Bearer {ADMIN_TOKEN}"))
        .body(Body::empty())
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), client.request(request))
        .await
        .map_err(|_| ())?
        .map_err(|_| ())
}

pub(crate) struct Process(pub(crate) Child, PathBuf, String);
impl Process {
    #[allow(
        dead_code,
        reason = "only process Gates that require graceful exit call this"
    )]
    pub(crate) async fn stop(&mut self) {
        let runtime_pid = lease_pid(&self.1);
        let pid = Pid::from_raw(self.0.id() as i32).expect("ocd PID must be positive");
        kill_process(pid, Signal::TERM).expect("signal ocd for graceful shutdown");
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = self.0.try_wait().expect("wait for ocd shutdown") {
                assert!(status.success(), "ocd graceful shutdown failed: {status}");
                break;
            }
            assert!(Instant::now() < deadline, "ocd graceful shutdown timed out");
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(
            !self.1.exists(),
            "normal shutdown left the workerd child lease"
        );
        assert_pid_gone(runtime_pid, "stock workerd after ocd shutdown");
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        if self.0.try_wait().unwrap().is_none() {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
        // Recover only the formally identified child before failure evidence
        // is retained or successful temporary state is removed.
        if let Err(error) = open_compute_runtime::recover_orphan_for_test(&self.1, &self.2) {
            if std::thread::panicking() {
                eprintln!("Workflow Gate orphan cleanup failed: {}", error.code());
            } else {
                panic!("Workflow Gate orphan cleanup failed: {}", error.code());
            }
        }
    }
}

fn lease_pid(path: &Path) -> i32 {
    let bytes = fs::read(path).expect("ready ocd must own a workerd child lease");
    let lease: serde_json::Value =
        serde_json::from_slice(&bytes).expect("workerd child lease must be valid JSON");
    let pid = lease["pid"]
        .as_i64()
        .and_then(|value| i32::try_from(value).ok())
        .expect("workerd child lease must contain a PID");
    assert!(pid > 1, "workerd child PID must be signal-safe");
    pid
}

fn assert_pid_gone(pid: i32, what: &str) {
    let pid = Pid::from_raw(pid).expect("tracked PID must be positive");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match test_kill_process(pid) {
            Err(error) if error == rustix::io::Errno::SRCH => return,
            Ok(()) => {}
            Err(error) => panic!("failed to inspect {what}: {error}"),
        }
        assert!(Instant::now() < deadline, "{what} is still live");
        std::thread::sleep(Duration::from_millis(20));
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

// This shared test-support module is compiled into gates that need only one listener.
#[allow(dead_code)]
pub(crate) fn distinct_addresses() -> (SocketAddr, SocketAddr) {
    let public = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let admin = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let public_addr = public.local_addr().unwrap();
    let admin_addr = admin.local_addr().unwrap();
    assert_ne!(
        public_addr, admin_addr,
        "public and admin listeners must be distinct"
    );
    (public_addr, admin_addr)
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
        Command::new(env!("CARGO_BIN_EXE_ocd"))
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
            "ocd exited before readiness"
        );
        if response(client, admin, "/health/ready", "GET")
            .await
            .is_ok_and(|r| r.status() == 200)
        {
            return;
        }
        if Instant::now() >= deadline {
            let status = admin_response(client, admin, "/client/v4/open-compute/system/status")
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
    let admin_token = root.join("admin.token");
    let deployer_token = root.join("deployer.token");
    let read_only_token = root.join("read-only.token");
    fs::write(&admin_token, b"workflow-admin\n").unwrap();
    fs::write(&deployer_token, b"workflow-deployer\n").unwrap();
    fs::write(&read_only_token, b"workflow-read-only\n").unwrap();
    fs::set_permissions(&admin_token, fs::Permissions::from_mode(0o600)).unwrap();
    fs::set_permissions(&deployer_token, fs::Permissions::from_mode(0o600)).unwrap();
    fs::set_permissions(&read_only_token, fs::Permissions::from_mode(0o600)).unwrap();
    let config = root.join("process.toml");
    fs::write(
        &config,
        format!(
            r#"
[server]
public_bind = "{public}"
admin_bind = "{admin}"

[server.admin_auth]
file = "{admin_token}"

[server.deployer_auth]
file = "{deployer_token}"

[server.read_only_auth]
file = "{read_only_token}"

[storage]
data_dir = "{data_dir}"
master_key_file = "{master_key_file}"
[s3]
endpoint = "{endpoint}"
region = "us-east-1"
bucket = "open-compute"
prefix = "system/"
force_path_style = true
access_key_id_file = "{access_key}"
secret_access_key_file = "{secret_key}"
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
            public = public,
            admin = admin,
            admin_token = admin_token.display(),
            deployer_token = deployer_token.display(),
            read_only_token = read_only_token.display(),
            data_dir = data.display(),
            master_key_file = data.join("keys/master.key").display(),
            endpoint = endpoint,
            access_key = key.display(),
            secret_key = secret.display(),
        ),
    )
    .unwrap();
    fs::set_permissions(&config, fs::Permissions::from_mode(0o600)).unwrap();
    config
}
