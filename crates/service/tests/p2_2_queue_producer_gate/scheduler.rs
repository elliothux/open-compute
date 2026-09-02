//! Real pinned-workerd `SchedulerService` claim, disposition, recovery, and producer paths.

use super::{
    DispatchResponse, artifact_store, deploy, dispatch, repo_root, runtime_config, storage_config,
    wait_pid_change, wait_running,
};
use axum::body::Body;
use axum::http::{Request, header};
use open_compute_artifacts::{ArtifactCache, MockS3};
use open_compute_core::clock::SystemClock;
use open_compute_core::{
    AccountId, BindingKind, CacheConfig, CanonicalBindingConfig, CanonicalPermissions,
    DurableObjectsConfig, QueueId, RequestId, ResourceId, SchedulerConfig, StartupId,
    SystemSchedulerClock, WorkflowFence, WorkflowId, WorkflowInstanceId, WorkflowToken,
    WorkflowVersionId,
};
use open_compute_runtime::{
    DirectoryServicePath, ExternalServiceAddress, GenerationAuthRegistry, OsJitter,
    PlatformReleaseMeta, StaticConfigCompiler, WorkerdSupervisor, WorkerdSupervisorOptions,
    verify_runtime_binary,
};
use open_compute_service::asset_backend::AssetBindingService;
use open_compute_service::runtime_bridge::{
    DispatchTarget, LoaderOutcome, WorkerdTransport, WorkflowOutcome, WorkflowRunRequest,
    bind_runtime_source, serve_runtime_source,
};
use open_compute_service::scheduler::SchedulerService;
use open_compute_service::service_invocations::ServiceInvocationRegistry;
use open_compute_service::{
    SqliteKvBindingExecutor, bind_binding_backend, product_promotion_for_test,
    serve_binding_backend_with_assets,
};
use open_compute_storage::{
    ClaimedQueueBatch, DO_NAMESPACE_SCHEMA_VERSION, PlatformStorage, QueueConfig,
    QueueConsumerConfig, QueueRepository, SchedulerStore, VersionRecord, WorkerRepository,
    WorkflowTarget,
};
use open_compute_workers::{
    BundleLimits, CanonicalBundle, CreateQueueOutcome, CreateQueueRequest, CreateResourceOutcome,
    CreateResourceRequest, CreateVersionRequest, DurableObjectResourceDriver, ModuleInput,
    ModuleType, QueueConsumerInput, QueueController, ResourceController, ResourcePins,
    RuntimeSource, RuntimeValidator, VersionBindingInput, VersionController, VersionPins,
    VersionServiceInput,
};
use rusqlite::{Connection, OptionalExtension as _};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p2_2_real_queue_scheduler_matrix() {
    let workerd = std::env::var_os("OPEN_COMPUTE_TEST_WORKERD")
        .map(PathBuf::from)
        .expect("OPEN_COMPUTE_TEST_WORKERD must name the verified stock runtime");
    let root = repo_root();
    let lock = root.join("packages/runtime/workerd.lock.json");
    let temp = tempfile::tempdir().unwrap();
    let storage = Arc::new(
        PlatformStorage::bootstrap(&storage_config(&temp.path().join("data")), &SystemClock)
            .unwrap(),
    );
    let scheduler_store =
        Arc::new(SchedulerStore::open(&storage.data_dir().scheduler_db_path(), 5_000, 1).unwrap());
    let mock = MockS3::spawn("open-compute").await;
    let artifacts = artifact_store(&mock);
    let cache = Arc::new(
        ArtifactCache::open(
            storage.data_dir().artifact_cache_dir(),
            CacheConfig::default(),
            StartupId::generate(),
        )
        .unwrap(),
    );
    let runtime = verify_runtime_binary(
        &lock,
        &workerd,
        Duration::from_secs(10),
        &open_compute_core::Redactor::new(),
    )
    .await
    .expect("formal pinned runtime");
    let source_auth = GenerationAuthRegistry::new();
    let binding_auth = GenerationAuthRegistry::new();
    let source_listener = bind_runtime_source().await.unwrap();
    let source_addr = source_listener.local_addr().unwrap();
    let binding_listener = bind_binding_backend().await.unwrap();
    let binding_addr = binding_listener.local_addr().unwrap();
    let resource_pins = ResourcePins::new();
    let version_pins = VersionPins::new();
    let service_invocations = Arc::new(ServiceInvocationRegistry::new(
        storage.clone(),
        version_pins.clone(),
    ));
    let (shutdown_tx, mut source_shutdown) = tokio::sync::watch::channel(false);
    let mut binding_shutdown = shutdown_tx.subscribe();
    let source_task = tokio::spawn({
        let source =
            RuntimeSource::new(storage.clone(), artifacts.clone(), BundleLimits::default());
        let auth = source_auth.clone();
        async move {
            serve_runtime_source(source_listener, source, auth, async move {
                let _ = source_shutdown.changed().await;
            })
            .await
        }
    });
    let binding_task = tokio::spawn({
        let backend_storage = storage.clone();
        let executor_storage = storage.clone();
        let auth = binding_auth.clone();
        let pins = resource_pins.clone();
        let scheduler = scheduler_store.clone();
        let artifacts = artifacts.clone();
        let cache = cache.clone();
        let version_pins = version_pins.clone();
        let services = service_invocations.clone();
        async move {
            let assets = Arc::new(AssetBindingService::new(
                backend_storage.clone(),
                artifacts,
                cache,
                version_pins,
            ));
            serve_binding_backend_with_assets(
                binding_listener,
                backend_storage,
                auth,
                pins,
                Arc::new(SqliteKvBindingExecutor::new(
                    executor_storage,
                    Arc::new(SystemClock),
                )),
                None,
                None,
                None,
                DurableObjectsConfig {
                    disk_high_watermark_percent: 98,
                    disk_stop_writes_percent: 99,
                    ..DurableObjectsConfig::default()
                },
                open_compute_core::QueuesConfig::default(),
                open_compute_core::WorkflowsConfig::default(),
                Some(scheduler),
                assets,
                services,
                None,
                None,
                async move {
                    let _ = binding_shutdown.changed().await;
                },
            )
            .await
        }
    });
    let compiler = StaticConfigCompiler::new(
        runtime.clone(),
        lock.clone(),
        root.join("packages/runtime"),
        storage.data_dir().runtime_dir(),
        PlatformReleaseMeta {
            version: "p2.2-scheduler-gate".to_owned(),
        },
        Duration::from_secs(20),
        open_compute_core::Redactor::new(),
    )
    .with_generation_auth(source_auth.clone())
    .with_binding_generation_auth(binding_auth.clone());
    let supervisor_slot = Arc::new(Mutex::new(None));
    let transport = WorkerdTransport::new(source_auth.clone(), supervisor_slot.clone());
    let do_storage = storage
        .data_dir()
        .prepare_durable_object_storage(
            &storage.identity().platform_id.to_string(),
            runtime.version_output(),
        )
        .unwrap();
    let supervisor = Arc::new(WorkerdSupervisor::new(
        WorkerdSupervisorOptions {
            runtime,
            compiler,
            config: runtime_config(),
            clock: Arc::new(SystemClock),
            jitter: Arc::new(OsJitter),
            redactor: open_compute_core::Redactor::new(),
            lease_path: Some(
                storage
                    .data_dir()
                    .runtime_dir()
                    .join("p2-2-scheduler-gate.lease"),
            ),
        },
        vec![
            ExternalServiceAddress::loopback("runtime-source", source_addr).unwrap(),
            ExternalServiceAddress::loopback("binding-backend", binding_addr).unwrap(),
        ],
        vec![DirectoryServicePath::local("do-storage", &do_storage).unwrap()],
        vec![source_auth.clone(), binding_auth.clone()],
    ));
    *supervisor_slot.lock().unwrap() = Some(supervisor.clone());
    supervisor.start();
    wait_running(&supervisor, Duration::from_secs(30)).await;

    let account = storage.identity().default_account_id;
    let events = create_queue(
        &storage,
        scheduler_store.clone(),
        account,
        "events",
        "events",
    );
    let dlq = create_queue(
        &storage,
        scheduler_store.clone(),
        account,
        "events-dlq",
        "events-dlq",
    );
    let workers = WorkerRepository::new(storage.db());
    let (worker, _) = workers
        .create_worker(
            account,
            "queue-scheduler",
            RequestId::generate(),
            10,
            1_000_000,
        )
        .unwrap();
    let (caller, _) = workers
        .create_worker(
            account,
            "queue-caller",
            RequestId::generate(),
            11,
            1_000_000,
        )
        .unwrap();
    let namespace = create_namespace(&storage, resource_pins.clone(), account, worker.id);
    let validator: Arc<dyn RuntimeValidator> = Arc::new(transport.clone());
    let versions = VersionController::new(
        &storage,
        artifacts.clone(),
        validator,
        BundleLimits::default(),
    )
    .with_product_promoter(product_promotion_for_test(
        storage.clone(),
        scheduler_store.clone(),
    ));
    let version = deploy(
        &versions,
        consumer_request(
            account,
            worker.id,
            events,
            dlq,
            namespace,
            "scheduler-bound",
            20,
        ),
    )
    .await;
    let caller_version = deploy(
        &versions,
        caller_request(account, caller.id, worker.id, "scheduler-caller", 21),
    )
    .await;
    let generation = i64::try_from(
        workers
            .get_worker(account, worker.id)
            .unwrap()
            .route_generation,
    )
    .unwrap();
    let caller_generation = i64::try_from(
        workers
            .get_worker(account, caller.id)
            .unwrap()
            .route_generation,
    )
    .unwrap();
    let scheduler = Arc::new(SchedulerService::new(
        scheduler_store.clone(),
        storage.clone(),
        transport.clone(),
        SchedulerConfig::default(),
        open_compute_core::WorkflowsConfig::default(),
        Arc::new(SystemSchedulerClock),
    ));
    let db = storage.data_dir().scheduler_db_path();
    let catalog = QueueRepository::new(storage.db());
    let events_row = catalog.get(account, events).unwrap();
    let empty = dispatch(
        &transport,
        account,
        worker.id,
        &version,
        generation,
        None,
        "/operator/metrics",
    )
    .await;
    assert_eq!(empty.status, 200, "{}", empty.body);
    let empty_metrics: serde_json::Value = serde_json::from_str(&empty.body).unwrap();
    assert_eq!(empty_metrics["backlogCount"], 0);
    assert!(empty_metrics.get("oldestMessageTimestamp").is_none());

    let worker_send = dispatch(
        &transport, account, worker.id, &version, generation, None, "/worker",
    )
    .await;
    assert_eq!(worker_send.status, 200, "{}", worker_send.body);
    let do_send = dispatch(
        &transport, account, worker.id, &version, generation, None, "/do",
    )
    .await;
    assert_eq!(
        do_send.status,
        200,
        "{}; diagnostics={:?}",
        do_send.body,
        supervisor.last_diagnostics()
    );
    let do_result: serde_json::Value = serde_json::from_str(&do_send.body).unwrap();
    assert_eq!(do_result["backlogCount"], 2, "{do_result}");
    let workflow = transport
        .dispatch_workflow(
            &WorkflowTarget {
                account_id: account,
                definition_id: WorkflowId::generate(),
                definition_name: "queue-flow".to_owned(),
                workflow_version_id: WorkflowVersionId::generate(),
                worker_id: worker.id,
                worker_version_id: version.id,
                worker_code_sha256: version.worker_code_sha256,
                class_name: "Flow".to_owned(),
                loader_schema_version: 1,
                capability_version: 1,
                descriptor_sha256: [7; 32],
            },
            &WorkflowRunRequest {
                fence: WorkflowFence {
                    instance_id: WorkflowInstanceId::generate(),
                    instance_generation: 1,
                    run_token: WorkflowToken::from_bytes([8; 32]),
                },
                external_instance_id: "queue-scheduler-flow".to_owned(),
                definition_name: "queue-flow".to_owned(),
                created_at_ms: 1_700_000_000_000,
                payload_base64: "T0NEVgECAA==".to_owned(),
                rollback: false,
                schedule: None,
            },
            Duration::from_secs(5),
        )
        .await
        .expect("workflow queue mutation");
    match workflow.result {
        WorkflowOutcome::Complete { .. } => {}
        outcome => panic!("unexpected Workflow outcome: {outcome:?}"),
    }
    let service_send = dispatch(
        &transport,
        account,
        caller.id,
        &caller_version,
        caller_generation,
        None,
        "/",
    )
    .await;
    assert_eq!(service_send.status, 200, "{}", service_send.body);
    assert_eq!(service_send.body, "service");
    assert_eq!(
        scheduler_store
            .queue_metrics(
                events,
                events_row.lifecycle_generation,
                events_row.config_generation,
            )
            .unwrap()
            .backlog_count,
        4
    );
    let live = dispatch(
        &transport,
        account,
        worker.id,
        &version,
        generation,
        None,
        "/operator/metrics",
    )
    .await;
    let live_metrics: serde_json::Value = serde_json::from_str(&live.body).unwrap();
    assert_eq!(live_metrics["backlogCount"], 4);
    assert!(live_metrics.get("oldestMessageTimestamp").is_some());
    assert!(apply_due(&scheduler).await >= 1);
    assert_eq!(
        scheduler_store
            .queue_metrics(
                events,
                events_row.lifecycle_generation,
                events_row.config_generation,
            )
            .unwrap()
            .backlog_count,
        0
    );

    send_text(
        &transport,
        account,
        worker.id,
        &version,
        generation,
        "ack-then-retry",
    )
    .await;
    send_text(
        &transport,
        account,
        worker.id,
        &version,
        generation,
        "retry-then-ack",
    )
    .await;
    let mixed = claim_one(&scheduler).await;
    assert_eq!(mixed.messages.len(), 2);
    let before_mixed = wall_ms();
    assert_eq!(claimed_count(&db), 2, "claimed messages must not ack early");
    scheduler.clone().dispatch_queue_batch(mixed).await;
    assert!(text_missing(&db, "ack-then-retry"));
    let retried = text_row(&db, "retry-then-ack").expect("retry-then-ack must remain");
    assert_eq!(retried.state, "ready");
    assert_eq!(retried.attempts, 1);
    let retry_delay = retried.available_at_ms.saturating_sub(before_mixed);
    assert!(
        (3_500..=5_500).contains(&retry_delay),
        "retry-then-ack delay {retry_delay}"
    );
    let [second_mixed] = scheduler_store
        .claim_queue_batches(retried.available_at_ms, 60_000, 0, 1, None)
        .unwrap()
        .0
        .try_into()
        .unwrap();
    assert_eq!(second_mixed.messages[0].delivery_attempt, 2);
    scheduler.clone().dispatch_queue_batch(second_mixed).await;
    assert!(text_missing(&db, "retry-then-ack"));

    send_text(
        &transport,
        account,
        worker.id,
        &version,
        generation,
        "ack-all-then-retry-all",
    )
    .await;
    send_text(
        &transport,
        account,
        worker.id,
        &version,
        generation,
        "ack-all-then-retry-all",
    )
    .await;
    let ack_all = claim_one(&scheduler).await;
    assert_eq!(ack_all.messages.len(), 2);
    scheduler.clone().dispatch_queue_batch(ack_all).await;
    assert_eq!(text_count(&db, "ack-all-then-retry-all"), 0);

    send_text(
        &transport,
        account,
        worker.id,
        &version,
        generation,
        "retry-all-then-ack-all",
    )
    .await;
    send_text(
        &transport,
        account,
        worker.id,
        &version,
        generation,
        "retry-all-then-ack-all",
    )
    .await;
    let retry_all = claim_one(&scheduler).await;
    assert_eq!(retry_all.messages.len(), 2);
    let before_retry_all = wall_ms();
    scheduler.clone().dispatch_queue_batch(retry_all).await;
    let delayed = text_rows(&db, "retry-all-then-ack-all");
    assert_eq!(delayed.len(), 2);
    for row in &delayed {
        assert_eq!(row.state, "ready");
        assert_eq!(row.attempts, 1);
        let delay = row.available_at_ms.saturating_sub(before_retry_all);
        assert!((5_500..=7_500).contains(&delay), "retryAll delay {delay}");
    }
    let [retry_all_second] = scheduler_store
        .claim_queue_batches(delayed[0].available_at_ms, 60_000, 0, 1, None)
        .unwrap()
        .0
        .try_into()
        .unwrap();
    assert_eq!(retry_all_second.messages.len(), 2);
    assert!(
        retry_all_second
            .messages
            .iter()
            .all(|message| message.delivery_attempt == 2)
    );
    scheduler
        .clone()
        .dispatch_queue_batch(retry_all_second)
        .await;
    assert_eq!(text_count(&db, "retry-all-then-ack-all"), 0);

    send_text(
        &transport, account, worker.id, &version, generation, "throw",
    )
    .await;
    let thrown = claim_one(&scheduler).await;
    assert_eq!(thrown.messages[0].delivery_attempt, 1);
    assert_eq!(claimed_count(&db), 1);
    scheduler.clone().dispatch_queue_batch(thrown).await;
    let after_throw = text_row(&db, "throw").expect("handler throw must not ack");
    assert_eq!(after_throw.state, "ready");
    assert_eq!(after_throw.attempts, 1);
    let [throw_retry] = scheduler_store
        .claim_queue_batches(
            after_throw.available_at_ms.max(wall_ms()),
            60_000,
            0,
            1,
            None,
        )
        .unwrap()
        .0
        .try_into()
        .unwrap();
    assert_eq!(throw_retry.messages[0].delivery_attempt, 2);
    scheduler.clone().dispatch_queue_batch(throw_retry).await;
    assert!(text_missing(&db, "throw"));

    send_text(
        &transport,
        account,
        worker.id,
        &version,
        generation,
        "wait-until",
    )
    .await;
    let rejected = claim_one(&scheduler).await;
    scheduler.clone().dispatch_queue_batch(rejected).await;
    let after_wait = text_row(&db, "wait-until").expect("rejected waitUntil must not ack");
    assert_eq!(after_wait.state, "ready");
    assert_eq!(after_wait.attempts, 1);
    let [wait_retry] = scheduler_store
        .claim_queue_batches(
            after_wait.available_at_ms.max(wall_ms()),
            60_000,
            0,
            1,
            None,
        )
        .unwrap()
        .0
        .try_into()
        .unwrap();
    scheduler.clone().dispatch_queue_batch(wait_retry).await;
    assert!(text_missing(&db, "wait-until"));

    send_text(
        &transport, account, worker.id, &version, generation, "reclaim",
    )
    .await;
    let leased = claim_one(&scheduler).await;
    assert_eq!(leased.messages[0].delivery_attempt, 1);
    assert_eq!(claimed_count(&db), 1, "visibility timeout must not ack");
    assert_eq!(
        scheduler_store
            .recover_expired_queue_batches(leased.claim_until_ms, 0, 8)
            .unwrap(),
        1
    );
    let recovered = text_row(&db, "reclaim").expect("reclaimed message");
    assert_eq!(recovered.state, "ready");
    assert_eq!(recovered.attempts, 0);
    let [redelivered] = scheduler_store
        .claim_queue_batches(leased.claim_until_ms, 60_000, 0, 1, None)
        .unwrap()
        .0
        .try_into()
        .unwrap();
    assert_eq!(redelivered.messages[0].id, leased.messages[0].id);
    assert_eq!(redelivered.messages[0].delivery_attempt, 1);
    assert_ne!(redelivered.claim_token, leased.claim_token);
    scheduler.clone().dispatch_queue_batch(redelivered).await;
    assert!(text_missing(&db, "reclaim"));

    send_text(
        &transport, account, worker.id, &version, generation, "dlq-me",
    )
    .await;
    let first_dlq = claim_one(&scheduler).await;
    assert_eq!(first_dlq.messages[0].delivery_attempt, 1);
    scheduler.clone().dispatch_queue_batch(first_dlq).await;
    let retried_dlq = text_row(&db, "dlq-me").expect("first retry stays on source");
    assert_eq!(retried_dlq.attempts, 1);
    let [second_dlq] = scheduler_store
        .claim_queue_batches(
            retried_dlq.available_at_ms.max(wall_ms()),
            60_000,
            0,
            1,
            None,
        )
        .unwrap()
        .0
        .try_into()
        .unwrap();
    assert_eq!(second_dlq.messages[0].delivery_attempt, 2);
    scheduler.clone().dispatch_queue_batch(second_dlq).await;
    assert_eq!(text_queue(&db, "dlq-me"), Some(dlq.to_string()));
    let dlq_row = catalog.get(account, dlq).unwrap();
    assert_eq!(
        scheduler_store
            .queue_metrics(
                events,
                events_row.lifecycle_generation,
                events_row.config_generation
            )
            .unwrap()
            .backlog_count,
        0
    );
    assert_eq!(
        scheduler_store
            .queue_metrics(dlq, dlq_row.lifecycle_generation, dlq_row.config_generation)
            .unwrap()
            .backlog_count,
        1
    );

    let v8 = dispatch(
        &transport, account, worker.id, &version, generation, None, "/v8",
    )
    .await;
    assert_eq!(v8.status, 200, "{}", v8.body);
    let v8_body = Connection::open(&db)
        .unwrap()
        .query_row(
            "SELECT body FROM queue_messages WHERE content_type = 'v8' ORDER BY seq DESC LIMIT 1",
            [],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .unwrap();
    assert!(v8_body.starts_with(&[0x4f, 0x43, 0x44, 0x56]));
    let before_restart = supervisor.snapshot().pid.unwrap();
    supervisor.report_unhealthy();
    wait_pid_change(&supervisor, before_restart, Duration::from_secs(30)).await;
    let restored = claim_one(&scheduler).await;
    assert_eq!(restored.messages[0].content_type.as_str(), "v8");
    scheduler.clone().dispatch_queue_batch(restored).await;
    assert_eq!(
        Connection::open(&db)
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM queue_messages WHERE content_type = 'v8'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        0
    );

    let drained = dispatch(
        &transport,
        account,
        worker.id,
        &version,
        generation,
        None,
        "/operator/metrics",
    )
    .await;
    let drained_metrics: serde_json::Value = serde_json::from_str(&drained.body).unwrap();
    assert_eq!(drained_metrics["backlogCount"], 0);
    assert!(drained_metrics.get("oldestMessageTimestamp").is_none());
    assert_eq!(apply_due(&scheduler).await, 0);

    supervisor.shutdown().await;
    assert_eq!(supervisor.owner_registry_len(), 0);
    let _ = shutdown_tx.send(true);
    source_task.await.unwrap().unwrap();
    binding_task.await.unwrap().unwrap();
}

fn consumer_source() -> &'static str {
    r#"import { DurableObject, WorkerEntrypoint, WorkflowEntrypoint } from "cloudflare:workers";

export class Producer extends WorkerEntrypoint {
  async enqueue(body, options) {
    try {
      return await this.env.EVENTS.send(body, options || { contentType: "text", delaySeconds: 0 });
    } catch (error) {
      return { error: String(error && (error.stableCode || error.message) || error) };
    }
  }
}

export class ProducerObject extends DurableObject {
  async fetch() {
    try {
      const result = await this.env.EVENTS.send("from-do", { contentType: "text", delaySeconds: 0 });
      return Response.json({ backlogCount: result.metadata.metrics.backlogCount });
    } catch (error) {
      const code = String(error && (error.stableCode || error.message) || error);
      return Response.json({ error: code });
    }
  }
}

export class Flow extends WorkflowEntrypoint {
  async run() {
    const result = await this.env.EVENTS.send("from-workflow", { contentType: "text", delaySeconds: 0 });
    return { backlogCount: result.metadata.metrics.backlogCount };
  }
}

export default {
  async fetch(request, env) {
    const path = new URL(request.url).pathname;
    if (path === "/operator/metrics") return Response.json(await env.EVENTS.metrics());
    if (path === "/worker") {
      const result = await env.EVENTS.send("from-worker", { contentType: "text", delaySeconds: 0 });
      return Response.json({ backlogCount: result.metadata.metrics.backlogCount });
    }
    if (path === "/do") return env.OBJECTS.getByName("queue-producer").fetch("https://do.test/");
    if (path === "/send") {
      return Response.json(await env.EVENTS.send(await request.text(), { contentType: "text", delaySeconds: 0 }));
    }
    if (path === "/v8") {
      const cycle = { v8: true, when: new Date(1_700_000_000_000) };
      cycle.self = cycle;
      cycle.items = new Map([["k", new Set([1, 2])]]);
      return Response.json(await env.EVENTS.send(cycle, { contentType: "v8", delaySeconds: 0 }));
    }
    return new Response("ok");
  },
  async queue(batch, _env, ctx) {
    const metrics = batch.metadata && batch.metadata.metrics;
    if (typeof batch.queue !== "string" || !metrics) throw new Error("metadata");
    if (typeof metrics.backlogCount !== "number" || typeof metrics.backlogBytes !== "number") {
      throw new Error("metrics");
    }
    if (metrics.backlogCount > 0 && !(metrics.oldestMessageTimestamp instanceof Date)) {
      throw new Error("oldest");
    }
    if (metrics.backlogCount === 0 && metrics.oldestMessageTimestamp !== undefined) {
      throw new Error("empty-oldest");
    }
    if (metrics.backlogCount < batch.messages.length) throw new Error("backlog");
    const texts = batch.messages.map((message) => typeof message.body === "string" ? message.body : "");
    if (texts.includes("throw") && batch.messages.some((message) => message.attempts === 1 && message.body === "throw")) {
      throw new Error("handler throw");
    }
    if (texts.includes("wait-until") && batch.messages.some((message) => message.attempts === 1 && message.body === "wait-until")) {
      ctx.waitUntil(Promise.reject(new Error("waitUntil")));
      return;
    }
    if (texts.every((text) => text === "retry-all-then-ack-all")) {
      if (batch.messages[0].attempts === 1) {
        batch.retryAll({ delaySeconds: 6 });
        batch.ackAll();
      } else {
        batch.ackAll();
      }
      return;
    }
    if (texts.every((text) => text === "ack-all-then-retry-all")) {
      batch.ackAll();
      batch.retryAll({ delaySeconds: 3 });
      return;
    }
    for (const message of batch.messages) {
      if (!(message.timestamp instanceof Date) || message.attempts < 1) throw new Error("message");
      if (message.body && message.body.v8 === true) {
        if (!(message.body.when instanceof Date) || !(message.body.items instanceof Map)
            || !(message.body.items.get("k") instanceof Set) || message.body.self !== message.body) {
          throw new Error("v8 body");
        }
        message.ack();
        continue;
      }
      if (message.body === "retry-then-ack") {
        if (message.attempts === 1) {
          message.retry({ delaySeconds: 4 });
          message.ack();
        } else {
          message.ack();
        }
        continue;
      }
      if (message.body === "ack-then-retry") {
        message.ack();
        message.retry({ delaySeconds: 9 });
        continue;
      }
      if (message.body === "dlq-me") {
        message.retry({ delaySeconds: 0 });
        continue;
      }
      message.ack();
    }
  }
};
"#
}

fn caller_source() -> &'static str {
    r#"export default {
  async fetch(_request, env) {
    try {
      const result = await env.PRODUCER.enqueue("from-service", { contentType: "text", delaySeconds: 0 });
      if (result && result.error) return Response.json(result);
      return new Response("service");
    } catch (error) {
      return Response.json({ error: String(error && (error.stableCode || error.message) || error) });
    }
  }
};"#
}

fn create_queue(
    storage: &PlatformStorage,
    scheduler: Arc<SchedulerStore>,
    account_id: AccountId,
    name: &str,
    key: &str,
) -> QueueId {
    match QueueController::new(storage, scheduler)
        .create(&CreateQueueRequest {
            account_id,
            name: name.to_owned(),
            config: QueueConfig::default(),
            idempotency_key: key.to_owned(),
            request_id: RequestId::generate(),
            now_ms: 1,
        })
        .unwrap()
    {
        CreateQueueOutcome::Applied(result) => result.queue.id,
        CreateQueueOutcome::Replay(_) => panic!("unexpected Queue create replay"),
    }
}

fn create_namespace(
    storage: &PlatformStorage,
    pins: ResourcePins,
    account_id: AccountId,
    worker_id: open_compute_core::WorkerId,
) -> ResourceId {
    let driver = DurableObjectResourceDriver::new(storage, worker_id, "ProducerObject");
    match ResourceController::new(storage, pins, driver)
        .create(&CreateResourceRequest {
            account_id,
            kind: BindingKind::DoNamespace,
            name: "queue-objects".to_owned(),
            idempotency_key: "p2-2-scheduler-do".to_owned(),
            driver_schema_version: DO_NAMESPACE_SCHEMA_VERSION,
            request_id: RequestId::generate(),
            now_ms: 12,
        })
        .unwrap()
    {
        CreateResourceOutcome::Applied(value) => value.resource_id,
        CreateResourceOutcome::Replay(_) => panic!("unexpected namespace replay"),
    }
}

fn consumer_request(
    account_id: AccountId,
    worker_id: open_compute_core::WorkerId,
    queue_id: QueueId,
    dlq_id: QueueId,
    namespace: ResourceId,
    key: &str,
    now_ms: i64,
) -> CreateVersionRequest {
    let bundle = CanonicalBundle::build(
        "index.js",
        vec![ModuleInput {
            name: "index.js".to_owned(),
            module_type: ModuleType::EsModule,
            bytes: consumer_source().as_bytes().to_vec(),
        }],
        BundleLimits::default(),
    )
    .unwrap();
    let mut bindings = BTreeMap::new();
    bindings.insert(
        "EVENTS".to_owned(),
        VersionBindingInput {
            kind: BindingKind::QueueProducer,
            id: ResourceId::from_uuid(queue_id.as_uuid()).unwrap(),
            permissions: CanonicalPermissions::default(),
            config: CanonicalBindingConfig::default(),
        },
    );
    bindings.insert(
        "OBJECTS".to_owned(),
        VersionBindingInput {
            kind: BindingKind::DoNamespace,
            id: namespace,
            permissions: CanonicalPermissions::default(),
            config: CanonicalBindingConfig::default(),
        },
    );
    CreateVersionRequest {
        account_id,
        worker_id,
        idempotency_key: key.to_owned(),
        content: open_compute_workers::VersionContent::Worker {
            bundle: bundle.into_bytes().into(),
            assets: None,
        },
        vars: BTreeMap::new(),
        secrets: BTreeMap::new(),
        bindings,
        services: BTreeMap::new(),
        runtime_features: Default::default(),
        queue_consumers: vec![QueueConsumerInput {
            queue: queue_id,
            entrypoint: None,
            config: QueueConsumerConfig {
                max_batch_size: 10,
                max_batch_timeout_seconds: 0,
                max_retries: 1,
                retry_delay_seconds: 0,
                max_concurrency: 4,
            },
            dead_letter_queue: Some(dlq_id),
        }],
        crons: Vec::new(),
        deployment_source: Some(open_compute_storage::DeploymentSource::VersionsApi),
        request_id: RequestId::generate(),
        now_ms,
    }
}

fn caller_request(
    account_id: AccountId,
    worker_id: open_compute_core::WorkerId,
    target_worker_id: open_compute_core::WorkerId,
    key: &str,
    now_ms: i64,
) -> CreateVersionRequest {
    let bundle = CanonicalBundle::build(
        "index.js",
        vec![ModuleInput {
            name: "index.js".to_owned(),
            module_type: ModuleType::EsModule,
            bytes: caller_source().as_bytes().to_vec(),
        }],
        BundleLimits::default(),
    )
    .unwrap();
    CreateVersionRequest {
        account_id,
        worker_id,
        idempotency_key: key.to_owned(),
        content: open_compute_workers::VersionContent::Worker {
            bundle: bundle.into_bytes().into(),
            assets: None,
        },
        vars: BTreeMap::new(),
        secrets: BTreeMap::new(),
        bindings: BTreeMap::new(),
        services: BTreeMap::from([(
            "PRODUCER".to_owned(),
            VersionServiceInput {
                target_worker_id,
                entrypoint: Some("Producer".to_owned()),
            },
        )]),
        runtime_features: Default::default(),
        queue_consumers: Vec::new(),
        crons: Vec::new(),
        deployment_source: Some(open_compute_storage::DeploymentSource::VersionsApi),
        request_id: RequestId::generate(),
        now_ms,
    }
}

async fn send_text(
    transport: &WorkerdTransport,
    account_id: AccountId,
    worker_id: open_compute_core::WorkerId,
    version: &VersionRecord,
    route_generation: i64,
    body: &str,
) {
    let response = post(
        transport,
        account_id,
        worker_id,
        version,
        route_generation,
        "/send",
        Body::from(body.to_owned()),
    )
    .await;
    assert_eq!(response.status, 200, "{}", response.body);
}

#[allow(clippy::too_many_arguments)]
async fn post(
    transport: &WorkerdTransport,
    account_id: AccountId,
    worker_id: open_compute_core::WorkerId,
    version: &VersionRecord,
    route_generation: i64,
    path: &str,
    body: Body,
) -> DispatchResponse {
    let request = Request::builder()
        .method("POST")
        .uri(path)
        .header(header::HOST, "queue.test")
        .body(body)
        .unwrap();
    let response = transport
        .dispatch(
            DispatchTarget {
                account_id,
                worker_id,
                version_id: version.id,
                worker_code_sha256: hex::encode(version.worker_code_sha256),
                entrypoint: None,
                route_generation,
                request_id: RequestId::generate(),
            },
            request,
        )
        .await
        .unwrap();
    let status = response.status().as_u16();
    let loader_outcome = response.extensions().get::<LoaderOutcome>().copied();
    let bytes = axum::body::to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    DispatchResponse {
        status,
        body: String::from_utf8(bytes.to_vec()).unwrap(),
        loader_outcome,
    }
}

async fn claim_one(scheduler: &SchedulerService) -> ClaimedQueueBatch {
    for _ in 0..50 {
        let mut batches = scheduler.claim_queue_consumers(1).await.unwrap();
        if let Some(batch) = batches.pop() {
            return batch;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("scheduler did not claim a due Queue batch");
}

async fn apply_due(scheduler: &Arc<SchedulerService>) -> usize {
    let mut claimed = 0;
    loop {
        let batches = scheduler.claim_queue_consumers(8).await.unwrap();
        if batches.is_empty() {
            return claimed;
        }
        claimed += batches.len();
        for batch in batches {
            scheduler.clone().dispatch_queue_batch(batch).await;
        }
    }
}

struct MessageRow {
    state: String,
    attempts: i64,
    available_at_ms: i64,
}

fn text_row(path: &Path, body: &str) -> Option<MessageRow> {
    text_rows(path, body).into_iter().next()
}

fn text_rows(path: &Path, body: &str) -> Vec<MessageRow> {
    let connection = Connection::open(path).unwrap();
    let mut statement = connection
        .prepare(
            "SELECT state, attempts, available_at_ms FROM queue_messages
             WHERE content_type = 'text' AND body = ?1 ORDER BY seq",
        )
        .unwrap();
    statement
        .query_map([body.as_bytes()], |row| {
            Ok(MessageRow {
                state: row.get(0)?,
                attempts: row.get(1)?,
                available_at_ms: row.get(2)?,
            })
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

fn text_count(path: &Path, body: &str) -> usize {
    text_rows(path, body).len()
}

fn text_missing(path: &Path, body: &str) -> bool {
    text_count(path, body) == 0
}

fn text_queue(path: &Path, body: &str) -> Option<String> {
    Connection::open(path)
        .unwrap()
        .query_row(
            "SELECT queue_id FROM queue_messages
             WHERE content_type = 'text' AND body = ?1 ORDER BY seq LIMIT 1",
            [body.as_bytes()],
            |row| row.get(0),
        )
        .optional()
        .unwrap()
}

fn claimed_count(path: &Path) -> i64 {
    Connection::open(path)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM queue_messages WHERE state = 'claimed'",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

fn wall_ms() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
}
