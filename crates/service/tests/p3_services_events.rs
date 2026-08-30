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
    DO_NAMESPACE_SCHEMA_VERSION, QueueContentType, WorkerRepository, WorkflowTarget,
};
use open_compute_workers::{
    BundleLimits, CanonicalBundle, CreateDeploymentOutcome, CreateDeploymentRequest,
    CreateResourceOutcome, CreateResourceRequest, DeploymentBindingInput, DeploymentContent,
    DeploymentController, DeploymentServiceInput, DurableObjectResourceDriver, ModuleInput,
    ModuleType, ResourceController, ResourcePins, RuntimeValidator,
};
use p3_services_support::Harness;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

const TARGET: &str = r#"
import { WorkerEntrypoint } from "cloudflare:workers";
export default class Target extends WorkerEntrypoint {
  ping(source) { return `service:${source}`; }
}
"#;

const EVENTS: &str = r#"
import { DurableObject, WorkflowEntrypoint } from "cloudflare:workers";

export class ObjectEvent extends DurableObject {
  async fetch() { return new Response(await this.env.TARGET.ping("do")); }
}

export class Flow extends WorkflowEntrypoint {
  async run() { return this.env.TARGET.ping("workflow"); }
}

export default {
  fetch(_request, env) { return env.OBJECT.getByName("event").fetch("https://object.test/"); },
  async queue(batch, env) {
    if (await env.TARGET.ping("queue") !== "service:queue") throw new Error("service mismatch");
    batch.ackAll();
  },
  async scheduled(controller, env) {
    if (await env.TARGET.ping("cron") !== "service:cron") throw new Error("service mismatch");
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
    let controller = DeploymentController::new(
        &harness.storage,
        harness.artifacts.clone(),
        validator,
        BundleLimits::default(),
    );
    let _target_deployment = deploy(
        &controller,
        deployment_request(
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
    let event_deployment = deploy(
        &controller,
        deployment_request(
            account,
            events.id,
            "service-event-sources-v1",
            EVENTS,
            BTreeMap::from([(
                "OBJECT".to_owned(),
                DeploymentBindingInput {
                    kind: BindingKind::DoNamespace,
                    id: namespace,
                    permissions: CanonicalPermissions::default(),
                    config: CanonicalBindingConfig::default(),
                },
            )]),
            BTreeMap::from([(
                "TARGET".to_owned(),
                DeploymentServiceInput {
                    target_worker_id: target.id,
                    entrypoint: None,
                },
            )]),
            2,
        ),
    )
    .await;
    let dispatch_target = dispatch_target(account, events.id, &event_deployment, None);

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
    assert_eq!(response.status(), 200);
    assert_eq!(
        to_bytes(response.into_body(), 1024).await.unwrap().as_ref(),
        b"service:do"
    );
    wait_service_drain(&harness).await;

    let workflow_target = WorkflowTarget {
        account_id: account,
        definition_id: WorkflowId::generate(),
        definition_name: "service-events".to_owned(),
        version_id: WorkflowVersionId::generate(),
        worker_id: events.id,
        deployment_id: event_deployment.id,
        worker_code_sha256: event_deployment.worker_code_sha256,
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
                payload_json: "null".to_owned(),
            },
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    match workflow.result {
        WorkflowOutcome::Complete {
            final_ordinal,
            output_json,
        } => {
            assert_eq!(final_ordinal, 0);
            assert_eq!(output_json, r#""service:workflow""#);
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
fn deployment_request(
    account_id: open_compute_core::AccountId,
    worker_id: open_compute_core::WorkerId,
    key: &str,
    source: &str,
    bindings: BTreeMap<String, DeploymentBindingInput>,
    services: BTreeMap<String, DeploymentServiceInput>,
    now_ms: i64,
) -> CreateDeploymentRequest {
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
    CreateDeploymentRequest {
        account_id,
        worker_id,
        idempotency_key: key.to_owned(),
        content: DeploymentContent::Worker {
            bundle: bundle.into_bytes().into(),
            assets: None,
        },
        compatibility_date: "2026-08-26".to_owned(),
        compatibility_flags: vec!["rpc".to_owned()],
        vars: BTreeMap::new(),
        secrets: BTreeMap::new(),
        bindings,
        services,
        runtime_features: Default::default(),
        queue_consumers: Vec::new(),
        crons: None,
        limits: serde_json::json!({"profile":"default"}),
        promote: true,
        request_id: RequestId::generate(),
        now_ms,
    }
}

async fn deploy(
    controller: &DeploymentController<'_>,
    request: CreateDeploymentRequest,
) -> open_compute_storage::DeploymentRecord {
    match controller.create_deployment(request).await.unwrap() {
        CreateDeploymentOutcome::Applied(result) => result.deployment,
        CreateDeploymentOutcome::Replay(_) => panic!("unexpected deployment replay"),
    }
}

fn dispatch_target(
    account_id: open_compute_core::AccountId,
    worker_id: open_compute_core::WorkerId,
    deployment: &open_compute_storage::DeploymentRecord,
    entrypoint: Option<&str>,
) -> DispatchTarget {
    DispatchTarget {
        account_id,
        worker_id,
        deployment_id: deployment.id,
        worker_code_sha256: hex::encode(deployment.worker_code_sha256),
        entrypoint: entrypoint.map(str::to_owned),
        route_generation: 1,
        request_id: RequestId::generate(),
    }
}

async fn wait_service_drain(harness: &Harness) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while harness.service_invocations.counts() != (0, 0, 0) {
        assert!(
            Instant::now() < deadline,
            "Service event root did not drain: {:?}",
            harness.service_invocations.counts()
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
