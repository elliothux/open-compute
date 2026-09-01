//! Runtime feasibility through production loading and a test-owned real SQLite
//! protocol fixture. Product-schema and scheduler acceptance belong to the product Gate.

#[path = "workflow_support/output_gate.rs"]
mod output_gate;
mod workflow_support;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use open_compute_core::workflow::{WorkflowDurableConfig, WorkflowStepDeclaration};
use open_compute_core::{WorkflowFence, WorkflowInstanceId, WorkflowToken};
use open_compute_runtime::GenerationAuthRegistry;
use open_compute_service::runtime_bridge::{WorkflowOutcome, WorkflowRunRequest};
use open_compute_service::workflow_http::WorkflowApiState;
use open_compute_storage::{DeploymentState, SchedulerStore, WorkflowRepository};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use workflow_support::{Harness, decode_workflow_json, encode_workflow_json};

#[derive(Clone)]
struct Backend {
    auth: GenerationAuthRegistry,
    db: Arc<Mutex<Connection>>,
}

fn now() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
}

fn raw_tcp_fixture_payload() -> Option<Value> {
    const NAMES: [&str; 3] = [
        "OPEN_COMPUTE_EGRESS_PUBLIC_HOSTNAME",
        "OPEN_COMPUTE_EGRESS_PRIVATE_HOSTNAME",
        "OPEN_COMPUTE_EGRESS_PUBLIC_TCP_PORT",
    ];
    let values = NAMES.map(std::env::var);
    if values.iter().all(Result::is_err) {
        return None;
    }
    let [hostname, private_hostname, tcp_port] =
        values.map(|value| value.expect("all raw TCP fixture values must be set"));
    Some(json!({
        "mode": "rawTcp",
        "hostname": hostname,
        "privateHostname": private_hostname,
        "tcpPort": tcp_port,
    }))
}

fn result(db: &Connection, id: &str, ordinal: i64) -> Value {
    db.query_row(
        "SELECT descriptor,state,output,code FROM steps WHERE id=?1 AND ordinal=?2",
        params![id, ordinal],
        |row| {
            let descriptor: String = row.get(0)?;
            let state: String = row.get(1)?;
            let output: Option<String> = row.get(2)?;
            let code: Option<String> = row.get(3)?;
            let descriptor: Value = serde_json::from_str(&descriptor).unwrap();
            Ok(match state.as_str() {
                "complete" if descriptor["kind"] == "wait_event" => {
                    let event = decode_workflow_json(output.as_deref().unwrap());
                    json!({
                        "state": "event",
                        "type": event["type"],
                        "payloadBase64": encode_workflow_json(&event["payload"]),
                        "timestampMs": event["timestampMs"],
                    })
                }
                "complete" => output.map_or_else(
                    || json!({"state":"complete"}),
                    |output| json!({"state":"complete","outputBase64":output}),
                ),
                "failed" => json!({"state":"failed","code":code}),
                _ => json!({"state":"suspended"}),
            })
        },
    )
    .unwrap()
}

async fn handle(
    State(backend): State<Backend>,
    Path(operation): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    if !backend.auth.authorize(
        headers
            .get("x-open-compute-binding-token")
            .and_then(|v| v.to_str().ok())
            .unwrap_or(""),
        headers
            .get("x-open-compute-startup-generation")
            .and_then(|v| v.to_str().ok())
            .unwrap_or(""),
    ) {
        return (StatusCode::NOT_FOUND, Json(json!({})));
    }
    let mut db = backend.db.lock().unwrap();
    let tx = db.transaction().unwrap();
    let id = body["instanceId"].as_str().unwrap();
    let token = body["runToken"].as_str().unwrap();
    let current: Option<String> = tx
        .query_row(
            "SELECT token FROM runs WHERE id=?1 AND state='running'",
            [id],
            |row| row.get(0),
        )
        .optional()
        .unwrap();
    if current.as_deref() != Some(token) || body["instanceGeneration"] != 1 {
        return (
            StatusCode::OK,
            Json(json!({"errorCode":"WORKFLOW_RUN_STALE"})),
        );
    }
    let ordinal = body["ordinal"].as_i64().unwrap_or(0);
    let reply = match operation.as_str() {
        "claim-batch" => {
            let mut grants = Vec::new();
            for descriptor in body["steps"].as_array().unwrap() {
                let ordinal = descriptor["ordinal"].as_i64().unwrap();
                let declaration: WorkflowStepDeclaration =
                    serde_json::from_value(descriptor.clone()).unwrap();
                let resolved = declaration.resolve().unwrap();
                let stored: Option<(String, String, i64, i64)> = tx
                    .query_row(
                        "SELECT descriptor,state,attempt,deadline FROM steps WHERE id=?1 AND ordinal=?2",
                        params![id, ordinal],
                        |row| Ok((row.get(0)?, row.get(1)?,row.get(2)?,row.get(3)?)),
                    )
                    .optional()
                    .unwrap();
                if let Some((saved, state, attempt, deadline)) = stored {
                    assert_eq!(serde_json::from_str::<Value>(&saved).unwrap(), *descriptor);
                    match &resolved.config {
                        WorkflowDurableConfig::Do(policy) => {
                            let timeout = i64::try_from(policy.timeout).unwrap();
                            let config = serde_json::to_value(policy).unwrap();
                            if state == "retry_wait" {
                                if deadline > now() {
                                    grants.push(json!({"state":"suspended"}));
                                    continue;
                                }
                                let token = format!("{:064x}", ordinal + 2 + (attempt + 1) * 1024);
                                tx.execute("UPDATE steps SET state='running',token=?3,attempt=attempt+1,deadline=?4 WHERE id=?1 AND ordinal=?2",
                                    params![id,ordinal,token,now()+timeout]).unwrap();
                                grants.push(
                                    json!({"state":"run","stepToken":token,"attempt":attempt+1,
                                    "remainingMs":timeout,"config":config}),
                                );
                                continue;
                            }
                            if state == "complete" {
                                grants.push(json!({"state":"complete","attempt":attempt,
                                    "config":config}));
                            } else {
                                grants.push(json!({"state":state}));
                            }
                        }
                        WorkflowDurableConfig::WaitEvent { .. }
                            if state == "waiting" && deadline <= now() =>
                        {
                            tx.execute(
                                "UPDATE steps SET state='failed',code='WORKFLOW_EVENT_TIMEOUT' WHERE id=?1 AND ordinal=?2",
                                params![id, ordinal],
                            )
                            .unwrap();
                            grants.push(json!({"state":"failed"}));
                        }
                        WorkflowDurableConfig::Sleep(_) | WorkflowDurableConfig::SleepUntil(_)
                            if state == "waiting" && deadline <= now() =>
                        {
                            tx.execute(
                                "UPDATE steps SET state='complete' WHERE id=?1 AND ordinal=?2",
                                params![id, ordinal],
                            )
                            .unwrap();
                            grants.push(json!({"state":"complete"}));
                        }
                        _ => grants.push(json!({"state":state})),
                    }
                    continue;
                }
                match &resolved.config {
                    WorkflowDurableConfig::Do(policy) => {
                        let timeout = i64::try_from(policy.timeout).unwrap();
                        let config = serde_json::to_value(policy).unwrap();
                        let token = format!("{:064x}", ordinal + 2);
                        tx.execute(
                            "INSERT INTO steps VALUES(?1,?2,?3,'running',?4,?5,NULL,NULL,1)",
                            params![id, ordinal, descriptor.to_string(), token, now() + timeout],
                        )
                        .unwrap();
                        grants.push(json!({"state":"run","stepToken":token,"attempt":1,
                            "remainingMs":timeout,"config":config}));
                    }
                    WorkflowDurableConfig::Sleep(duration) => {
                        let deadline = now() + i64::try_from(*duration).unwrap();
                        tx.execute(
                            "INSERT INTO steps VALUES(?1,?2,?3,'waiting',NULL,?4,NULL,NULL,0)",
                            params![id, ordinal, descriptor.to_string(), deadline],
                        )
                        .unwrap();
                        grants.push(json!({"state":"suspended"}));
                    }
                    WorkflowDurableConfig::SleepUntil(timestamp) => {
                        let state = if *timestamp <= now() {
                            "complete"
                        } else {
                            "waiting"
                        };
                        tx.execute(
                            "INSERT INTO steps VALUES(?1,?2,?3,?4,NULL,?5,NULL,NULL,0)",
                            params![id, ordinal, descriptor.to_string(), state, timestamp],
                        )
                        .unwrap();
                        grants.push(json!({"state":if state == "complete" { "complete" } else { "suspended" }}));
                    }
                    WorkflowDurableConfig::WaitEvent { timeout_ms, .. } => {
                        let deadline = now() + i64::try_from(*timeout_ms).unwrap();
                        let state = if *timeout_ms == 0 {
                            "failed"
                        } else {
                            "waiting"
                        };
                        tx.execute(
                            "INSERT INTO steps VALUES(?1,?2,?3,?4,NULL,?5,NULL,?6,0)",
                            params![
                                id,
                                ordinal,
                                descriptor.to_string(),
                                state,
                                deadline,
                                if state == "failed" {
                                    Some("WORKFLOW_EVENT_TIMEOUT")
                                } else {
                                    None
                                }
                            ],
                        )
                        .unwrap();
                        grants.push(
                            json!({"state":if state == "failed" { "failed" } else { "suspended" }}),
                        );
                    }
                }
            }
            json!({"steps":grants})
        }
        "success" | "failure" | "timeout" => {
            let saved: Option<(String, Option<String>, i64, i64, String)> = tx
                .query_row(
                    "SELECT state,token,deadline,attempt,descriptor FROM steps WHERE id=?1 AND ordinal=?2",
                    params![id, ordinal],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?,row.get(3)?,row.get(4)?)),
                )
                .optional()
                .unwrap();
            match saved {
                Some((state, Some(token), deadline, attempt, descriptor))
                    if state == "running"
                        && body["stepToken"] == token
                        && body["attempt"] == attempt =>
                {
                    let code = if operation == "timeout" || deadline <= now() {
                        Some("WORKFLOW_STEP_TIMEOUT")
                    } else if operation == "failure" {
                        Some(body["code"].as_str().unwrap())
                    } else {
                        None
                    };
                    let descriptor: Value = serde_json::from_str(&descriptor).unwrap();
                    let policy = open_compute_core::workflow::WorkflowStepConfig::resolve(
                        &descriptor["config"],
                    )
                    .unwrap();
                    let retry = matches!(
                        code,
                        Some("WORKFLOW_EXECUTION_FAILED" | "WORKFLOW_STEP_TIMEOUT")
                    ) && attempt <= i64::from(policy.retries.limit);
                    let code = if !retry && code == Some("WORKFLOW_EXECUTION_FAILED") {
                        Some("WORKFLOW_STEP_RETRIES_EXHAUSTED")
                    } else {
                        code
                    };
                    let state = if retry {
                        "retry_wait"
                    } else if code.is_some() {
                        "failed"
                    } else {
                        "complete"
                    };
                    tx.execute(
                        "UPDATE steps SET state=?3,output=?4,code=?5,token=NULL WHERE id=?1 AND ordinal=?2",
                        params![
                            id,
                            ordinal,
                            state,
                            if state=="complete" {body["outputBase64"].as_str()} else {None},
                            code
                        ],
                    )
                    .unwrap();
                    if retry {
                        let delay = i64::try_from(
                            policy
                                .retries
                                .delay_after(u32::try_from(attempt).unwrap())
                                .unwrap(),
                        )
                        .unwrap();
                        tx.execute(
                            "UPDATE steps SET deadline=?3 WHERE id=?1 AND ordinal=?2",
                            params![id, ordinal, now() + delay],
                        )
                        .unwrap();
                    }
                    result(&tx, id, ordinal)
                }
                _ => json!({"errorCode":"WORKFLOW_STEP_STALE"}),
            }
        }
        "result" => result(&tx, id, ordinal),
        "yield" => {
            let active: i64 = tx
                .query_row(
                    "SELECT count(*) FROM steps WHERE id=?1 AND state='running'",
                    [id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(active, 0, "yield must drain granted siblings");
            tx.execute(
                "UPDATE runs SET state='waiting',token=NULL WHERE id=?1",
                [id],
            )
            .unwrap();
            json!({"ok":true})
        }
        _ => panic!("unknown private operation"),
    };
    tx.commit().unwrap();
    (StatusCode::OK, Json(reply))
}

fn envelope(db: &Connection, mode: &str) -> WorkflowRunRequest {
    let id = WorkflowInstanceId::generate();
    db.execute(
        "INSERT INTO runs VALUES (?1,'running',?2)",
        params![id.to_string(), "11".repeat(32)],
    )
    .unwrap();
    WorkflowRunRequest {
        fence: WorkflowFence {
            instance_id: id,
            instance_generation: 1,
            run_token: WorkflowToken::from_bytes([0x11; 32]),
        },
        external_instance_id: "public-instance".into(),
        definition_name: "probe".into(),
        created_at_ms: now(),
        payload_base64: encode_workflow_json(&json!({"mode":mode})),
        rollback: false,
        schedule: None,
    }
}

const SOURCE: &str = r#"
import { WorkflowEntrypoint } from 'cloudflare:workers';
import { NonRetryableError } from 'cloudflare:workflows';
import { connect } from 'cloudflare:sockets';
let observedPrivateGrant=false;
async function rawTcpProbe(config) {
  const payload = new Uint8Array([21, 22, 23, 24]);
  const socket = connect({ hostname: config.hostname, port: Number(config.tcpPort) }, {
    allowHalfOpen: true, secureTransport: 'off',
  });
  await socket.opened;
  const writer = socket.writable.getWriter();
  await writer.write(new TextEncoder().encode(`ECHO ${payload.byteLength}\n`));
  await writer.write(payload);
  await writer.close();
  writer.releaseLock();
  const echoed = new Uint8Array(await new Response(socket.readable).arrayBuffer());
  await socket.close();
  await socket.closed;
  let denied = false;
  const privateSocket = connect({
    hostname: config.privateHostname, port: Number(config.tcpPort),
  });
  try {
    await privateSocket.opened;
    await privateSocket.close();
  } catch {
    denied = true;
    try { await privateSocket.close(); } catch {}
  }
  if (echoed.length !== payload.length
      || !echoed.every((value, index) => value === payload[index])) {
    throw new Error('Workflow raw TCP echo mismatch');
  }
  return { bytes: echoed.length, denied };
}
function observe(value) {
  if (value && typeof value==='object'
      && ('stepToken' in value || 'runToken' in value || 'creationNonce' in value)) observedPrivateGrant=true;
}
async function hostile(step,mode) {
  const constructor=Object.getOwnPropertyDescriptor(Promise.prototype,'constructor');
  const then=Promise.prototype.then;
  Object.defineProperty(Promise.prototype,'constructor',{value:function TenantPromise(){}});
  Promise.prototype.then=function(onValue,onError){
    return then.call(this,value=>{observe(value);return typeof onValue==='function'?onValue(value):value;},onError);
  };
  let getters=0;
  try {
    if (mode==='hostileWait') {
      const event=await step.waitForEvent('approval',{type:'approved',timeout:86400000});
      return {observedPrivateGrant,date:event.timestamp instanceof Date,payload:event.payload};
    }
    if (mode==='hostileRetry') {
      const attempt=await step.do('retry',{retries:{limit:1,delay:0}},ctx=>{
        if(ctx.attempt===1)throw new Error('private retry');return ctx.attempt;
      });
      return {observedPrivateGrant,attempt};
    }
    const values=await Promise.all([
      step.do('thenable',()=>({then(resolve){resolve(7);}})),
      step.do('json-hook',()=>{
        const value={safe:8};
        Object.defineProperty(value,'toJSON',{value(){getters++;throw new Error('private JSON hook');}});
        return value;
      }),
    ]);
    try {
      await step.do('hostile-error',{retries:{limit:0,delay:0}},()=>{
        throw Object.defineProperties(new Error(),{
          message:{get(){getters++;throw new Error('private message');}},
          name:{get(){getters++;throw new Error('private name');}},
          stack:{get(){getters++;throw new Error('private stack');}},
        });
      });
    } catch {}
    return {observedPrivateGrant,getters,values};
  } finally {
    Object.defineProperty(Promise.prototype,'constructor',constructor);
    Promise.prototype.then=then;
  }
}
export class Flow extends WorkflowEntrypoint {
  async run(event,step) {
    const mode=event.payload.mode;
    if (mode==='rawTcp') return rawTcpProbe(event.payload);
    if (mode.startsWith('hostile')) return hostile(step,mode);
    if (mode==='sleep') { await step.sleep('long',86400000); return 'awake'; }
    if (mode==='catch') {
      try { await step.sleep('long',86400000); } catch {}
      try { await step.do('forbidden',()=> 'not allowed'); } catch {}
      return 'forged success';
    }
    if (mode==='forged') { const error=new Error('suspension'); error.name='WorkflowSuspension'; throw error; }
    if (mode==='oversizedFinal') return 'x'.repeat(1024*1024);
    if (mode==='oversizedStep' || mode==='forgedSerialization') {
      try { await step.do('serialize',()=>mode==='oversizedStep' ? 'x'.repeat(1024*1024)
        : {get value() { throw new Error('WORKFLOW_RESULT_TOO_LARGE'); }}); } catch {}
      return 'caught serialization must remain latched';
    }
    if (mode==='context') return step.do('context',context=>({frozen:Object.isFrozen(context)
      && Object.isFrozen(context.step) && Object.isFrozen(context.config) && Object.isFrozen(context.config.retries),
      attempt:context.attempt}));
    if (mode==='nonretryable') {
      try { await step.do('fatal',()=>{throw new NonRetryableError('private secret');}); }
      catch(error) { return { native:error instanceof NonRetryableError,name:error.name,message:error.message }; }
    }
    if (['timeout','lateResolve','lateReject'].includes(mode)) {
      try { await step.do('timed',{timeout:50,retries:{limit:0,delay:0}},async()=>{
        if (mode==='timeout') return new Promise(()=>{});
        await new Promise(resolve=>setTimeout(resolve,150));
        if (mode==='lateReject') throw new Error('private late rejection');
        return 'late success';
      }); }
      catch(error) { return { timeout:error.message==='WORKFLOW_STEP_TIMEOUT' }; }
    }
    if (mode==='parallel') {
      return Promise.all([90,10,40,20].map((delay,index)=>step.do(`p${index}`,async()=>{
        await new Promise(resolve=>setTimeout(resolve,delay)); return index;
      })));
    }
    return step.do('normal',()=>7);
  }
}
export default { fetch(){return new Response('ordinary');} };
"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workflow_runtime_suspension_timeout_parallel_and_native_errors() {
    let mut harness = Harness::start().await;
    let db = Connection::open(
        harness
            .storage
            .data_dir()
            .root()
            .join("durable-waiting-probe.sqlite"),
    )
    .unwrap();
    db.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
        CREATE TABLE runs(id TEXT PRIMARY KEY,state TEXT NOT NULL,token TEXT);
        CREATE TABLE steps(id TEXT NOT NULL REFERENCES runs(id),ordinal INTEGER NOT NULL,descriptor TEXT NOT NULL,
            state TEXT NOT NULL,token TEXT,deadline INTEGER NOT NULL,output TEXT,code TEXT,attempt INTEGER NOT NULL,PRIMARY KEY(id,ordinal));").unwrap();
    let db = Arc::new(Mutex::new(db));
    let backend = Backend {
        auth: harness.binding_auth.clone(),
        db: db.clone(),
    };
    let listener = harness.binding_listener.take().unwrap();
    let mut shutdown = harness.shutdown.subscribe();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route("/internal/workflows/runs/{operation}", post(handle))
                .with_state(backend),
        )
        .with_graceful_shutdown(async move {
            let _ = shutdown.changed().await;
        })
        .await
        .unwrap();
    });
    let target = harness.deploy(SOURCE, "Flow").await;
    let scheduler = Arc::new(
        SchedulerStore::open(
            &harness.storage.data_dir().ensure_scheduler_db().unwrap(),
            5000,
            now(),
        )
        .unwrap(),
    );
    let account = harness.storage.identity().default_account_id;
    let repository = WorkflowRepository::new(harness.storage.db());
    let definition = repository
        .create_definition(account, "probe", now())
        .unwrap();
    let api = WorkflowApiState::new(
        harness.storage.clone(),
        scheduler,
        harness.transport.clone(),
        Default::default(),
    );
    let current = api
        .create_version(account, definition.id, target.deployment_id, "Flow".into())
        .await
        .unwrap();
    assert_eq!(current.state, DeploymentState::Ready);
    assert_eq!(current.target.capability_version, 1);
    assert_eq!(
        repository
            .version(account, current.target.version_id)
            .unwrap()
            .target,
        current.target
    );
    let version = current.target;
    harness.transport.probe_workflow(&version).await.unwrap();
    if let Some(payload) = raw_tcp_fixture_payload() {
        let mut request = envelope(&db.lock().unwrap(), "rawTcp");
        request.payload_base64 = encode_workflow_json(&payload);
        let response = harness
            .transport
            .dispatch_workflow(&version, &request, Duration::from_secs(10))
            .await
            .unwrap();
        let WorkflowOutcome::Complete { output_base64, .. } = response.result else {
            panic!("expected raw TCP Workflow completion");
        };
        assert_eq!(
            decode_workflow_json(&output_base64),
            json!({"bytes": 4.0, "denied": true})
        );
    }
    for mode in ["hostileWait", "hostileRetry"] {
        let mut request = envelope(&db.lock().unwrap(), mode);
        let first = harness
            .transport
            .dispatch_workflow(&version, &request, Duration::from_secs(10))
            .await;
        assert!(
            first.is_ok(),
            "mode={mode}: {first:?} {:?}",
            harness.supervisor.last_diagnostics()
        );
        let first = first.unwrap();
        assert!(matches!(first.result, WorkflowOutcome::Suspended { .. }));
        assert!(!first.drain_incomplete);
        {
            let db = db.lock().unwrap();
            if mode == "hostileWait" {
                let event = encode_workflow_json(
                    &json!({"type":"approved","payload":7,"timestampMs":now()}),
                );
                db.execute(
                    "UPDATE steps SET state='complete',output=?2 WHERE id=?1 AND state='waiting'",
                    params![request.fence.instance_id.to_string(), event],
                )
                .unwrap();
            }
            assert_eq!(
                db.execute(
                    "UPDATE runs SET state='running',token=?2 WHERE id=?1 AND state='waiting'",
                    params![request.fence.instance_id.to_string(), "33".repeat(32)]
                )
                .unwrap(),
                1
            );
        }
        request.fence.run_token = WorkflowToken::from_bytes([0x33; 32]);
        let replay = harness
            .transport
            .dispatch_workflow(&version, &request, Duration::from_secs(10))
            .await
            .unwrap();
        let WorkflowOutcome::Complete { output_base64, .. } = replay.result else {
            panic!("expected replay completion");
        };
        let output = decode_workflow_json(&output_base64);
        assert_eq!(output["observedPrivateGrant"], false, "{mode}");
        if mode == "hostileWait" {
            assert_eq!(output["date"], true);
            assert_eq!(output["payload"], 7.0);
        } else {
            assert_eq!(output["attempt"], 2.0);
        }
    }
    let mut sleeping = None;
    for mode in [
        "normal",
        "sleep",
        "catch",
        "forged",
        "oversizedFinal",
        "oversizedStep",
        "forgedSerialization",
        "context",
        "parallel",
        "nonretryable",
        "hostile",
        "lateResolve",
        "lateReject",
        "timeout",
    ] {
        let request = envelope(&db.lock().unwrap(), mode);
        let started = Instant::now();
        let response = harness
            .transport
            .dispatch_workflow(&version, &request, Duration::from_secs(40))
            .await;
        assert!(
            response.is_ok(),
            "mode={mode}: {response:?} {:?}",
            harness.supervisor.last_diagnostics()
        );
        let response = response.unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(if mode == "timeout" { 35 } else { 3 }),
            "long waiting must end the actual dispatch RPC"
        );
        match (&response.result, mode) {
            (WorkflowOutcome::Suspended { final_ordinal }, "sleep" | "catch") => {
                assert_eq!(*final_ordinal, 1);
                let db = db.lock().unwrap();
                let (state, token): (String, Option<String>) = db
                    .query_row(
                        "SELECT state,token FROM runs WHERE id=?1",
                        [request.fence.instance_id.to_string()],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .unwrap();
                assert_eq!(state, "waiting");
                assert!(token.is_none());
                let count: i64 = db
                    .query_row(
                        "SELECT count(*) FROM steps WHERE id=?1",
                        [request.fence.instance_id.to_string()],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(count, 1, "catch cannot acquire another grant");
                assert!(!response.drain_incomplete);
                if mode == "sleep" {
                    sleeping = Some(request);
                }
            }
            (WorkflowOutcome::Errored { error_code, .. }, "forged") => {
                assert_eq!(error_code, "WORKFLOW_EXECUTION_FAILED");
            }
            (WorkflowOutcome::Errored { error_code, .. }, "oversizedFinal" | "oversizedStep") => {
                assert_eq!(error_code, "WORKFLOW_RESULT_TOO_LARGE");
            }
            (WorkflowOutcome::Errored { error_code, .. }, "forgedSerialization") => {
                assert_eq!(error_code, "WORKFLOW_SERIALIZATION_UNSUPPORTED");
            }
            (WorkflowOutcome::Complete { output_base64, .. }, _) => {
                let output = decode_workflow_json(output_base64);
                match mode {
                    "normal" => assert_eq!(output, 7.0),
                    "context" => assert_eq!(output, json!({"frozen":true,"attempt":1.0})),
                    "parallel" => assert_eq!(output, json!([0.0, 1.0, 2.0, 3.0])),
                    "nonretryable" => assert_eq!(
                        output,
                        json!({"native":true,"name":"NonRetryableError","message":"Workflow step is not retryable"})
                    ),
                    "hostile" => assert_eq!(
                        output,
                        json!({"observedPrivateGrant":false,"getters":0.0,"values":[7.0,{"safe":8.0}]})
                    ),
                    "lateResolve" | "lateReject" => {
                        assert_eq!(output, json!({"timeout":true}));
                        assert!(!response.drain_incomplete);
                        let (state, output, code): (String, Option<String>, String) = db
                            .lock()
                            .unwrap()
                            .query_row(
                                "SELECT state,output,code FROM steps WHERE id=?1",
                                [request.fence.instance_id.to_string()],
                                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                            )
                            .unwrap();
                        assert_eq!(state, "failed");
                        assert!(output.is_none());
                        assert_eq!(code, "WORKFLOW_STEP_TIMEOUT");
                    }
                    _ => panic!("unexpected completion"),
                }
            }
            (WorkflowOutcome::Unknown { .. }, "timeout") => {
                assert!(response.drain_incomplete);
                assert!(started.elapsed() >= Duration::from_secs(30));
                // Logical timeout is persisted, but a non-drained invocation
                // neither yields nor terminalizes the instance.
                let state: String = db
                    .lock()
                    .unwrap()
                    .query_row(
                        "SELECT state FROM runs WHERE id=?1",
                        [request.fence.instance_id.to_string()],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(state, "running");
            }
            (WorkflowOutcome::Errored { error_code, .. }, _) => {
                panic!("unexpected mode={mode}: {error_code}")
            }
            _ => panic!("unexpected mode={mode}: {:?}", response.result),
        }
    }
    // Repeated calls after incomplete drain must not grow uncounted background
    // invocations. The transport quarantine is shared by all of its clones.
    let quarantined = envelope(&db.lock().unwrap(), "normal");
    for _ in 0..4 {
        assert!(
            harness
                .transport
                .clone()
                .dispatch_workflow(&version, &quarantined, Duration::from_secs(10))
                .await
                .is_err()
        );
    }
    let count: i64 = db
        .lock()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM steps WHERE id=?1",
            [quarantined.fence.instance_id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        count, 0,
        "quarantine must refuse before starting another loaded invocation"
    );
    // Keep the absolute waiting deadline while discarding the complete loaded
    // isolate. This fixture supplies the due promotion; product tests must prove
    // the same transition through the production scheduler and migrations.
    let mut sleeping = sleeping.unwrap();
    let id = sleeping.fence.instance_id.to_string();
    let due: i64 = db
        .lock()
        .unwrap()
        .query_row("SELECT deadline FROM steps WHERE id=?1", [&id], |row| {
            row.get(0)
        })
        .unwrap();
    harness.restart().await;
    {
        let db = db.lock().unwrap();
        db.execute(
            "UPDATE steps SET state='complete',output=NULL WHERE id=?1 AND state='waiting'",
            [&id],
        )
        .unwrap();
        db.execute(
            "UPDATE runs SET state='running',token=?2 WHERE id=?1",
            params![&id, "33".repeat(32)],
        )
        .unwrap();
    }
    sleeping.fence.run_token = WorkflowToken::from_bytes([0x33; 32]);
    let replay = harness
        .transport
        .dispatch_workflow(&version, &sleeping, Duration::from_secs(10))
        .await
        .unwrap();
    assert_eq!(replay.loader_outcome, "cold");
    assert!(
        matches!(replay.result,WorkflowOutcome::Complete{ref output_base64,..} if decode_workflow_json(output_base64)==json!("awake"))
    );
    assert_eq!(
        db.lock()
            .unwrap()
            .query_row("SELECT deadline FROM steps WHERE id=?1", [&id], |row| row
                .get::<_, i64>(
                0
            ))
            .unwrap(),
        due
    );
    harness.stop().await;
    server.await.unwrap();
}
