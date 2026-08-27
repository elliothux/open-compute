//! Actual platformd SIGKILL after a durable step, followed by orphan recovery and replay.

use super::*;
use open_compute_storage::WorkerRepository;
use std::fs;
use std::net::SocketAddr;
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Instant;

type Client =
    hyper_util::client::legacy::Client<hyper_util::client::legacy::connect::HttpConnector, Body>;

async fn response(
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

async fn tenant_json(client: &Client, address: SocketAddr, path: &str) -> serde_json::Value {
    let response = response(client, address, path, "POST").await.unwrap();
    assert_eq!(response.status(), 200);
    let bytes = to_bytes(Body::new(response.into_body()), 65536)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        if self.0.try_wait().unwrap().is_none() {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}
pub(super) struct Evidence(pub(super) Option<tempfile::TempDir>);
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

fn address() -> SocketAddr {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
}

fn spawn(config: &Path, log: &Path) -> Process {
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
    )
}

async fn ready(client: &Client, admin: SocketAddr, child: &mut Process) {
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        assert!(child.0.try_wait().unwrap().is_none());
        if response(client, admin, "/health/ready", "GET")
            .await
            .is_ok_and(|r| r.status() == 200)
        {
            return;
        }
        assert!(Instant::now() < deadline, "process readiness timed out");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workflow_platformd_sigkill_after_step_commit_replays_without_callback() {
    let mut harness = Harness::start().await;
    let store = Arc::new(
        SchedulerStore::open(
            &harness.storage.data_dir().ensure_scheduler_db().unwrap(),
            5000,
            now(),
        )
        .unwrap(),
    );
    let account = harness.storage.identity().default_account_id;
    let definition = WorkflowRepository::new(harness.storage.db())
        .create_definition(account, "crash-flow", now())
        .unwrap();
    let target = harness.deploy(SOURCE, "Flow").await;
    WorkflowApiState::new(
        harness.storage.clone(),
        store.clone(),
        harness.transport.clone(),
    )
    .create_version(account, definition.id, target.deployment_id, "Flow".into())
    .await
    .unwrap();
    let caller = harness
        .deploy_bound(
            CALLER,
            "Flow",
            BTreeMap::from([(
                "FLOW".into(),
                DeploymentBindingInput {
                    kind: BindingKind::Workflow,
                    id: ResourceId::from_uuid(definition.id.as_uuid()).unwrap(),
                    permissions: CanonicalPermissions::default(),
                    config: CanonicalBindingConfig::default(),
                },
            )]),
        )
        .await;
    let workers = WorkerRepository::new(harness.storage.db());
    workers
        .promote(
            account,
            caller.worker_id,
            caller.deployment_id,
            None,
            RequestId::generate(),
            now(),
        )
        .unwrap();
    workers
        .create_exact_route(
            account,
            caller.worker_id,
            "workflow.example",
            "/",
            None,
            Some(caller.deployment_id),
            RequestId::generate(),
            now(),
        )
        .unwrap();
    harness.quiesce().await;
    let mock = harness.mock.clone();
    let evidence = Evidence(harness.temp.take());
    let root = evidence.0.as_ref().unwrap().path();
    let data = harness.storage.data_dir().root().to_owned();
    drop(store);
    drop(harness);
    let public = address();
    let admin = address();
    let config = config(root, &data, &mock.endpoint, public, admin);
    let log = root.join("platformd.log");
    let client: Client =
        hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
            .build_http();
    let mut first = spawn(&config, &log);
    ready(&client, admin, &mut first).await;
    let create = tenant_json(&client, public, "/create/crash-instance").await;
    assert_eq!(create["id"], "crash-instance");
    let connection = rusqlite::Connection::open_with_flags(
        data.join("scheduler.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let committed = loop {
        let output: Option<String> = connection.query_row("SELECT CAST(s.output_json AS TEXT) FROM workflow_steps s
            JOIN workflow_instances i ON i.id=s.instance_id WHERE i.external_instance_id='crash-instance' AND s.state='complete'", [], |row|row.get(0)).ok();
        if let Some(output) = output {
            break output;
        }
        assert!(first.0.try_wait().unwrap().is_none());
        assert!(Instant::now() < deadline, "first step did not commit");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    first.0.kill().unwrap();
    assert!(!first.0.wait().unwrap().success());
    drop(first);
    let mut second = spawn(&config, &log);
    ready(&client, admin, &mut second).await;
    let deadline = Instant::now() + Duration::from_secs(45);
    let status = loop {
        let status = tenant_json(&client, public, "/status/crash-instance").await;
        if status["status"] == "complete" {
            break status;
        }
        assert!(second.0.try_wait().unwrap().is_none());
        assert!(
            Instant::now() < deadline,
            "replay did not complete: {status}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert_eq!(status["output"]["callbacks"], 0);
    assert_eq!(
        status["output"]["nonce"],
        serde_json::from_str::<serde_json::Value>(&committed).unwrap()
    );
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM workflow_instances", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM workflow_steps", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert!(
        Command::new("/bin/kill")
            .args(["-TERM", &second.0.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(exit) = second.0.try_wait().unwrap() {
            assert!(exit.success());
            break;
        }
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    drop(connection);
    let reopened = SchedulerStore::open(&data.join("scheduler.sqlite"), 5000, now()).unwrap();
    assert_eq!(reopened.inspect_workflows(now()).unwrap().complete, 1);
    assert_eq!(reopened.inspect_workflows(now()).unwrap().running, 0);
    drop(reopened);
    assert!(!fs::read_to_string(&log).unwrap().contains(&committed));
}

fn config(
    root: &Path,
    data: &Path,
    endpoint: &str,
    public: SocketAddr,
    admin: SocketAddr,
) -> PathBuf {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
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
binary = "{}"
lock_file = "{}"
assets_dir = "{}"
startup_timeout_ms = 20000
shutdown_grace_ms = 500
[workflows]
lease_ms = 6000
heartbeat_ms = 1000
dispatch_timeout_ms = 30000
recovery_backoff_ms = 100
"#,
            data.display(),
            data.join("keys/master.key").display(),
            key.display(),
            secret.display(),
            PathBuf::from(std::env::var_os("OPEN_COMPUTE_TEST_WORKERD").unwrap()).display(),
            workspace.join("runtime/workerd.lock.json").display(),
            workspace.join("runtime").display()
        ),
    )
    .unwrap();
    fs::set_permissions(&config, fs::Permissions::from_mode(0o600)).unwrap();
    config
}

const SOURCE: &str = r#"
import { WorkflowEntrypoint } from 'cloudflare:workers';
export class Flow extends WorkflowEntrypoint {
  async run(event,step) {
    let callbacks = 0;
    const nonce = await step.do('commit', () => { callbacks++; return crypto.randomUUID(); });
    await new Promise(resolve => setTimeout(resolve, 10000));
    return {nonce,callbacks};
  }
}
export default {fetch(){return new Response('workflow');}};
"#;
