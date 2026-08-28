//! Production catalog/RuntimeSource/binding transport coverage for capability-two callers.

use super::{Harness, now};
use axum::{Router, routing::post};
use open_compute_core::{BindingKind, ErrorCode, ResourceId, WorkflowsConfig};
use open_compute_service::runtime_bridge::DispatchTarget;
use open_compute_service::workflow_backend::WorkflowBindingService;
use open_compute_service::workflow_http::WorkflowApiState;
use open_compute_storage::{SchedulerStore, WorkflowRepository};
use open_compute_workers::{DeploymentBindingInput, WorkflowController};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mixed_callers_use_frozen_capabilities_and_private_instance_handles() {
    let mut harness = Harness::start().await;
    let scheduler = Arc::new(
        SchedulerStore::open(
            &harness.storage.data_dir().ensure_scheduler_db().unwrap(),
            5000,
            now(),
        )
        .unwrap(),
    );
    let service = WorkflowBindingService::new(
        harness.storage.clone(),
        scheduler.clone(),
        WorkflowsConfig::default(),
    )
    .unwrap();
    let auth = harness.binding_auth.clone();
    let listener = harness.binding_listener.take().unwrap();
    let mut shutdown = harness.shutdown.subscribe();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route(
                "/internal/bindings/v1/workflow/{binding}/{operation}",
                post(move |request| {
                    let service = service.clone();
                    let auth = auth.clone();
                    async move { service.handle(request, auth).await }
                }),
            ),
        )
        .with_graceful_shutdown(async move {
            let _ = shutdown.changed().await;
        })
        .await
        .unwrap();
    });
    let flow = harness
        .deploy(
            r#"
        import { WorkflowEntrypoint } from 'cloudflare:workers';
        export class Flow extends WorkflowEntrypoint { async run() { return 1; } }
        export default { fetch() { return new Response('flow'); } };
    "#,
            "Flow",
        )
        .await;
    let account = harness.storage.identity().default_account_id;
    let repository = WorkflowRepository::new(harness.storage.db());
    let definition = repository
        .create_definition(account, "caller-migration", now())
        .unwrap();
    let api = WorkflowApiState::new(
        harness.storage.clone(),
        scheduler.clone(),
        harness.transport.clone(),
        Default::default(),
    );
    api.create_version(account, definition.id, flow.deployment_id, "Flow".into(), 1)
        .await
        .unwrap();
    let config = WorkflowsConfig::default();
    let identity = WorkflowController::new(&harness.storage, &scheduler, &config)
        .create(
            account,
            definition.id,
            1,
            Some("original"),
            open_compute_workers::WorkflowCreateInput {
                payload_json: "null",
                retention: None,
            },
            now(),
        )
        .unwrap();
    let caller = harness
        .deploy_bound(
            CALLER,
            "",
            [1, 2]
                .into_iter()
                .map(|capability| {
                    (
                        format!("FLOW_{capability}"),
                        DeploymentBindingInput {
                            kind: BindingKind::Workflow,
                            id: ResourceId::from_uuid(definition.id.as_uuid()).unwrap(),
                            capability_version: capability,
                            permissions: Default::default(),
                            config: Default::default(),
                        },
                    )
                })
                .collect::<BTreeMap<_, _>>(),
        )
        .await;
    assert_eq!(
        request(&harness, &caller, "read").await,
        json!({
        "legacy":{"status":"queued"},"durable":{"status":"queued"},
        "keys":["id"],"serialized":"{\"id\":\"original\"}"})
    );
    assert_eq!(
        request(&harness, &caller, "mismatch").await,
        json!({"error":"WORKFLOW_CAPABILITY_MISMATCH"})
    );
    assert_eq!(
        repository
            .find_instance(definition.id, "new")
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowInstanceNotFound
    );
    assert_eq!(
        request(&harness, &caller, "mutate-legacy").await,
        json!({"error":"WORKFLOW_METHOD_UNSUPPORTED"})
    );
    // A new execution capability never changes old bindings or old instance identity.
    api.create_version(account, definition.id, flow.deployment_id, "Flow".into(), 2)
        .await
        .unwrap();
    assert_eq!(
        request(&harness, &caller, "old-create").await,
        json!({"error":"WORKFLOW_CAPABILITY_MISMATCH"})
    );
    assert_eq!(
        repository
            .find_instance(definition.id, "original")
            .unwrap()
            .identity,
        identity
    );
    assert_eq!(
        request(&harness, &caller, "read").await["durable"]["status"],
        "queued"
    );
    harness.restart().await;
    assert_eq!(
        request(&harness, &caller, "read").await["durable"]["status"],
        "queued"
    );
    harness.stop().await;
    server.await.unwrap();
}

async fn request(harness: &Harness, target: &DispatchTarget, operation: &str) -> Value {
    let mut target = target.clone();
    target.entrypoint = None;
    let response = tokio::time::timeout(
        Duration::from_secs(10),
        harness.transport.dispatch(
            target,
            axum::http::Request::builder()
                .method("POST")
                .uri(format!("https://caller.invalid/{operation}"))
                .header("host", "caller.invalid")
                .body(axum::body::Body::empty())
                .unwrap(),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 8192)
            .await
            .unwrap(),
    )
    .unwrap()
}

const CALLER: &str = r#"
export default { async fetch(request,env) {
  try {
    await request.arrayBuffer();
    const operation = new URL(request.url).pathname;
    if (operation === '/mismatch') await env.FLOW_2.create({id:'new'});
    if (operation === '/old-create') await env.FLOW_1.create({id:'new'});
    if (operation === '/mutate-legacy') await (await env.FLOW_2.get('original')).restart();
    const old = await env.FLOW_1.get('original');
    const durable = await env.FLOW_2.get('original');
    return Response.json({legacy:await old.status(),durable:await durable.status(),
      keys:Object.keys(durable),serialized:JSON.stringify(durable)});
  } catch(error) { return Response.json({error:error.message}); }
} };
"#;
