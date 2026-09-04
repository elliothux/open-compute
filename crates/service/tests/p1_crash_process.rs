//! P1 real-process SIGKILL/orphan recovery Gate.

mod common;

use common::scrub_shell_s3_env;

use open_compute_artifacts::MockS3;
use open_compute_core::{
    BindingKind, PlatformConfig, RequestId, ResourceId, ResourceState, SystemClock,
};
use open_compute_service::config_load::load_platform_config;
use open_compute_storage::{
    ControlDb, D1DatabaseRepository, D1Engine, D1Paths, D1QueryLimits, PlatformStorage,
    ReserveResourceCreate, ResourceCreateReservation, ResourceRecord, ResourceRepository,
};
use open_compute_workers::{D1ResourceDriver, KvResourceDriver, ResourceDriver};
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
    public: SocketAddr,
    admin: SocketAddr,
) -> PathBuf {
    let access_key = root.join("access-key");
    let secret_key = root.join("secret-key");
    let admin_token = root.join("admin-token");
    let deployer_token = root.join("deployer-token");
    let read_only_token = root.join("read-only-token");
    write_mode(&access_key, b"AKIAP1CRASHPROCESS1", 0o600);
    write_mode(
        &secret_key,
        b"wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
        0o600,
    );
    write_mode(&admin_token, b"p1-crash-admin\n", 0o600);
    write_mode(&deployer_token, b"p1-crash-deployer\n", 0o600);
    write_mode(&read_only_token, b"p1-crash-read-only\n", 0o600);
    let config = root.join("platform.toml");
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
startup_timeout_ms = 20000
shutdown_grace_ms = 2000
kill_timeout_ms = 1000

[hardening]
emergency_reserve_bytes = 16777216

[metrics]
enabled = true
max_label_value_bytes = 64
max_series = 1024
"#,
            data_dir = data_dir.display(),
            master_key = data_dir.join("keys/master.key").display(),
            endpoint = mock.endpoint,
            access_key = access_key.display(),
            secret_key = secret_key.display(),
            admin_token = admin_token.display(),
            deployer_token = deployer_token.display(),
            read_only_token = read_only_token.display(),
        ),
    )
    .expect("config");
    config
}

fn spawn_ocd(config: &Path, log: &Path) -> Child {
    let stderr = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(log)
        .expect("open bounded process log");
    let mut command = Command::new(env!("CARGO_BIN_EXE_ocd"));
    command
        .args(["run", "--config"])
        .arg(config)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr));
    scrub_shell_s3_env(&mut command);
    command.spawn().expect("spawn ocd")
}

fn signal(child: &Child, name: &str) {
    let status = Command::new("/bin/kill")
        .args([name, &child.id().to_string()])
        .status()
        .expect("signal ocd");
    assert!(status.success(), "signal {name} failed");
}

async fn wait_ready(address: SocketAddr, child: &mut Child) {
    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        assert!(
            child.try_wait().expect("child state").is_none(),
            "ocd exited"
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
        assert!(Instant::now() < deadline, "ocd did not become ready");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn wait_exit(child: &mut Child, timeout: Duration) -> std::process::ExitStatus {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().expect("child state") {
            return status;
        }
        assert!(Instant::now() < deadline, "ocd did not exit");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn seed_resource_recovery(config: &PlatformConfig) -> Vec<(ResourceRecord, ResourceState)> {
    let storage =
        PlatformStorage::bootstrap_with_hardening(&config.storage, &config.hardening, &SystemClock)
            .expect("initialize platform authority");
    let resources = ResourceRepository::new(storage.db());
    let reserve = |kind, name: &str| {
        let fingerprint = storage.crypto().fingerprint_request(name.as_bytes());
        let reservation = resources
            .reserve_create(
                &ReserveResourceCreate {
                    account_id: storage.identity().default_account_id,
                    kind,
                    name,
                    idempotency_key: name,
                    fingerprint_key_id: storage.crypto().fingerprint_key_id(),
                    request_fingerprint: &fingerprint,
                    resource_id: ResourceId::generate(),
                    driver_schema_version: 1,
                    request_id: RequestId::generate(),
                    now_ms: 1,
                    expires_at_ms: i64::MAX,
                },
                config.hardening.max_resources_per_kind_per_account,
            )
            .expect("reserve current resource intent");
        let ResourceCreateReservation::Reserved(resource) = reservation else {
            panic!("expected new resource reservation");
        };
        resource
    };

    // The daemon must recover both a catalog-free reservation and an unpublished
    // valid database without replacing the latter with an empty database.
    let kv = reserve(BindingKind::KvNamespace, "pending-kv");
    let d1 = reserve(BindingKind::D1Database, "pending-d1");
    D1DatabaseRepository::new(storage.db())
        .ensure_database(
            &d1,
            &D1Paths::storage_key(d1.account_id, d1.id),
            1,
            config.d1.database_quota_bytes,
        )
        .expect("D1 catalog intent");
    let stage = D1Paths::open(storage.data_dir().root())
        .expect("D1 paths")
        .create_database_staging(d1.id)
        .expect("D1 staging");
    let engine = D1Engine::create(
        &stage.join("data.sqlite"),
        d1.account_id,
        d1.id,
        d1.created_at_ms,
        config.d1.database_quota_bytes,
    )
    .expect("staged D1 database");
    engine
        .exec(
            "CREATE TABLE recovery_marker(value TEXT); INSERT INTO recovery_marker VALUES('retained');",
            D1QueryLimits::query(&config.d1).expect("D1 limits"),
        )
        .expect("staged user data");
    engine.checkpoint(true).expect("staging checkpoint");

    let mut expected = vec![(kv, ResourceState::Ready), (d1, ResourceState::Ready)];
    let drivers: [Box<dyn ResourceDriver + '_>; 2] = [
        Box::new(KvResourceDriver::new(
            &storage,
            config.kv.namespace_quota_bytes,
        )),
        Box::new(D1ResourceDriver::new(
            &storage,
            config.d1.database_quota_bytes,
        )),
    ];
    for (index, driver) in drivers.into_iter().enumerate() {
        let resource = reserve(driver.kind(), &format!("deleting-{index}"));
        driver.create(&resource).expect("create deletion fixture");
        resources.mark_ready(resource.id, 2).expect("ready fixture");
        resources
            .begin_delete(resource.account_id, resource.id, 3)
            .expect("persist deletion intent");
        let deleting = resources
            .get(resource.account_id, resource.id)
            .expect("deleting authority");
        driver.begin_delete(&deleting).expect("quarantine resource");
        expected.push((deleting, ResourceState::Tombstoned));
    }
    expected
}

fn assert_recovered_resources(data_dir: &Path, expected: &[(ResourceRecord, ResourceState)]) {
    // Only inspect through a WAL-aware read-only connection while ocd owns
    // the data directory. No second writer or lifecycle owner is introduced.
    let db = ControlDb::open_readonly_wal_aware(&data_dir.join("control.sqlite"), 5_000)
        .expect("read serving authority");
    let repository = ResourceRepository::new(&db);
    for (resource, state) in expected {
        assert_eq!(
            repository
                .get(resource.account_id, resource.id)
                .expect("recovered resource")
                .state,
            *state,
        );
    }
    assert!(
        repository
            .reconcile_candidates()
            .expect("pending resources")
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn p1_ocd_sigkill_reclaims_orphan_and_restarts_cleanly() {
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
    let config = write_config(&root, &data_dir, &mock, public, admin);
    let process_log = root.join("ocd.log");
    let loaded = load_platform_config(&config).expect("load config");
    let resources = seed_resource_recovery(&loaded.config);

    let mut first = ChildGuard(spawn_ocd(&config, &process_log));
    wait_ready(admin, first.child_mut()).await;
    assert_recovered_resources(&data_dir, &resources);
    signal(first.child(), "-KILL");
    let first_status = wait_exit(first.child_mut(), Duration::from_secs(5)).await;
    assert!(!first_status.success());

    let mut second = ChildGuard(spawn_ocd(&config, &process_log));
    wait_ready(admin, second.child_mut()).await;
    assert_recovered_resources(&data_dir, &resources);
    signal(second.child(), "-TERM");
    let second_status = wait_exit(second.child_mut(), Duration::from_secs(20)).await;
    assert!(
        second_status.success(),
        "graceful restart exit: {second_status}"
    );

    let storage = PlatformStorage::bootstrap(&loaded.config.storage, &SystemClock)
        .expect("reacquire data-dir and verify control SQLite");
    storage.db().quick_check().expect("control quick_check");
    let (d1, _) = resources
        .iter()
        .find(|(resource, state)| {
            resource.kind == BindingKind::D1Database && *state == ResourceState::Ready
        })
        .expect("recovered D1");
    let path = D1Paths::open(storage.data_dir().root())
        .expect("D1 paths")
        .database_path(d1.account_id, d1.id);
    let value: String =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("read recovered D1")
            .query_row("SELECT value FROM recovery_marker", [], |row| row.get(0))
            .expect("retained staged data");
    assert_eq!(value, "retained");
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
