//! Workflow callbacks use the real product facades without acquiring host authority.

use super::*;
use open_compute_storage::{PlatformStorage, WorkerRepository};
use open_compute_workers::{
    BundleLimits, CanonicalBundle, CreateDeploymentRequest, CreateQueueOutcome, CreateQueueRequest,
    DeploymentController, ModuleInput, ModuleType, QueueController,
};
use p0_exit_support::{
    GateStack, admin_json, admin_router, deploy, open_scheduler, repo_root, storage_config, stores,
};
use serde_json::{Value, json};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workflow_step_uses_kv_d1_r2_do_queue_and_replay_preserves_external_effects() {
    let root = repo_root();
    let temp = tempfile::Builder::new()
        .prefix("workflow-products-")
        .tempdir_in(root.join(".temp/workflow-run"))
        .unwrap();
    let evidence = process_crash::Evidence(Some(temp));
    let temp = evidence.0.as_ref().unwrap();
    let storage = Arc::new(
        PlatformStorage::bootstrap(&storage_config(&temp.path().join("data")), &SystemClock)
            .unwrap(),
    );
    let scheduler = open_scheduler(&storage);
    let mock = open_compute_artifacts::MockS3::spawn("open-compute").await;
    let (artifacts, objects) = stores(&mock);
    let pins = ResourcePins::new();
    let stack = GateStack::start(
        storage.clone(),
        scheduler.clone(),
        artifacts.clone(),
        objects.clone(),
        pins.clone(),
        std::env::var_os("OPEN_COMPUTE_TEST_WORKERD")
            .unwrap()
            .into(),
        root.join("packages/runtime/workerd.lock.json"),
        root.join("packages/runtime"),
        "workflow-products",
    )
    .await;
    let api = admin_router(
        storage.clone(),
        artifacts.clone(),
        objects,
        pins,
        &stack,
        scheduler.clone(),
    );
    let account = storage.identity().default_account_id;
    let worker = WorkerRepository::new(storage.db())
        .create_worker(
            account,
            "workflow-products",
            RequestId::generate(),
            now(),
            1_000_000,
        )
        .unwrap()
        .0;
    let mut bindings = BTreeMap::new();
    for (binding, kind, path, body, nested) in [
        (
            "KV",
            BindingKind::KvNamespace,
            "kv/namespaces",
            json!({"name":"workflow-kv"}),
            false,
        ),
        (
            "R2",
            BindingKind::R2Bucket,
            "r2/buckets",
            json!({"name":"workflow-r2"}),
            true,
        ),
        (
            "DB",
            BindingKind::D1Database,
            "d1/databases",
            json!({"name":"workflow-d1"}),
            false,
        ),
        (
            "OBJECTS",
            BindingKind::DoNamespace,
            "durable-objects/namespaces",
            json!({"name":"workflow-do","workerId":worker.id,"className":"Counter"}),
            false,
        ),
    ] {
        let (status, value) = admin_json(
            &api,
            "POST",
            &format!("/v1/accounts/{account}/{path}"),
            body,
            Some(binding),
        )
        .await;
        assert!(status.is_success(), "{value}");
        let id = if nested {
            &value["bucket"]["resourceId"]
        } else {
            &value["resourceId"]
        };
        bindings.insert(
            binding.into(),
            DeploymentBindingInput {
                kind,
                id: id.as_str().unwrap().parse().unwrap(),
                permissions: Default::default(),
                config: Default::default(),
            },
        );
    }
    let CreateQueueOutcome::Applied(queue) = QueueController::new(&storage, scheduler.clone())
        .create(&CreateQueueRequest {
            account_id: account,
            name: "workflow-queue".into(),
            config: Default::default(),
            idempotency_key: "workflow-queue".into(),
            request_id: RequestId::generate(),
            now_ms: now(),
        })
        .unwrap()
    else {
        panic!("new queue must be applied");
    };
    bindings.insert(
        "QUEUE".into(),
        DeploymentBindingInput {
            kind: BindingKind::QueueProducer,
            id: ResourceId::from_uuid(queue.queue.id.as_uuid()).unwrap(),
            permissions: Default::default(),
            config: Default::default(),
        },
    );
    let bundle = CanonicalBundle::build(
        "index.js",
        vec![ModuleInput {
            name: "index.js".into(),
            module_type: ModuleType::EsModule,
            bytes: SOURCE.as_bytes().to_vec(),
        }],
        BundleLimits::default(),
    )
    .unwrap();
    let controller = DeploymentController::new(
        &storage,
        artifacts,
        Arc::new(stack.transport.clone()),
        BundleLimits::default(),
    )
    .with_product_promoter(open_compute_service::product_promotion_for_test(
        storage.clone(),
        scheduler.clone(),
    ));
    let deployment = deploy(
        &controller,
        CreateDeploymentRequest {
            account_id: account,
            worker_id: worker.id,
            idempotency_key: "workflow-products".into(),
            bundle: bundle.into_bytes().into(),
            compatibility_date: "2026-08-22".into(),
            compatibility_flags: vec!["rpc".into()],
            vars: Default::default(),
            secrets: Default::default(),
            bindings,
            queue_consumers: vec![open_compute_workers::QueueConsumerInput {
                queue: queue.queue.id,
                entrypoint: None,
                config: Default::default(),
                dead_letter_queue: None,
            }],
            crons: Some(vec!["* * * * *".into()]),
            limits: json!({"profile":"default"}),
            promote: true,
            request_id: RequestId::generate(),
            now_ms: now(),
        },
        &stack.supervisor,
    )
    .await;
    let definition = WorkflowRepository::new(storage.db())
        .create_definition(account, "workflow-products", now())
        .unwrap();
    let version = WorkflowApiState::new(
        storage.clone(),
        scheduler.clone(),
        stack.transport.clone(),
        Default::default(),
    )
    .create_version(account, definition.id, deployment.id, "Flow".into())
    .await
    .unwrap();
    let config = WorkflowsConfig::default();
    let workflow = WorkflowController::new(&storage, &scheduler, &config);
    workflow
        .create(
            account,
            definition.id,
            Some("products-instance"),
            open_compute_workers::WorkflowCreateInput {
                payload_json: "null",
                retention: None,
            },
            now(),
        )
        .unwrap();
    let run = workflow
        .claim(now(), &mut Default::default())
        .unwrap()
        .unwrap();
    let target = version.target;
    let envelope = WorkflowRunRequest {
        fence: run.fence.clone(),
        external_instance_id: run.external_instance_id,
        definition_name: run.target.definition_name,
        created_at_ms: run.created_at_ms,
        payload_json: run.input_json,
    };
    let result = stack
        .transport
        .dispatch_workflow(
            &target,
            &envelope,
            Duration::from_millis(config.dispatch_timeout_ms),
        )
        .await
        .unwrap();
    let (_, output_json) = complete(result);
    let before: Value = serde_json::from_str(&output_json).unwrap();
    assert!(
        before["value"].get("failure").is_none(),
        "product facade failure: {before}"
    );
    assert_eq!(before["calls"], 1);
    assert_eq!(before["value"]["kv"], "stored");
    assert_eq!(before["value"]["r2"], "stored");
    assert_eq!(before["value"]["d1"], 7);
    assert_eq!(before["value"]["object"], 1);
    assert_eq!(
        before["value"]["context"],
        json!({"attempt":1,"step":{"name":"products","count":1},"config":{
            "retries":{"limit":5,"delay":10000,"backoff":"exponential"},"timeout":60000
        }})
    );
    let expired = now() + i64::try_from(config.lease_ms + 1).unwrap();
    scheduler.recover_workflows(expired, &config, 32).unwrap();
    let replay = workflow
        .claim(expired + 1000, &mut Default::default())
        .unwrap()
        .unwrap();
    let envelope = WorkflowRunRequest {
        fence: replay.fence.clone(),
        external_instance_id: replay.external_instance_id,
        definition_name: replay.target.definition_name,
        created_at_ms: replay.created_at_ms,
        payload_json: replay.input_json,
    };
    let replay_result = stack
        .transport
        .dispatch_workflow(
            &target,
            &envelope,
            Duration::from_millis(config.dispatch_timeout_ms),
        )
        .await
        .unwrap();
    let (final_ordinal, replay_output) = complete(replay_result);
    let after: Value = serde_json::from_str(&replay_output).unwrap();
    assert_eq!(after["calls"], 0);
    assert_eq!(after["value"], before["value"]);
    assert_eq!(
        scheduler
            .queue_metrics(queue.queue.id, 1, 1)
            .unwrap()
            .backlog_count,
        1
    );
    scheduler
        .finish_workflow(
            &replay.fence,
            &WorkflowCompletion::Complete {
                output_json: replay_output,
                final_ordinal,
            },
            expired + 1001,
            &config,
        )
        .unwrap();
    workflow
        .reconcile(&mut WorkflowReconcileCursor::default(), 32, expired + 1002)
        .unwrap();
    // A crash after an external effect but before its step result is committed must
    // repeat the callback. Its stable instance/name/count key deduplicates DO state.
    workflow
        .create(
            account,
            definition.id,
            Some("external-effect"),
            open_compute_workers::WorkflowCreateInput {
                payload_json: "{\"crashBeforeCommit\":true}",
                retention: None,
            },
            now(),
        )
        .unwrap();
    let interrupted = workflow
        .claim(now(), &mut Default::default())
        .unwrap()
        .unwrap();
    let request = WorkflowRunRequest {
        fence: interrupted.fence.clone(),
        external_instance_id: interrupted.external_instance_id,
        definition_name: interrupted.target.definition_name,
        created_at_ms: interrupted.created_at_ms,
        payload_json: interrupted.input_json,
    };
    let dispatch_timeout = Duration::from_millis(config.dispatch_timeout_ms);
    let observation = tokio::spawn({
        let transport = stack.transport.clone();
        let target = target.clone();
        async move {
            transport
                .dispatch_workflow(&target, &request, dispatch_timeout)
                .await
        }
    });
    let generation = WorkerRepository::new(storage.db())
        .get_worker(account, worker.id)
        .unwrap()
        .route_generation;
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let response = p0_exit_support::dispatch(
            &stack.transport,
            account,
            worker.id,
            &deployment,
            generation,
            "/effect",
        )
        .await;
        if response.body == "\"ready\"" {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "external effect did not commit"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        scheduler
            .workflow_instance(interrupted.fence.instance_id)
            .unwrap()
            .unwrap()
            .completed_step_count,
        0
    );
    let pid = stack.supervisor.snapshot().pid.unwrap();
    p0_exit_support::kill_workerd(pid);
    assert!(observation.await.unwrap().is_err());
    p0_exit_support::wait_pid_change(&stack.supervisor, pid, Duration::from_secs(30)).await;
    let expired = now() + i64::try_from(config.lease_ms + 1).unwrap();
    scheduler.recover_workflows(expired, &config, 32).unwrap();
    let retry = workflow
        .claim(expired + 1000, &mut Default::default())
        .unwrap()
        .unwrap();
    let request = WorkflowRunRequest {
        fence: retry.fence.clone(),
        external_instance_id: retry.external_instance_id,
        definition_name: retry.target.definition_name,
        created_at_ms: retry.created_at_ms,
        payload_json: retry.input_json,
    };
    let retried = stack
        .transport
        .dispatch_workflow(
            &target,
            &request,
            Duration::from_millis(config.dispatch_timeout_ms),
        )
        .await
        .unwrap();
    let (final_ordinal, retried_output) = complete(retried);
    let value: Value = serde_json::from_str(&retried_output).unwrap();
    assert_eq!(value, json!({"attempts":2,"durable":1}));
    scheduler
        .finish_workflow(
            &retry.fence,
            &WorkflowCompletion::Complete {
                output_json: retried_output,
                final_ordinal,
            },
            expired + 1001,
            &config,
        )
        .unwrap();
    workflow
        .reconcile(&mut WorkflowReconcileCursor::default(), 32, expired + 1002)
        .unwrap();
    for index in 0..32 {
        workflow
            .create(
                account,
                definition.id,
                Some(&format!("backlog-{index}")),
                open_compute_workers::WorkflowCreateInput {
                    payload_json: "{\"backlog\":true}",
                    retention: None,
                },
                now(),
            )
            .unwrap();
    }
    let clock = Arc::new(open_compute_core::DeterministicSchedulerClock::new(
        now() + 61_000,
    ));
    let service = Arc::new(SchedulerService::new(
        scheduler.clone(),
        storage.clone(),
        stack.transport.clone(),
        SchedulerConfig {
            max_in_flight: 4,
            ..Default::default()
        },
        config.clone(),
        clock,
    ));
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let kernel = tokio::spawn(service.clone().run(stopped));
    let generation = WorkerRepository::new(storage.db())
        .get_worker(account, worker.id)
        .unwrap()
        .route_generation;
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        let response = p0_exit_support::dispatch(
            &stack.transport,
            account,
            worker.id,
            &deployment,
            generation,
            "/fairness",
        )
        .await;
        let flags: Value = serde_json::from_str(&response.body).unwrap();
        if flags == json!(["done", "done", "done"]) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "other pools starved: {flags}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        scheduler.inspect_workflows(now()).unwrap().queued > 0,
        "other pools must finish before the Workflow backlog drains"
    );
    assert_eq!(
        scheduler
            .queue_metrics(queue.queue.id, 1, 1)
            .unwrap()
            .backlog_count,
        0
    );
    assert!(
        service
            .inspect()
            .unwrap()
            .cron_activations
            .iter()
            .any(|row| row.last_outcome.as_deref() == Some("complete"))
    );
    stop.send(true).unwrap();
    kernel.await.unwrap().unwrap();
    stack.stop().await;
}

const SOURCE: &str = r#"
import { WorkflowEntrypoint, DurableObject } from 'cloudflare:workers';
export class Counter extends DurableObject {
  async recordOnce(key) {
    const prior = await this.ctx.storage.get(key);
    if (prior) return prior;
    await this.ctx.storage.put(key, 1);
    await this.ctx.storage.setAlarm(Date.now()+1);
    return 1;
  }
  async alarm() { await this.env.KV.put('alarm','done'); }
}
export class Flow extends WorkflowEntrypoint {
  async run(event,step) {
    if(event.payload?.crashBeforeCommit) {
      return step.do('effect',{timeout:240000,retries:{limit:0,delay:0}},async context=>{
        const attempts = Number(await this.env.KV.get('effects') || 0)+1;
        await this.env.KV.put('effects',String(attempts));
        const durable = await this.env.OBJECTS.getByName('counter').recordOnce(`${event.instanceId}:${context.step.name}:${context.step.count}`);
        await this.env.KV.put('effects-ready','ready');
        await new Promise(resolve=>setTimeout(resolve,5000));
        return {attempts,durable};
      });
    }
    if(event.payload?.backlog) {
      return step.do('hold',async()=>{await new Promise(resolve=>setTimeout(resolve,2000));return true;});
    }
    let calls = 0;
    const value = await step.do('products', async context => {
      calls++;
      const key = `${event.instanceId}:${context.step.name}:${context.step.count}`;
      let stage = 'kv-write';
      try {
      await this.env.KV.put(key, 'stored');
      stage = 'r2-write';
      await this.env.R2.put(key, 'stored');
      stage = 'd1';
      const d1 = await this.env.DB.prepare('SELECT 7 AS value').first('value');
      stage = 'do';
      const object = await this.env.OBJECTS.getByName('counter').recordOnce(key);
      stage = 'queue';
      await this.env.QUEUE.send({key});
      stage = 'readback';
      return {kv:await this.env.KV.get(key),r2:await (await this.env.R2.get(key)).text(),d1,object,context};
      } catch(error) { return {failure:{stage,message:String(error)}}; }
    });
    return {calls,value};
  }
}
export default {
  async fetch(request,env){
    if(new URL(request.url).pathname==='/effect') return Response.json(await env.KV.get('effects-ready'));
    return Response.json(await Promise.all(['queue','cron','alarm'].map(key=>env.KV.get(key))));
  },
  async queue(batch,env){await env.KV.put('queue','done');batch.ackAll();},
  async scheduled(controller,env){await env.KV.put('cron','done');}
};
"#;
