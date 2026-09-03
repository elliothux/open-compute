//! Service Binding calls from Queue, Cron, Durable Object, and Workflow event sources.

#![cfg(feature = "test-support")]

mod p3_services_support;

use axum::body::{Body, to_bytes};
use axum::http::{Request, header};
use base64::Engine as _;
use open_compute_core::{
    BindingKind, CanonicalBindingConfig, CanonicalPermissions, QueueMessageId, RequestId,
    WorkflowFence, WorkflowId, WorkflowInstanceId, WorkflowToken, WorkflowVersionId,
};
use open_compute_service::runtime_bridge::{
    DispatchTarget, QueueDispatchMessage, QueueDispatchRequest, ScheduledDispatchRequest,
    WorkflowOutcome, WorkflowRunRequest,
};
use open_compute_storage::{
    DO_NAMESPACE_SCHEMA_VERSION, QueueContentType, SchedulerStore, WorkerRepository, WorkflowTarget,
};
use open_compute_workers::{
    BundleLimits, CanonicalBundle, CreateResourceOutcome, CreateResourceRequest,
    CreateVersionOutcome, CreateVersionRequest, DurableObjectResourceDriver, ModuleInput,
    ModuleType, ResourceController, ResourcePins, RuntimeValidator, VersionBindingInput,
    VersionContent, VersionController, VersionServiceInput,
};
use p3_services_support::Harness;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

const TARGET: &str = r#"
import { WorkerEntrypoint } from "cloudflare:workers";
export default class Target extends WorkerEntrypoint {
  ping(source) { return `service:${source}`; }
  async connect(socket) {
    const source = await new Response(socket.readable).text();
    const writer = socket.writable.getWriter();
    await writer.write(new TextEncoder().encode(`service:${source}`));
    await writer.close();
    writer.releaseLock();
  }
}
"#;

const EVENTS: &str = r#"
import { DurableObject, WorkflowEntrypoint } from "cloudflare:workers";

async function socketPing(target, source) {
  const socket = target.connect(`${source}.invalid:1`, { allowHalfOpen: true });
  await socket.opened;
  const writer = socket.writable.getWriter();
  await writer.write(new TextEncoder().encode(source));
  await writer.close();
  writer.releaseLock();
  const reply = await new Response(socket.readable).text();
  await socket.close();
  await socket.closed;
  if (reply !== `service:${source}`) throw new Error("service socket mismatch");
  return reply;
}

export class ObjectEvent extends DurableObject {
  async fetch() { return new Response(await socketPing(this.env.TARGET, "do")); }
}

export class Flow extends WorkflowEntrypoint {
  async run() { return socketPing(this.env.TARGET, "workflow"); }
}

export default {
  fetch(_request, env) { return env.OBJECT.getByName("event").fetch("https://object.test/"); },
  async queue(batch, env) {
    await socketPing(env.TARGET, "queue");
    batch.ackAll();
  },
  async scheduled(controller, env) {
    await socketPing(env.TARGET, "cron");
    controller.noRetry();
  },
};
"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn p3_service_calls_from_queue_cron_do_and_workflow_event_sources() {
    let harness = Harness::start("p3-services-events").await;
    let account = harness.storage.identity().default_account_id;
    let repository = WorkerRepository::new(harness.storage.db());
    let (target, _) = repository
        .create_worker(
            account,
            "service-event-target",
            RequestId::generate(),
            1,
            1_000_000,
        )
        .unwrap();
    let (events, _) = repository
        .create_worker(
            account,
            "service-event-sources",
            RequestId::generate(),
            2,
            1_000_000,
        )
        .unwrap();
    let namespace = create_namespace(&harness, account, events.id);
    let validator: Arc<dyn RuntimeValidator> = Arc::new(harness.transport.clone());
    let scheduler = Arc::new(
        SchedulerStore::open(
            &harness.storage.data_dir().ensure_scheduler_db().unwrap(),
            100,
            1,
        )
        .unwrap(),
    );
    let controller = VersionController::new(
        &harness.storage,
        harness.artifacts.clone(),
        validator,
        BundleLimits::default(),
    )
    .with_product_promoter(open_compute_service::product_promotion_for_test(
        harness.storage.clone(),
        scheduler,
    ));
    let _target_version = deploy(
        &controller,
        version_request(
            account,
            target.id,
            "service-event-target-v1",
            TARGET,
            BTreeMap::new(),
            BTreeMap::new(),
            1,
        ),
    )
    .await;
    let mut event_request = version_request(
        account,
        events.id,
        "service-event-sources-v1",
        EVENTS,
        BTreeMap::from([(
            "OBJECT".to_owned(),
            VersionBindingInput {
                kind: BindingKind::DoNamespace,
                id: namespace,
                permissions: CanonicalPermissions::default(),
                config: CanonicalBindingConfig::default(),
            },
        )]),
        BTreeMap::from([(
            "TARGET".to_owned(),
            VersionServiceInput {
                target_worker_id: target.id,
                entrypoint: None,
                props: None,
            },
        )]),
        2,
    );
    event_request.crons = vec!["* * * * *".to_owned()];
    let event_version = deploy(&controller, event_request).await;
    let dispatch_target = dispatch_target(account, events.id, &event_version, None);

    let message_id = QueueMessageId::generate();
    let queue = harness
        .transport
        .dispatch_queue(
            &dispatch_target,
            &QueueDispatchRequest {
                queue_name: "service-events".to_owned(),
                messages: vec![QueueDispatchMessage {
                    id: message_id.to_string(),
                    timestamp_ms: 1_788_048_000_000,
                    attempts: 1,
                    content_type: QueueContentType::Text,
                    body_base64: base64::engine::general_purpose::STANDARD.encode("event"),
                }],
                metadata: Default::default(),
            },
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    assert_eq!(queue.outcome, "ok");
    assert!(queue.ack_all);
    wait_service_drain(&harness).await;

    let scheduled = harness
        .transport
        .dispatch_scheduled(
            &dispatch_target,
            &ScheduledDispatchRequest {
                scheduled_time_ms: 1_788_048_060_000,
                cron: "* * * * *".to_owned(),
                scheduled_handler: true,
                workflow_bindings: Vec::new(),
            },
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    assert_eq!(scheduled.outcome, "ok");
    assert!(scheduled.no_retry);
    wait_service_drain(&harness).await;

    let response = harness
        .transport
        .dispatch(
            dispatch_target.clone(),
            Request::builder()
                .method("GET")
                .uri("/do")
                .header(header::HOST, "service-events.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 1024).await.unwrap();
    assert_eq!(status, 200, "unexpected DO response: {body:?}");
    assert_eq!(body.as_ref(), b"service:do");
    wait_service_drain(&harness).await;

    let workflow_target = WorkflowTarget {
        account_id: account,
        definition_id: WorkflowId::generate(),
        definition_name: "service-events".to_owned(),
        workflow_version_id: WorkflowVersionId::generate(),
        worker_id: events.id,
        worker_version_id: event_version.id,
        worker_code_sha256: event_version.worker_code_sha256,
        class_name: "Flow".to_owned(),
        loader_schema_version: 1,
        capability_version: 1,
        descriptor_sha256: [7; 32],
    };
    let workflow = harness
        .transport
        .dispatch_workflow(
            &workflow_target,
            &WorkflowRunRequest {
                fence: WorkflowFence {
                    instance_id: WorkflowInstanceId::generate(),
                    instance_generation: 1,
                    run_token: WorkflowToken::from_bytes([8; 32]),
                },
                external_instance_id: "service-event-instance".to_owned(),
                definition_name: "service-events".to_owned(),
                created_at_ms: 1_788_048_000_000,
                payload_base64: "T0NEVgECAA==".to_owned(),
                rollback: false,
                schedule: None,
            },
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    match workflow.result {
        WorkflowOutcome::Complete {
            final_ordinal,
            output_base64,
        } => {
            assert_eq!(final_ordinal, 0);
            assert_eq!(output_base64, "T0NEVgECBgAAABBzZXJ2aWNlOndvcmtmbG93");
        }
        outcome => panic!("unexpected Workflow outcome: {outcome:?}"),
    }
    wait_service_drain(&harness).await;
    harness.stop().await;
}

fn create_namespace(
    harness: &Harness,
    account_id: open_compute_core::AccountId,
    worker_id: open_compute_core::WorkerId,
) -> open_compute_core::ResourceId {
    let driver = DurableObjectResourceDriver::new(&harness.storage, worker_id, "ObjectEvent");
    match ResourceController::new(&harness.storage, ResourcePins::new(), driver)
        .create(&CreateResourceRequest {
            account_id,
            kind: BindingKind::DoNamespace,
            name: "service-event-object".to_owned(),
            idempotency_key: "service-event-object".to_owned(),
            driver_schema_version: DO_NAMESPACE_SCHEMA_VERSION,
            request_id: RequestId::generate(),
            now_ms: 1,
        })
        .unwrap()
    {
        CreateResourceOutcome::Applied(value) => value.resource_id,
        CreateResourceOutcome::Replay(_) => panic!("unexpected namespace replay"),
    }
}

#[allow(clippy::too_many_arguments)]
fn version_request(
    account_id: open_compute_core::AccountId,
    worker_id: open_compute_core::WorkerId,
    key: &str,
    source: &str,
    bindings: BTreeMap<String, VersionBindingInput>,
    services: BTreeMap<String, VersionServiceInput>,
    now_ms: i64,
) -> CreateVersionRequest {
    let bundle = CanonicalBundle::build(
        "index.js",
        vec![ModuleInput {
            name: "index.js".to_owned(),
            module_type: ModuleType::EsModule,
            bytes: source.as_bytes().to_vec(),
        }],
        BundleLimits::default(),
    )
    .unwrap();
    CreateVersionRequest {
        account_id,
        worker_id,
        idempotency_key: key.to_owned(),
        content: VersionContent::Worker {
            bundle: bundle.into_bytes().into(),
            assets: None,
        },
        vars: BTreeMap::new(),
        secrets: BTreeMap::new(),
        bindings,
        services,
        runtime_features: Default::default(),
        queue_consumers: Vec::new(),
        crons: Vec::new(),
        deployment_source: Some(open_compute_storage::DeploymentSource::VersionsApi),
        request_id: RequestId::generate(),
        now_ms,
    }
}

async fn deploy(
    controller: &VersionController<'_>,
    request: CreateVersionRequest,
) -> open_compute_storage::VersionRecord {
    match controller.create_version(request).await.unwrap() {
        CreateVersionOutcome::Applied(result) => result.version,
        CreateVersionOutcome::Replay(_) => panic!("unexpected version replay"),
    }
}

fn dispatch_target(
    account_id: open_compute_core::AccountId,
    worker_id: open_compute_core::WorkerId,
    version: &open_compute_storage::VersionRecord,
    entrypoint: Option<&str>,
) -> DispatchTarget {
    DispatchTarget {
        account_id,
        worker_id,
        version_id: version.id,
        worker_code_sha256: hex::encode(version.worker_code_sha256),
        entrypoint: entrypoint.map(str::to_owned),
        route_generation: 1,
        request_id: RequestId::generate(),
    }
}

async fn wait_service_drain(harness: &Harness) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while harness.service_invocations.counts() != (0, 0, 0) {
        assert!(
            Instant::now() < deadline,
            "Service event root did not drain: {:?}",
            harness.service_invocations.counts()
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
