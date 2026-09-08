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

mod p2_2_real_queue_scheduler_matrix;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p2_2_real_queue_scheduler_matrix() {
    p2_2_real_queue_scheduler_matrix::run().await;
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
                props: None,
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

#[allow(
    clippy::too_many_arguments,
    reason = "scenario helpers keep distinct fixture identities explicit"
)]
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
