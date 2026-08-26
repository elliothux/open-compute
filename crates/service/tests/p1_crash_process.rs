//! P1 real-process SIGKILL/orphan recovery Gate.

use open_compute_artifacts::MockS3;
use open_compute_core::SystemClock;
use open_compute_service::config_load::load_platform_config;
use open_compute_storage::PlatformStorage;
use std::fs;
use std::fs::OpenOptions;
use std::net::{SocketAddr, TcpListener};
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

struct ChildGuard(Child);

impl ChildGuard {
    fn child(&self) -> &Child {
        &self.0
    }

    fn child_mut(&mut self) -> &mut Child {
        &mut self.0
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.0.try_wait().is_ok_and(|status| status.is_none()) {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace")
        .to_path_buf()
}

fn unused_addr() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("ephemeral listener");
    listener.local_addr().expect("listener address")
}

fn write_mode(path: &Path, bytes: &[u8], mode: u32) {
    fs::write(path, bytes).expect("write fixture");
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("fixture mode");
}

fn write_config(
    root: &Path,
    data_dir: &Path,
    mock: &MockS3,
    workerd: &Path,
    public: SocketAddr,
    admin: SocketAddr,
) -> PathBuf {
    let access_key = root.join("access-key");
    let secret_key = root.join("secret-key");
    write_mode(&access_key, b"AKIAP1CRASHPROCESS1", 0o600);
    write_mode(
        &secret_key,
        b"wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
        0o600,
    );
    let workspace = repo_root();
    let config = root.join("platform.toml");
    fs::write(
        &config,
        format!(
            r#"
[server]
public_bind = "{public}"
admin_bind = "{admin}"

[storage]
data_dir = "{data_dir}"
master_key_file = "{master_key}"
free_space_soft_bytes = 134217728
free_space_hard_bytes = 67108864

[s3]
endpoint = "{endpoint}"
region = "us-east-1"
bucket = "open-compute"
force_path_style = true
access_key_id_file = "{access_key}"
secret_access_key_file = "{secret_key}"
prefix = "system/"
max_retries = 1
retry_backoff_ms = 10
connect_timeout_ms = 500
request_timeout_ms = 2000

[runtime]
binary = "{workerd}"
lock_file = "{lock}"
assets_dir = "{assets}"
startup_timeout_ms = 20000
shutdown_grace_ms = 2000
kill_timeout_ms = 1000

[hardening]
emergency_reserve_bytes = 16777216

[metrics]
enabled = true
max_label_value_bytes = 64
max_series = 512
"#,
            data_dir = data_dir.display(),
            master_key = data_dir.join("keys/master.key").display(),
            endpoint = mock.endpoint,
            access_key = access_key.display(),
            secret_key = secret_key.display(),
            workerd = workerd.display(),
            lock = workspace.join("runtime/workerd.lock.json").display(),
            assets = workspace.join("runtime").display(),
        ),
    )
    .expect("config");
    config
}

fn spawn_platformd(config: &Path, log: &Path) -> Child {
    let stderr = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(log)
        .expect("open bounded process log");
    Command::new(env!("CARGO_BIN_EXE_platformd"))
        .args(["run", "--config"])
        .arg(config)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr))
        .spawn()
        .expect("spawn platformd")
}

fn signal(child: &Child, name: &str) {
    let status = Command::new("/bin/kill")
        .args([name, &child.id().to_string()])
        .status()
        .expect("signal platformd");
    assert!(status.success(), "signal {name} failed");
}

async fn wait_ready(address: SocketAddr, child: &mut Child) {
    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        assert!(
            child.try_wait().expect("child state").is_none(),
            "platformd exited"
        );
        if let Ok(Ok(mut stream)) = tokio::time::timeout(
            Duration::from_millis(500),
            tokio::net::TcpStream::connect(address),
        )
        .await
        {
            let request = format!(
                "GET /health/ready HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n"
            );
            if stream.write_all(request.as_bytes()).await.is_ok() {
                let mut response = Vec::new();
                if tokio::time::timeout(Duration::from_secs(1), stream.read_to_end(&mut response))
                    .await
                    .is_ok_and(|result| result.is_ok())
                    && response.starts_with(b"HTTP/1.1 200")
                {
                    return;
                }
            }
        }
        assert!(Instant::now() < deadline, "platformd did not become ready");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn wait_exit(child: &mut Child, timeout: Duration) -> std::process::ExitStatus {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().expect("child state") {
            return status;
        }
        assert!(Instant::now() < deadline, "platformd did not exit");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn p1_platformd_sigkill_reclaims_orphan_and_restarts_cleanly() {
    let workerd = std::env::var_os("OPEN_COMPUTE_TEST_WORKERD")
        .map(PathBuf::from)
        .expect("OPEN_COMPUTE_TEST_WORKERD");
    assert!(workerd.is_file(), "stock workerd is missing");
    let temp = TempDir::new().expect("temp");
    let root = fs::canonicalize(temp.path()).expect("canonical temp");
    let data_dir = root.join("data");
    let mock = MockS3::spawn("open-compute").await;
    let public = unused_addr();
    let admin = unused_addr();
    let config = write_config(&root, &data_dir, &mock, &workerd, public, admin);
    let process_log = root.join("platformd.log");
    let loaded = load_platform_config(&config).expect("load config");
    drop(
        PlatformStorage::bootstrap(&loaded.config.storage, &SystemClock)
            .expect("initialize platform authority"),
    );

    let mut first = ChildGuard(spawn_platformd(&config, &process_log));
    wait_ready(admin, first.child_mut()).await;
    signal(first.child(), "-KILL");
    let first_status = wait_exit(first.child_mut(), Duration::from_secs(5)).await;
    assert!(!first_status.success());

    let mut second = ChildGuard(spawn_platformd(&config, &process_log));
    wait_ready(admin, second.child_mut()).await;
    signal(second.child(), "-TERM");
    let second_status = wait_exit(second.child_mut(), Duration::from_secs(20)).await;
    assert!(
        second_status.success(),
        "graceful restart exit: {second_status}"
    );

    let storage = PlatformStorage::bootstrap(&loaded.config.storage, &SystemClock)
        .expect("reacquire data-dir and verify control SQLite");
    storage.db().quick_check().expect("control quick_check");
    let logs = fs::read(&process_log).expect("process logs");
    for canary in [
        b"AKIAP1CRASHPROCESS1".as_slice(),
        b"wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".as_slice(),
        mock.endpoint.as_bytes(),
        data_dir.as_os_str().as_encoded_bytes(),
    ] {
        assert!(
            !logs.windows(canary.len()).any(|window| window == canary),
            "process log leaked a secret or topology canary"
        );
    }
}
