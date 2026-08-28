//! Batch admission, illegal overlap, and bounded large replay on the real V2 path.

use super::{Harness, now, start_backend};
use open_compute_core::{MetricsConfig, SchedulerConfig, WorkflowsConfig};
use open_compute_service::{
    metrics::MetricsRegistry, scheduler::SchedulerService, workflow_http::WorkflowApiState,
};
use open_compute_storage::{SchedulerStore, WorkflowRepository};
use open_compute_workers::{WorkflowController, WorkflowCreateInput};
use serde_json::{Value, json};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn production_batches_enforce_join_limits_and_replay_large_outputs() {
    let mut harness = Harness::start().await;
    let store = Arc::new(
        SchedulerStore::open(
            &harness.storage.data_dir().ensure_scheduler_db().unwrap(),
            5000,
            now(),
        )
        .unwrap(),
    );
    let limits = WorkflowsConfig {
        max_parallel_steps: 16,
        ..Default::default()
    };
    let metrics =
        Arc::new(MetricsRegistry::new(&MetricsConfig::default(), "test", "test").unwrap());
    let backend = start_backend(&mut harness, &store, &limits, &metrics);
    let target = harness.deploy(SOURCE, "Flow").await;
    let account = harness.storage.identity().default_account_id;
    let definition = WorkflowRepository::new(harness.storage.db())
        .create_definition(account, "batch-admission", now())
        .unwrap();
    WorkflowApiState::new(
        harness.storage.clone(),
        store.clone(),
        harness.transport.clone(),
        limits.clone(),
    )
    .create_version(
        account,
        definition.id,
        target.deployment_id,
        "Flow".into(),
        2,
    )
    .await
    .unwrap();
    let controller = WorkflowController::new(&harness.storage, &store, &limits);
    let mut instances = Vec::new();
    for mode in [
        "nested", "overlap", "unjoined", "overflow", "joined", "large",
    ] {
        let input = json!({"mode":mode}).to_string();
        let identity = controller
            .create(
                account,
                definition.id,
                2,
                Some(mode),
                WorkflowCreateInput {
                    payload_json: &input,
                    retention: None,
                },
                now(),
            )
            .unwrap();
        if mode == "large" {
            controller
                .send_event(
                    account,
                    definition.id,
                    identity.instance_id,
                    "ready",
                    &json!("e".repeat(1024 * 1024 - 100)).to_string(),
                    now(),
                )
                .unwrap();
        }
        instances.push((mode, identity.instance_id));
    }
    let service = Arc::new(
        SchedulerService::new(
            store.clone(),
            harness.storage.clone(),
            harness.transport.clone(),
            SchedulerConfig::default(),
            limits,
            Arc::new(open_compute_core::SystemSchedulerClock),
        )
        .with_metrics(metrics),
    );
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let kernel = tokio::spawn(service.run(stopped));
    for (mode, id) in instances {
        let deadline = Instant::now() + Duration::from_secs(40);
        let record = loop {
            let record = store.workflow_instance(id).unwrap().unwrap();
            if record.state.is_terminal() {
                break record;
            }
            assert!(
                Instant::now() < deadline,
                "batch did not settle: {mode}: {record:?}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        if matches!(mode, "joined" | "large") {
            assert!(
                record.error_code.is_none(),
                "{mode}: {:?}",
                record.error_code
            );
            let output: Value =
                serde_json::from_str(record.output_json.as_deref().unwrap()).unwrap();
            if mode == "joined" {
                assert_eq!(
                    output,
                    json!({"peak":4,"statuses":["fulfilled","rejected","fulfilled","fulfilled"],"next":7})
                );
            } else {
                assert_eq!(
                    output,
                    json!({"lengths":vec![1024*1024-100;16],"calls":0,"eventLength":1024*1024-100})
                );
            }
        } else {
            assert_eq!(
                record.error_code.as_deref(),
                Some("WORKFLOW_PARALLEL_STEP_UNSUPPORTED"),
                "{mode}"
            );
            if mode == "overflow" {
                assert_eq!(record.durable.as_ref().unwrap().registered_step_count, 0);
            }
        }
        store.verify_workflow_history(id).unwrap();
    }
    stop.send(true).unwrap();
    kernel.await.unwrap().unwrap();
    harness.stop().await;
    backend.await.unwrap().unwrap();
}

const SOURCE: &str = r#"
import {WorkflowEntrypoint} from 'cloudflare:workers';
import {NonRetryableError} from 'cloudflare:workflows';
export class Flow extends WorkflowEntrypoint {
  async run(event,step) {
    const mode=event.payload.mode;
    if(mode==='nested') return step.do('outer',async()=>{await Promise.resolve();return step.do('inner',()=>1);});
    if(mode==='overlap') {
      const running=step.do('first',()=>1);
      try {await step.sleep('overlap',1);} catch {}
      await running; return 'must remain latched';
    }
    if(mode==='unjoined') {step.do('forgotten',()=>1);return 'must fail';}
    if(mode==='overflow') return Promise.all(Array.from({length:17},(_,index)=>step.do(`p${index}`,()=>1)));
    if(mode==='joined') {
      let active=0,peak=0;
      const values=await Promise.allSettled(Array.from({length:4},(_,index)=>step.do(`p${index}`,async()=>{
        active++;peak=Math.max(peak,active);await new Promise(resolve=>setTimeout(resolve,20));active--;
        if(index===1) throw new NonRetryableError('private failure');return index;
      })));
      return {peak,statuses:values.map(value=>value.status),next:await step.do('next',()=>7)};
    }
    let calls=0;
    const values=await Promise.all(Array.from({length:16},(_,index)=>step.do(`large${index}`,()=>{calls++;return 'x'.repeat(1024*1024-100);} )));
    await step.sleep('yield',1);
    const signal=await step.waitForEvent('event',{type:'ready',timeout:'1 minute'});
    return {lengths:values.map(value=>value.length),calls,eventLength:signal.payload.length};
  }
}
export default {fetch(){return new Response('ready');}};
"#;
