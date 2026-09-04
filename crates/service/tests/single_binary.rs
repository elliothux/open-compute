//! The actual executable must work alone, with no checkout, PATH tools, or external runtime.

use open_compute_artifacts::MockS3;
use open_compute_core::PlatformConfig;
use open_compute_core::config::SecretReference;
use open_compute_runtime::{
    embedded_payload_sha256, embedded_runtime_lock, recover_orphan_for_test,
};
use rustix::process::{Pid, Signal, kill_process};
use std::fs;
use std::net::SocketAddr;
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::Duration;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

struct Evidence(Option<TempDir>);

impl Evidence {
    fn new() -> Self {
        let runs = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.temp/single-binary-run");
        fs::create_dir_all(&runs).unwrap();
        let runs = runs.canonicalize().unwrap();
        Self(Some(
            tempfile::Builder::new()
                .prefix("single-")
                .tempdir_in(runs)
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
            let failed = path.parent().unwrap().join("failed");
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
        // Keep the test's OS temporary root so panic cleanup inspects the same staging root.
        .env("TMPDIR", std::env::temp_dir());
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
        vec!["capabilities", "--json"],
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
    assert_eq!(config.storage.data_dir, data);
    assert_eq!(config.storage.master_key_file, data.join("keys/master.key"));
    assert_eq!(
        fs::read_dir(root.path()).unwrap().count(),
        1,
        "read-only commands must not initialize a data dir or extract runtime files"
    );
    assert!(
        !command(&binary)
            .arg("package-release")
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(
        !command(&binary)
            .args(["config", "init", "--data-dir", "relative"])
            .output()
            .unwrap()
            .status
            .success()
    );
    let config_path = root.path().join("config.toml");
    fs::write(&config_path, output.stdout).unwrap();
    successful(
        &binary,
        &["--config", config_path.to_str().unwrap(), "config", "check"],
    );
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
    child: Child,
    lease: PathBuf,
    digest: String,
    log: PathBuf,
}

impl Process {
    fn spawn(binary: &Path, config: &Path, data: &Path, log: &Path) -> Self {
        let file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(log)
            .unwrap();
        let child = command(binary)
            .arg("--config")
            .arg(config)
            .arg("run")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(file)
            .spawn()
            .unwrap();
        let (lock, _) = embedded_runtime_lock().unwrap();
        Self {
            child,
            lease: data.join("runtime/child.lease"),
            digest: lock.current_target().unwrap().1.binary_sha256.clone(),
            log: log.to_owned(),
        }
    }

    async fn ready(&mut self, address: SocketAddr) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            assert!(
                self.child.try_wait().unwrap().is_none(),
                "ocd exited: {}",
                fs::read_to_string(&self.log).unwrap()
            );
            if let Ok(Ok(true)) =
                tokio::time::timeout(Duration::from_secs(1), ready_request(address)).await
            {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "not ready: {}",
                fs::read_to_string(&self.log).unwrap()
            );
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
        assert!(
            !self.lease.exists(),
            "normal shutdown must remove its child lease"
        );
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Err(error) = recover_orphan_for_test(&self.lease, &self.digest) {
            eprintln!("single-binary fixture cleanup failed: {}", error.code());
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn single_file_first_start_restart_orphan_recovery_and_corruption_failure() {
    let root = Evidence::new();
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
    let mut config = PlatformConfig::default();
    config.server.public_bind = address.to_string();
    config.server.admin_auth = SecretReference {
        env: None,
        file: Some(admin_token),
    };
    config.server.deployer_auth = SecretReference {
        env: None,
        file: Some(deployer_token),
    };
    config.server.read_only_auth = SecretReference {
        env: None,
        file: Some(read_only_token),
    };
    config.storage.data_dir = data.clone();
    config.storage.master_key_file = data.join("keys/master.key");
    config.s3.endpoint = mock.endpoint.clone();
    config.s3.region = "us-east-1".to_owned();
    config.s3.access_key_id_env = None;
    config.s3.secret_access_key_env = None;
    config.s3.access_key_id_file = Some(key);
    config.s3.secret_access_key_file = Some(secret);
    config.runtime.shutdown_grace_ms = 1000;
    config.runtime.drain_timeout_ms = 1000;
    config.runtime.kill_timeout_ms = 1000;
    fs::write(&config_path, toml::to_string(&config).unwrap()).unwrap();
    let log = root.path().join("stderr.log");
    let mut process = Process::spawn(&binary, &config_path, &data, &log);
    process.ready(address).await;
    let package = data
        .join("runtime/packages")
        .join(embedded_payload_sha256());
    assert!(package.join("workerd").is_file());
    assert!(package.join("runtime/dist/manifest.json").is_file());
    let modified = fs::metadata(package.join("workerd"))
        .unwrap()
        .modified()
        .unwrap();
    let master_key = fs::read(data.join("keys/master.key")).unwrap();
    let competitor = command(&binary)
        .arg("--config")
        .arg(&config_path)
        .arg("run")
        .output()
        .unwrap();
    assert!(!competitor.status.success());
    assert!(String::from_utf8_lossy(&competitor.stderr).contains("DATA_DIR_IN_USE"));
    process.stop().await;

    let mut process = Process::spawn(&binary, &config_path, &data, &log);
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
    let mut recovered = Process::spawn(&binary, &config_path, &data, &log);
    recovered.ready(address).await;
    recovered.stop().await;
    drop(process);
    drop(recovered);
    let asset = package.join("runtime/config.capnp");
    fs::set_permissions(&asset, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&asset, "corrupt").unwrap();
    let failure = command(&binary)
        .arg("--config")
        .arg(&config_path)
        .arg("run")
        .output()
        .unwrap();
    assert!(!failure.status.success());
    assert!(String::from_utf8_lossy(&failure.stderr).contains("RUNTIME_INVALID"));
    assert_eq!(
        fs::read(&asset).unwrap(),
        b"corrupt",
        "corrupt cache must not be repaired silently"
    );
    assert_eq!(
        fs::read_dir(data.join("runtime/packages")).unwrap().count(),
        1
    );
    assert!(mock.object_count() == 0);
}
