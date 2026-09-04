//! Actual ocd SIGKILL after a durable step, followed by orphan recovery and replay.

pub(super) use super::platform_process::Evidence;
use super::platform_process::{Client, address, config, ready, spawn, tenant_json};
use super::*;
use open_compute_storage::WorkerRepository;
use std::fs;
use std::process::Command;
use std::time::Instant;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workflow_ocd_sigkill_after_step_commit_replays_without_callback() {
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
        Default::default(),
    )
    .create_version(account, definition.id, target.version_id, "Flow".into())
    .await
    .unwrap();
    let caller = harness
        .deploy_bound(
            CALLER,
            "Flow",
            BTreeMap::from([(
                "FLOW".into(),
                VersionBindingInput {
                    kind: BindingKind::Workflow,
                    id: ResourceId::from_uuid(definition.id.as_uuid()).unwrap(),
                    permissions: CanonicalPermissions::default(),
                    config: CanonicalBindingConfig {
                        workflow_class_name: Some("Flow".into()),
                        ..Default::default()
                    },
                },
            )]),
        )
        .await;
    let workers = WorkerRepository::new(harness.storage.db());
    workers
        .promote(
            account,
            caller.worker_id,
            caller.version_id,
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
            Some(caller.version_id),
            RequestId::generate(),
            now(),
            1_000_000,
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
    let log = root.join("ocd.log");
    let client: Client =
        hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
            .build_http();
    let mut first = spawn(&config, &log);
    ready(&client, admin, &mut first).await;
    let create = tenant_json(&client, public, "/create/crash-instance").await;
    assert_eq!(create["id"], "crash-instance", "{create}");
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
    assert_eq!(status["output"]["nonce"], decode_workflow_json(&committed));
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
