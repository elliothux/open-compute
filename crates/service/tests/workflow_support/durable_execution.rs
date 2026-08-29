//! Current caller, private backend, scheduler driver and stock-workerd execution.

use super::{Harness, now};
use open_compute_core::{
    BindingKind, MetricsConfig, ResourceId, SchedulerConfig, WorkflowInstanceId, WorkflowsConfig,
};
use open_compute_service::{
    metrics::MetricsRegistry, scheduler::SchedulerService, workflow_http::WorkflowApiState,
};
use open_compute_storage::scheduler::WorkflowState;
use open_compute_storage::{SchedulerStore, WorkflowRefState, WorkflowRepository};
use open_compute_workers::DeploymentBindingInput;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, Instant},
};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn production_driver_replays_waits_retries_and_events_after_runtime_restart() {
    let mut harness = Harness::start().await;
    let store = Arc::new(
        SchedulerStore::open(
            &harness.storage.data_dir().ensure_scheduler_db().unwrap(),
            5000,
            now(),
        )
        .unwrap(),
    );
    let limits = WorkflowsConfig::default();
    let metrics =
        Arc::new(MetricsRegistry::new(&MetricsConfig::default(), "test", "test").unwrap());
    let server = super::start_backend(&mut harness, &store, &limits, &metrics);
    let flow = harness.deploy(FLOW, "Flow").await;
    let account = harness.storage.identity().default_account_id;
    let repository = WorkflowRepository::new(harness.storage.db());
    let definition = repository
        .create_definition(account, "durable-execution", now())
        .unwrap();
    let api = WorkflowApiState::new(
        harness.storage.clone(),
        store.clone(),
        harness.transport.clone(),
        Default::default(),
    );
    api.create_version(account, definition.id, flow.deployment_id, "Flow".into())
        .await
        .unwrap();
    let caller = harness
        .deploy_bound(
            CALLER,
            "",
            BTreeMap::from([(
                "FLOW".into(),
                DeploymentBindingInput {
                    kind: BindingKind::Workflow,
                    id: ResourceId::from_uuid(definition.id.as_uuid()).unwrap(),
                    permissions: Default::default(),
                    config: Default::default(),
                },
            )]),
        )
        .await;
    assert_eq!(
        request(&harness, &caller, "create").await,
        json!({"id":"durable","status":"queued"})
    );
    let identity = repository
        .find_instance(definition.id, "durable")
        .unwrap()
        .identity;
    let service = Arc::new(
        SchedulerService::new(
            store.clone(),
            harness.storage.clone(),
            harness.transport.clone(),
            SchedulerConfig::default(),
            limits.clone(),
            Arc::new(open_compute_core::SystemSchedulerClock),
        )
        .with_metrics(metrics.clone()),
    );
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let kernel = tokio::spawn(service.run(stopped));
    wait_event_wait(&store, identity.instance_id, &metrics).await;
    assert_eq!(
        repository
            .reservation(identity.instance_id)
            .unwrap()
            .unwrap()
            .state,
        WorkflowRefState::Live
    );
    let before = store
        .workflow_instance(identity.instance_id)
        .unwrap()
        .unwrap();
    assert_eq!(before.durable.registered_step_count, 4);
    assert!(before.run_token.is_none());
    assert_eq!(
        request(&harness, &caller, "pause").await,
        json!({"ok":true})
    );
    assert_eq!(
        request(&harness, &caller, "status").await,
        json!({"status":"paused"})
    );
    stop.send(true).unwrap();
    kernel.await.unwrap().unwrap();
    harness.restart().await;
    let service = Arc::new(
        SchedulerService::new(
            store.clone(),
            harness.storage.clone(),
            harness.transport.clone(),
            SchedulerConfig::default(),
            limits,
            Arc::new(open_compute_core::SystemSchedulerClock),
        )
        .with_metrics(metrics.clone()),
    );
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let kernel = tokio::spawn(service.run(stopped));
    assert_eq!(request(&harness, &caller, "send").await, json!({"ok":true}));
    assert_eq!(
        request(&harness, &caller, "status").await,
        json!({"status":"paused"})
    );
    assert_eq!(
        request(&harness, &caller, "resume").await,
        json!({"ok":true})
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let status = request(&harness, &caller, "status").await;
        if status["status"] == "complete" {
            assert_eq!(
                status["output"],
                json!({"initial":1,"event":{"approved":true},"retryAttempt":2,"second":2,"nonRetryable":true,"eventTimeout":true,"timestamp":true})
            );
            break;
        }
        assert_ne!(status["status"], "errored", "{status}");
        assert!(
            Instant::now() < deadline,
            "Workflow driver did not finish: {status}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    stop.send(true).unwrap();
    kernel.await.unwrap().unwrap();
    let complete = store
        .workflow_instance(identity.instance_id)
        .unwrap()
        .unwrap();
    assert_eq!(complete.state, WorkflowState::Complete);
    assert_eq!(complete.identity, identity);
    assert_eq!(complete.durable.retention.success_retention_ms, 3_600_000);
    assert_eq!(
        complete.durable.expires_at_ms,
        complete.terminal_at_ms.map(|time| time + 3_600_000)
    );
    assert_eq!(
        repository
            .reservation(identity.instance_id)
            .unwrap()
            .unwrap()
            .state,
        WorkflowRefState::Retained
    );
    assert!(repository.instance_referrers_intact(&identity).unwrap());
    store.verify_workflow_history(identity.instance_id).unwrap();
    let rendered =
        metrics.render(&open_compute_service::health::HealthCoordinator::new().snapshot());
    assert!(rendered.contains("open_compute_workflow_in_flight 0"));
    assert!(rendered.contains("open_compute_workflow_runs_total{outcome=\"unknown\"} 0"));
    assert_eq!(
        request(&harness, &caller, "restart").await,
        json!({"ok":true})
    );
    let restarted = store
        .workflow_instance(identity.instance_id)
        .unwrap()
        .unwrap();
    assert_eq!(restarted.state, WorkflowState::Queued);
    assert_eq!(
        restarted.identity.instance_generation,
        identity.instance_generation + 1
    );
    assert_eq!(restarted.identity.target, identity.target);
    assert_eq!(
        request(&harness, &caller, "terminate").await,
        json!({"ok":true})
    );
    assert_eq!(
        request(&harness, &caller, "status").await,
        json!({"status":"terminated"})
    );
    store.verify_workflow_history(identity.instance_id).unwrap();
    harness.stop().await;
    server.await.unwrap().unwrap();
}

async fn wait_event_wait(
    store: &SchedulerStore,
    id: WorkflowInstanceId,
    metrics: &MetricsRegistry,
) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let record = store.workflow_instance(id).unwrap().unwrap();
        let rendered =
            metrics.render(&open_compute_service::health::HealthCoordinator::new().snapshot());
        if record.state == WorkflowState::Waiting
            && record.durable.registered_step_count == 4
            && rendered.contains("open_compute_workflow_in_flight 0")
        {
            return;
        }
        assert!(
            !record.state.is_terminal(),
            "unexpected terminal: {:?} {:?}",
            record.state,
            record.error_code
        );
        assert!(
            Instant::now() < deadline,
            "Workflow did not release its wait activation: {record:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn request(
    harness: &Harness,
    target: &open_compute_service::runtime_bridge::DispatchTarget,
    operation: &str,
) -> Value {
    let mut target = target.clone();
    target.entrypoint = None;
    let response = tokio::time::timeout(
        Duration::from_secs(10),
        harness.transport.dispatch(
            target,
            axum::http::Request::builder()
                .method("POST")
                .uri(format!("https://workflow.invalid/{operation}"))
                .header("host", "workflow.invalid")
                .body(axum::body::Body::empty())
                .unwrap(),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 65536)
            .await
            .unwrap(),
    )
    .unwrap()
}

const CALLER: &str = r#"
export default {async fetch(request,env) {
  const operation=new URL(request.url).pathname;
  if(operation==='/create') {
    const instance=await env.FLOW.create({id:'durable',params:{seed:1},retention:{successRetention:'1 hour',errorRetention:'2 hours'}});
    return Response.json({id:instance.id,...await instance.status()});
  }
  const instance=await env.FLOW.get('durable');
  if(['/pause','/resume','/terminate','/restart'].includes(operation)) {await instance[operation.slice(1)]();return Response.json({ok:true});}
  if(operation==='/send') {await instance.sendEvent({type:'approval',payload:{approved:true}});return Response.json({ok:true});}
  return Response.json(await instance.status());
}};
"#;
const FLOW: &str = r#"
import {WorkflowEntrypoint} from 'cloudflare:workers';
import {NonRetryableError} from 'cloudflare:workflows';
export class Flow extends WorkflowEntrypoint {async run(event,step) {
  const initial=await step.do('initial',async context=>event.payload.seed*context.attempt);
  await step.sleep('sleep','150 ms');
  let eventTimeout=false;
  try {await step.waitForEvent('expired',{type:'unused',timeout:0});} catch {eventTimeout=true;}
  const received=await step.waitForEvent('approval',{type:'approval',timeout:'30 seconds'});
  const [retryAttempt,second]=await Promise.all([
    step.do('retry',{timeout:'2 seconds',retries:{limit:1,delay:'100 ms'}},async context=>{if(context.attempt===1)throw new Error('private callback error');return context.attempt;}),
    step.do('second',async()=>2),
  ]);
  let nonRetryable=false;
  try {await step.do('nonretryable',async()=>{throw new NonRetryableError('private error');});}
  catch(error) {nonRetryable=error instanceof NonRetryableError;}
  return {initial,event:received.payload,retryAttempt,second,nonRetryable,eventTimeout,timestamp:received.timestamp instanceof Date};
}}
export default {fetch(){return new Response('flow');}};
"#;
