//! P2.4.0 runtime feasibility: real artifacts, immutable loader, callback-aware RPC,
//! and test-owned SQLite persistence before introducing a production schema.

#[path = "workflow_support/output_gate.rs"]
mod output_gate;
mod workflow_support;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use open_compute_core::{WorkflowFence, WorkflowInstanceId, WorkflowToken};
use open_compute_runtime::GenerationAuthRegistry;
use open_compute_service::runtime_bridge::WorkflowRunRequest;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use workflow_support::Harness;

#[derive(Clone)]
struct ProbeBackend {
    auth: GenerationAuthRegistry,
    db: Arc<Mutex<Connection>>,
    fault: Arc<AtomicU8>,
}

async fn backend(
    State(state): State<ProbeBackend>,
    Path(operation): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    if !state.auth.authorize(
        headers
            .get("x-open-compute-binding-token")
            .unwrap()
            .to_str()
            .unwrap(),
        headers
            .get("x-open-compute-startup-generation")
            .unwrap()
            .to_str()
            .unwrap(),
    ) {
        return (StatusCode::NOT_FOUND, Json(json!({})));
    }
    let db = state.db.lock().unwrap();
    let id = body["instanceId"].as_str().unwrap();
    let ordinal = body["ordinal"].as_i64().unwrap();
    if body["runToken"] != "11".repeat(32) || body["instanceGeneration"] != 1 {
        return (
            StatusCode::OK,
            Json(json!({"errorCode":"WORKFLOW_RUN_STALE"})),
        );
    }
    if operation == "claim" {
        let row: Option<(String, Option<String>)> = db
            .query_row(
                "SELECT state, output FROM probe_steps WHERE id=?1 AND ordinal=?2",
                params![id, ordinal],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .unwrap();
        if let Some((status, output)) = row {
            if status == "complete" {
                return (
                    StatusCode::OK,
                    Json(json!({"state":"complete","outputJson":output.unwrap()})),
                );
            }
            if status == "failed" {
                return (
                    StatusCode::OK,
                    Json(json!({"state":"failed","error":{"message":"Workflow execution failed"}})),
                );
            }
        }
        db.execute(
            "INSERT OR IGNORE INTO probe_steps VALUES (?1,?2,'running',NULL)",
            params![id, ordinal],
        )
        .unwrap();
        let token = if state
            .fault
            .compare_exchange(3, 0, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            "33".repeat(32)
        } else {
            "22".repeat(32)
        };
        return (
            StatusCode::OK,
            Json(json!({"state":"run","stepToken":token})),
        );
    }
    if body["stepToken"] != "22".repeat(32) {
        return (
            StatusCode::OK,
            Json(json!({"errorCode":"WORKFLOW_STEP_STALE"})),
        );
    }
    let fault = state.fault.swap(0, Ordering::SeqCst);
    if fault == 1 {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({})));
    }
    if operation == "success" {
        let input = body["outputJson"].as_str().unwrap();
        let canonical = open_compute_core::workflow::canonical_json(
            input,
            open_compute_core::ErrorCode::WorkflowResultTooLarge,
        )
        .unwrap();
        assert_eq!(canonical, input);
    }
    let (state, output) = if operation == "success" {
        ("complete", body["outputJson"].as_str())
    } else {
        ("failed", None)
    };
    db.execute(
        "UPDATE probe_steps SET state=?3,output=?4 WHERE id=?1 AND ordinal=?2 AND state='running'",
        params![id, ordinal, state, output],
    )
    .unwrap();
    if fault == 2 {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({})));
    }
    (StatusCode::OK, Json(json!({"ok":true})))
}

const SOURCE: &str = concat!(
    include_str!("workflow_support/promise-probe.js"),
    r#"
import { WorkflowEntrypoint } from 'cloudflare:workers';
export class Orders extends WorkflowEntrypoint {
  async run(event, step) {
    if (!(event.timestamp instanceof Date) || this.env.MODE !== 'frozen'
        || Object.keys(this.env).join(',') !== 'MODE'
        || Object.keys(event).sort().join(',') !== 'instanceId,payload,timestamp,workflowName'
        || Object.keys(this.ctx.props).length) throw new Error('private identity leak');
    const mode = event.payload.mode;
    if (mode === 'primordials') return observeWorkflowGrant(step);
    if (mode === 'throw') throw new Error('secret /private/path Bearer password');
    if (mode === 'timeout') await new Promise(resolve => setTimeout(resolve, 10000));
    if (mode === 'background') this.ctx.waitUntil(Promise.reject(new Error('secret background')));
    let calls = 0;
    const count = mode === 'many' ? 1024 : 2;
    let value;
    for (let i = 0; i < count; i++) {
      try {
        value = await step.do('same-name', async ctx => {
          calls++;
          if (mode === 'caught') throw new Error('secret caught');
          if (mode === 'bigint') return 1n;
          if (mode === 'cycle') { const value = {}; value.self = value; return value; }
          if (ctx.attempt !== 1 || ctx.config !== null || ctx.step.count !== i + 1) throw new Error('context');
          return { index: i, value: event.payload.value };
        });
      } catch (error) { if (mode !== 'caught') throw error; }
    }
    if (mode === 'parallel') await Promise.all([step.do('one', async () => 1), step.do('two', async () => 2)]);
    return { calls, value, instanceId: event.instanceId };
  }
}
export class Wrong { run() {} }
export class Missing extends WorkflowEntrypoint {}
export default { fetch() { return new Response('ordinary'); } };
"#
);

fn envelope(mode: &str) -> WorkflowRunRequest {
    WorkflowRunRequest {
        fence: WorkflowFence {
            instance_id: WorkflowInstanceId::generate(),
            instance_generation: 1,
            run_token: WorkflowToken::from_bytes([0x11; 32]),
        },
        external_instance_id: "orders-1".into(),
        definition_name: "orders".into(),
        created_at_ms: 1_787_700_000_000,
        payload_json: json!({"mode":mode,"value":"frozen-payload"}).to_string(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dynamic_workflow_step_bridge_replay_and_restart() {
    let mut harness = Harness::start().await;
    let db = Connection::open(
        harness
            .storage
            .data_dir()
            .root()
            .join("workflow-probe.sqlite"),
    )
    .unwrap();
    db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
        CREATE TABLE probe_steps(id TEXT,ordinal INTEGER,state TEXT,output TEXT,PRIMARY KEY(id,ordinal));").unwrap();
    let fault = Arc::new(AtomicU8::new(0));
    let state = ProbeBackend {
        auth: harness.binding_auth.clone(),
        db: Arc::new(Mutex::new(db)),
        fault: fault.clone(),
    };
    let listener = harness.binding_listener.take().unwrap();
    let mut shutdown = harness.shutdown.subscribe();
    let backend_task = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route("/internal/workflows/v1/runs/{operation}", post(backend))
                .with_state(state),
        )
        .with_graceful_shutdown(async move {
            let _ = shutdown.changed().await;
        })
        .await
        .unwrap();
    });
    let target = harness.deploy(SOURCE, "Orders").await;
    harness.transport.probe_workflow(&target).await.unwrap();
    let request = envelope("normal");
    let first = harness
        .transport
        .dispatch_workflow(&target, &request, Duration::from_secs(10))
        .await;
    if first.is_err() {
        harness.supervisor.shutdown().await;
    }
    assert!(
        first.is_ok(),
        "{first:?} {:?}",
        harness.supervisor.last_diagnostics()
    );
    let first = first.unwrap();
    assert_eq!(first.outcome, "complete", "{first:?}");
    assert_eq!(first.loader_outcome, "cold");
    assert_eq!(
        serde_json::from_str::<Value>(&first.output_json.unwrap()).unwrap()["calls"],
        2
    );
    let hostile = harness
        .transport
        .dispatch_workflow(&target, &envelope("primordials"), Duration::from_secs(10))
        .await
        .unwrap();
    assert_eq!(hostile.outcome, "complete");
    assert_eq!(
        serde_json::from_str::<Value>(&hostile.output_json.unwrap()).unwrap(),
        json!({"observedPrivateGrant":false}),
        "tenant Promise hooks must never observe private backend grants"
    );
    let replay = harness
        .transport
        .dispatch_workflow(&target, &request, Duration::from_secs(10))
        .await
        .unwrap();
    assert_eq!(replay.loader_outcome, "warm");
    assert_eq!(
        serde_json::from_str::<Value>(&replay.output_json.unwrap()).unwrap()["calls"],
        0
    );
    harness.restart().await;
    let replay = harness
        .transport
        .dispatch_workflow(&target, &request, Duration::from_secs(10))
        .await
        .unwrap();
    assert_eq!(replay.loader_outcome, "cold");
    assert_eq!(
        serde_json::from_str::<Value>(&replay.output_json.unwrap()).unwrap()["calls"],
        0
    );
    for mode in [
        "throw",
        "caught",
        "background",
        "parallel",
        "bigint",
        "cycle",
    ] {
        let result = harness
            .transport
            .dispatch_workflow(&target, &envelope(mode), Duration::from_secs(10))
            .await
            .unwrap();
        assert_eq!(result.outcome, "errored", "{mode}: {result:?}");
        assert!(
            !serde_json::to_string(&result.error)
                .unwrap()
                .contains("secret")
        );
    }
    let mut stale = envelope("normal");
    stale.fence.run_token = WorkflowToken::from_bytes([0x33; 32]);
    let result = harness
        .transport
        .dispatch_workflow(&target, &stale, Duration::from_secs(10))
        .await
        .unwrap();
    assert_eq!(result.error_code.as_deref(), Some("WORKFLOW_RUN_STALE"));
    fault.store(3, Ordering::SeqCst);
    let result = harness
        .transport
        .dispatch_workflow(&target, &envelope("normal"), Duration::from_secs(10))
        .await
        .unwrap();
    assert_eq!(result.error_code.as_deref(), Some("WORKFLOW_STEP_STALE"));
    for (point, callbacks_after_reactivation) in [(1, 2), (2, 1)] {
        let request = envelope("normal");
        fault.store(point, Ordering::SeqCst);
        assert!(
            harness
                .transport
                .dispatch_workflow(&target, &request, Duration::from_secs(10))
                .await
                .is_err()
        );
        let recovered = harness
            .transport
            .dispatch_workflow(&target, &request, Duration::from_secs(10))
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&recovered.output_json.unwrap()).unwrap()["calls"],
            callbacks_after_reactivation
        );
    }
    let many = harness
        .transport
        .dispatch_workflow(&target, &envelope("many"), Duration::from_secs(30))
        .await
        .unwrap();
    assert_eq!(many.outcome, "complete", "{many:?}");
    assert_eq!(many.final_ordinal, 1024);
    assert!(
        harness
            .transport
            .dispatch_workflow(&target, &envelope("timeout"), Duration::from_millis(100))
            .await
            .is_err()
    );
    for class in ["Wrong", "Missing", "Absent"] {
        let mut invalid = target.clone();
        invalid.entrypoint = Some(class.into());
        assert!(harness.transport.probe_workflow(&invalid).await.is_err());
    }
    harness.stop().await;
    backend_task.await.unwrap();
}
