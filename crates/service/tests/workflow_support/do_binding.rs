//! A real DO can inspect Workflow state, but cannot create an external durable effect.

use super::*;
use open_compute_core::{BindingKind, CanonicalBindingConfig, WorkflowId};
use open_compute_storage::{DO_NAMESPACE_SCHEMA_VERSION, WorkerRepository};
use open_compute_workers::{
    CreateResourceOutcome, CreateResourceRequest, DurableObjectResourceDriver, ResourceController,
};
use serde_json::json;

pub(super) async fn verify(harness: &Harness, definition: WorkflowId) {
    let account = harness.storage.identity().default_account_id;
    let worker = WorkerRepository::new(harness.storage.db())
        .create_worker(
            account,
            "workflow-reader",
            RequestId::generate(),
            now(),
            1_000_000,
        )
        .unwrap()
        .0;
    let driver = DurableObjectResourceDriver::new(&harness.storage, worker.id, "Reader");
    let CreateResourceOutcome::Applied(namespace) =
        ResourceController::new(&harness.storage, ResourcePins::new(), driver)
            .create(&CreateResourceRequest {
                account_id: account,
                kind: BindingKind::DoNamespace,
                name: "workflow-reader".into(),
                idempotency_key: "workflow-reader".into(),
                driver_schema_version: DO_NAMESPACE_SCHEMA_VERSION,
                request_id: RequestId::generate(),
                now_ms: now(),
            })
            .unwrap()
    else {
        panic!("new namespace must be applied");
    };
    let bindings = [
        (
            "FLOW",
            BindingKind::Workflow,
            ResourceId::from_uuid(definition.as_uuid()).unwrap(),
        ),
        ("OBJECTS", BindingKind::DoNamespace, namespace.resource_id),
    ]
    .into_iter()
    .map(|(name, kind, id)| {
        let config = if kind == BindingKind::Workflow {
            CanonicalBindingConfig {
                workflow_class_name: Some("Flow".into()),
                ..Default::default()
            }
        } else {
            CanonicalBindingConfig::default()
        };
        (
            name.into(),
            VersionBindingInput {
                kind,
                id,
                permissions: Default::default(),
                config,
            },
        )
    })
    .collect();
    let caller = harness.deploy_worker(worker.id, SOURCE, "", bindings).await;
    // Reuse the transport while the tenant intentionally leaves each POST body
    // unread; neither RPC binding depends on draining that unrelated input.
    for _ in 0..16 {
        assert_eq!(
            request(harness, &caller, "/status", json!({})).await,
            json!({"id":"first","status":"queued"})
        );
        assert_eq!(
            request(harness, &caller, "/create", json!({})).await,
            json!({"created":true})
        );
    }
    WorkflowRepository::new(harness.storage.db())
        .find_instance(definition, "from-do")
        .unwrap();
}

const SOURCE: &str = r#"
import { DurableObject } from 'cloudflare:workers';
export class Reader extends DurableObject {
  async read() {
    const instance = await this.env.FLOW.get('first');
    return {id:instance.id,...await instance.status()};
  }
  async create() {
    try { await this.env.FLOW.create({id:'from-do'}); }
    catch {}
    return {created:true};
  }
}
export default { async fetch(request,env) {
  if (request.headers.has('connection')) throw new Error('internal hop header exposed');
  const object = env.OBJECTS.getByName('reader');
  return Response.json(new URL(request.url).pathname==='/create'
    ? await object.create() : await object.read());
} };
"#;
