//! HTTP -> Queue -> Consumer -> durable Workflow -> KV/D1/R2/DO, across real SIGKILL cuts.

#![cfg(feature = "test-support")]

#[allow(dead_code)]
mod p0_exit_support;
#[path = "workflow_support/platform_process.rs"]
#[allow(dead_code)]
mod platform_process;
#[path = "p2_exit_support/setup.rs"]
mod setup;

use axum::body::{Body, to_bytes};
use axum::http::Request;
use open_compute_core::{ErrorCode, WorkflowFence, WorkflowToken, WorkflowsConfig};
use open_compute_storage::SchedulerStore;
use open_compute_storage::scheduler::{WorkflowStepAttempt, WorkflowStepOutcome};
use platform_process::{Client, Process, address, config, ready, spawn};
use rusqlite::{Connection, OpenFlags, OptionalExtension as _};
use serde_json::{Value, json};
use std::io::Write as _;
use std::net::SocketAddr;
use std::process::Command;
use std::time::{Duration, Instant};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p2_chain_preserves_queue_handoff_frozen_workflow_and_due_work_across_sigkill() {
    let fixture = setup::prepare().await;
    let root = fixture.evidence.0.as_ref().unwrap().path();
    let public = address();
    let admin = address();
    let config = config(root, &fixture.data, &fixture.mock.endpoint, public, admin);
    let contents = std::fs::read_to_string(&config).unwrap().replace(
        "dispatch_timeout_ms = 30000",
        "dispatch_timeout_ms = 120000",
    );
    std::fs::write(&config, contents).unwrap();
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&config)
        .unwrap();
    writeln!(file, "\n[scheduler]\nclaim_lease_ms=6000\ndispatch_timeout_ms=5000\nlease_guard_ms=1000\nshutdown_drain_ms=2000").unwrap();
    // This Gate validates crash recovery, not the independent disk-pressure path.
    // Keep it isolated from unrelated host-volume utilization while retaining a
    // fail-closed stop-writes threshold inside the supported hard bounds.
    writeln!(
        file,
        "\n[durable_objects]\ndisk_high_watermark_percent=98\ndisk_stop_writes_percent=99"
    )
    .unwrap();
    // Restart with the same product policy that froze resource authority during setup.
    for (section, policy) in [
        (
            "kv",
            toml::to_string(&p0_exit_support::kv_config()).unwrap(),
        ),
        (
            "r2",
            toml::to_string(&p0_exit_support::r2_config()).unwrap(),
        ),
        (
            "d1",
            toml::to_string(&p0_exit_support::d1_config()).unwrap(),
        ),
    ] {
        writeln!(file, "\n[{section}]\n{policy}").unwrap();
    }
    drop(file);
    let log = root.join("platformd.log");
    let client: Client =
        hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
            .build_http();
    let database = Connection::open_with_flags(
        fixture.data.join("scheduler.sqlite"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let mut process = spawn(&config, &log);
    ready(&client, admin, &mut process).await;

    // Accepted HTTP send is durable before the delayed consumer can claim it.
    assert_eq!(
        request(&client, public, "/enqueue/chain", json!({})).await["accepted"],
        true
    );
    assert_eq!(
        count(
            &database,
            "SELECT count(*) FROM queue_messages WHERE state='ready'"
        ),
        1
    );
    crash(&mut process);
    process = spawn(&config, &log);
    ready(&client, admin, &mut process).await;
    let identity: String = wait(
        &mut process,
        "consumer create before acknowledgement",
        || {
            database
                .query_row(
                    "SELECT i.id FROM workflow_instances i WHERE i.external_instance_id='chain'
            AND EXISTS(SELECT 1 FROM queue_messages WHERE state='claimed')",
                    [],
                    |r| r.get(0),
                )
                .optional()
                .unwrap()
        },
    )
    .await;
    assert_eq!(
        count(&database, "SELECT count(*) FROM workflow_instances"),
        1
    );
    assert_eq!(count(&database, "SELECT count(*) FROM queue_messages"), 1);

    // The consumer created a Workflow, but its acknowledgement has not committed.
    // Redelivery must find that same instance rather than losing or duplicating it.
    crash(&mut process);
    process = spawn(&config, &log);
    ready(&client, admin, &mut process).await;
    request(&client, public, "/release-consumer/chain", json!({})).await;
    wait(&mut process, "redelivered consumer acknowledgement", || {
        (count(&database, "SELECT count(*) FROM queue_messages") == 0).then_some(())
    })
    .await;
    let deadline = Instant::now() + Duration::from_secs(45);
    let (fence, attempt) = loop {
        if let Some(grant) = grant(&database) {
            break grant;
        }
        if count(
            &database,
            "SELECT count(*) FROM workflow_steps WHERE ordinal=0 AND error_code IS NOT NULL",
        ) > 0
        {
            let diagnostic = request(&client, public, "/diagnostic/chain", json!({})).await;
            panic!("product step failed before its commit: {diagnostic}");
        }
        assert!(
            Instant::now() < deadline,
            "uncommitted callback grant was not issued"
        );
        assert!(process.0.try_wait().unwrap().is_none());
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let committed: String = database
        .query_row(
            "SELECT CAST(output_json AS TEXT) FROM workflow_steps
        WHERE instance_id=?1 AND ordinal=0 AND state='complete'",
            [&identity],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(fence.instance_id.to_string(), identity);
    assert_eq!(attempt.attempt, 1);
    assert_eq!(
        request(&client, public, "/guards/chain", json!({})).await,
        json!({"id":"chain", "status":"running", "errors":
            vec!["WORKFLOW_DO_OUTPUT_GATE_UNSUPPORTED"; 6]}),
    );
    assert_eq!(
        count(&database, "SELECT count(*) FROM workflow_instances"),
        1
    );

    // Kill an actual platformd while a callback owns an uncommitted attempt.
    // Its completed sibling remains immutable, while Unknown reissues the same attempt.
    crash(&mut process);
    process = spawn(&config, &log);
    ready(&client, admin, &mut process).await;
    let (next_fence, next_attempt) = wait(&mut process, "Unknown callback recovery", || {
        grant(&database).filter(|(next, _)| next.run_token != fence.run_token)
    })
    .await;
    assert_eq!(next_attempt.attempt, attempt.attempt);
    assert_eq!(next_fence.instance_generation, fence.instance_generation);
    let store = SchedulerStore::open(
        &fixture.data.join("scheduler.sqlite"),
        5000,
        p0_exit_support::now_ms(),
    )
    .unwrap();
    assert_eq!(
        store
            .settle_workflow_step(
                &fence,
                &attempt,
                WorkflowStepOutcome::Success("\"stale\""),
                p0_exit_support::now_ms(),
                &WorkflowsConfig::default()
            )
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowRunStale
    );
    drop(store);
    request(&client, public, "/release-workflow/chain", json!({})).await;
    let due: i64 = wait(&mut process, "durable sleep releases its run lease", || {
        database.query_row("SELECT s.due_at_ms FROM workflow_steps s JOIN workflow_instances i ON i.id=s.instance_id
            WHERE i.id=?1 AND s.kind='sleep' AND s.state='waiting' AND i.state='waiting' AND i.run_token IS NULL",
            [&identity], |r| r.get(0)).optional().unwrap()
    }).await;
    request(&client, public, "/pause/chain", json!({})).await;
    request(
        &client,
        public,
        "/event/chain",
        json!({"type":"continue","payload":{"accepted":true}}),
    )
    .await;
    assert_eq!(count(&database, "SELECT count(*) FROM workflow_events"), 1);
    let version = request(
        &client,
        admin,
        &format!(
            "/v1/accounts/{}/workflows/{}/versions",
            fixture.account, fixture.definition
        ),
        json!({"deploymentId":fixture.future,"className":"Flow"}),
    )
    .await;
    assert_eq!(version["state"], "ready");
    request(&client, public, "/arm/chain", json!({})).await;
    assert!(
        p0_exit_support::now_ms() < due,
        "sleep must become due after the crash cut"
    );

    // A paused Workflow retains the accepted event and original sleep deadline.
    // Both its deadline and the DO's native alarm become due while platformd is down.
    crash(&mut process);
    let delay = u64::try_from(due.saturating_sub(p0_exit_support::now_ms()).max(0)).unwrap();
    tokio::time::sleep(Duration::from_millis(delay.max(2200))).await;
    process = spawn(&config, &log);
    ready(&client, admin, &mut process).await;
    assert_eq!(
        request(&client, public, "/status/chain", json!({})).await["status"],
        "paused"
    );
    assert_eq!(count(&database, "SELECT count(*) FROM workflow_events"), 1);
    assert_eq!(
        database
            .query_row(
                "SELECT started_at_ms+json_extract(CAST(config_json AS TEXT),'$.durationMs')
                 FROM workflow_steps WHERE instance_id=?1 AND kind='sleep'",
                [&identity],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        due
    );
    // Maintenance may settle a due sleep while paused, clearing only its due
    // projection. It must preserve the original deadline and not execute the
    // next user step until resume.
    let completed_at: i64 = wait(&mut process, "paused sleep expiry", || {
        database
            .query_row(
                "SELECT completed_at_ms FROM workflow_steps
            WHERE instance_id=?1 AND kind='sleep' AND state='complete' AND due_at_ms IS NULL",
                [&identity],
                |row| row.get(0),
            )
            .optional()
            .unwrap()
    })
    .await;
    assert!(completed_at >= due);
    assert_eq!(count(&database, "SELECT count(*) FROM workflow_steps"), 3);
    assert_eq!(
        count(
            &database,
            "SELECT count(*) FROM workflow_instances WHERE run_token IS NOT NULL"
        ),
        0
    );
    request(&client, public, "/resume/chain", json!({})).await;
    wait(
        &mut process,
        "Workflow completion after paused event recovery",
        || {
            database
                .query_row(
                    "SELECT output_json FROM workflow_instances WHERE id=?1 AND state='complete'",
                    [&identity],
                    |r| r.get::<_, Vec<u8>>(0),
                )
                .optional()
                .unwrap()
        },
    )
    .await;
    let status = request(&client, public, "/status/chain", json!({})).await;
    let output = &status["output"];
    assert_eq!(output["version"], "frozen");
    assert_eq!(output["callbacks"], 0);
    assert_eq!(output["payload"], json!({"accepted":true}));
    assert_eq!(output["dated"], true);
    assert_eq!(
        output["products"],
        serde_json::from_str::<Value>(&committed).unwrap()
    );
    for product in ["kv", "r2", "d1"] {
        assert_eq!(output["products"][product], "frozen");
    }
    assert_eq!(output["products"]["object"]["count"], 1);
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let effects = request(&client, public, "/effects/chain", json!({})).await;
        if effects["alarm"] == "done" {
            assert_eq!(effects["kv"], "frozen");
            assert_eq!(effects["r2"], "frozen");
            assert_eq!(effects["rows"], 1);
            assert_eq!(effects["object"]["count"], 1);
            break;
        }
        assert!(Instant::now() < deadline, "DO alarm was not recovered");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let store = SchedulerStore::open(
        &fixture.data.join("scheduler.sqlite"),
        5000,
        p0_exit_support::now_ms(),
    )
    .unwrap();
    let instance = store.workflow_instance(fence.instance_id).unwrap().unwrap();
    assert_eq!(instance.identity.target.deployment_id, fixture.frozen);
    store.verify_workflow_history(fence.instance_id).unwrap();
    assert_eq!(
        store
            .queue_metrics(fixture.queue, 1, 1)
            .unwrap()
            .backlog_count,
        0
    );
    assert_eq!(
        count(&database, "SELECT count(*) FROM workflow_instances"),
        1
    );
    assert_eq!(count(&database, "SELECT count(*) FROM workflow_events"), 0);
    assert_eq!(
        count(
            &database,
            "SELECT count(*) FROM workflow_steps WHERE state!='complete'"
        ),
        0
    );
    drop(store);
    assert!(
        Command::new("/bin/kill")
            .args(["-TERM", &process.0.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(exit) = process.0.try_wait().unwrap() {
            assert!(exit.success());
            break;
        }
        assert!(Instant::now() < deadline, "platformd did not stop");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(tokio::net::TcpStream::connect(public).await.is_err());
    assert!(tokio::net::TcpStream::connect(admin).await.is_err());
    let logs = std::fs::read_to_string(log).unwrap();
    assert!(!logs.contains(&committed));
    for token in [&fence.run_token, &attempt.step_token] {
        assert!(!logs.contains(&serde_json::to_string(token).unwrap()));
    }
}

fn crash(process: &mut Process) {
    process.0.kill().unwrap();
    assert!(!process.0.wait().unwrap().success());
}

fn count(database: &Connection, sql: &str) -> i64 {
    database.query_row(sql, [], |row| row.get(0)).unwrap()
}

async fn wait<T>(process: &mut Process, phase: &str, mut inspect: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        assert!(
            process.0.try_wait().unwrap().is_none(),
            "platformd exited during {phase}"
        );
        if let Some(value) = inspect() {
            return value;
        }
        assert!(Instant::now() < deadline, "timed out: {phase}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn grant(database: &Connection) -> Option<(WorkflowFence, WorkflowStepAttempt)> {
    database.query_row("SELECT i.id,i.instance_generation,i.run_token,s.ordinal,s.attempt,s.step_token
        FROM workflow_instances i JOIN workflow_steps s ON s.instance_id=i.id
        WHERE i.external_instance_id='chain' AND i.state='running' AND s.ordinal=1 AND s.state='running'", [], |row| {
        Ok((WorkflowFence {
            instance_id: row.get::<_, String>(0)?.parse().unwrap(), instance_generation: row.get(1)?,
            run_token: WorkflowToken::from_bytes(row.get::<_, Vec<u8>>(2)?.try_into().unwrap()),
        }, WorkflowStepAttempt {
            ordinal: row.get(3)?, attempt: row.get(4)?,
            step_token: WorkflowToken::from_bytes(row.get::<_, Vec<u8>>(5)?.try_into().unwrap()),
        }))
    }).optional().unwrap()
}

async fn request(client: &Client, address: SocketAddr, path: &str, body: Value) -> Value {
    let request = Request::builder()
        .method("POST")
        .uri(format!("http://{address}{path}"))
        .header("host", "workflow.example")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = tokio::time::timeout(Duration::from_secs(10), client.request(request))
        .await
        .unwrap()
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(Body::new(response.into_body()), 65536)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(status.is_success(), "{path}: {status}: {value}");
    value
}
