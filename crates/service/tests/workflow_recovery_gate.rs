//! Current Workflow snapshot, process-recovery, transport-fault, and product-binding Gate.

#![cfg(feature = "test-support")]

mod workflow_support;

#[allow(dead_code)]
mod p0_exit_support;

use axum::body::{Body, to_bytes};
use axum::http::Request;
use open_compute_core::{
    BindingKind, CanonicalBindingConfig, CanonicalPermissions, ErrorCode, RequestId, ResourceId,
    SchedulerClock as _, SchedulerConfig, SystemClock, SystemSchedulerClock, WorkflowsConfig,
};
use open_compute_service::runtime_bridge::{
    DispatchTarget, WorkflowDispatchResult, WorkflowOutcome, WorkflowRunRequest,
};
use open_compute_service::scheduler::SchedulerService;
use open_compute_service::workflow_http::WorkflowApiState;
use open_compute_service::{SqliteKvBindingExecutor, serve_binding_backend};
use open_compute_storage::scheduler::{WorkflowCompletion, WorkflowState};
use open_compute_storage::{SchedulerStore, VersionState, WorkflowRepository};
use open_compute_workers::{
    ResourcePins, VersionBindingInput, WorkflowController, WorkflowReconcileCursor,
};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use workflow_support::{Harness, decode_workflow_json, encode_workflow_json};

fn now() -> i64 {
    SystemSchedulerClock.wall_time_ms()
}

async fn request(
    harness: &Harness,
    target: &DispatchTarget,
    path: &str,
    body: serde_json::Value,
) -> serde_json::Value {
    let mut target = target.clone();
    target.entrypoint = None;
    target.request_id = RequestId::generate();
    let response = harness
        .transport
        .dispatch(
            target,
            Request::builder()
                .method("POST")
                .uri(format!("https://workflow.example{path}"))
                .header("content-type", "application/json")
                .header("host", "workflow.example")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap_or_else(|error| panic!("tenant request {path}: {error:?}"));
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&bytes));
    serde_json::from_slice(&bytes).unwrap()
}

fn complete(result: WorkflowDispatchResult) -> (u32, String) {
    match result.result {
        WorkflowOutcome::Complete {
            final_ordinal,
            output_base64,
        } => (final_ordinal, output_base64),
        outcome => panic!("expected complete Workflow result, got {outcome:?}"),
    }
}

fn start_backend(
    harness: &mut Harness,
    store: &Arc<SchedulerStore>,
    config: &WorkflowsConfig,
) -> tokio::task::JoinHandle<Result<(), open_compute_core::PlatformError>> {
    let mut shutdown = harness.shutdown.subscribe();
    tokio::spawn({
        let listener = harness.binding_listener.take().unwrap();
        let storage = harness.storage.clone();
        let auth = harness.binding_auth.clone();
        let store = store.clone();
        let config = config.clone();
        async move {
            serve_binding_backend(
                listener,
                storage.clone(),
                auth,
                ResourcePins::new(),
                Arc::new(SqliteKvBindingExecutor::new(storage, Arc::new(SystemClock))),
                None,
                None,
                None,
                Default::default(),
                Default::default(),
                config,
                Some(store),
                async move {
                    let _ = shutdown.changed().await;
                },
            )
            .await
        }
    })
}

const CALLER: &str = r#"
export default { async fetch(request,env) {
  try {
    const [_,method,id] = new URL(request.url).pathname.split('/');
    const params = await request.json();
    if (method === 'create') {
      const handle = await env.FLOW.create({id,params});
      return Response.json({id:handle.id,...await handle.status()});
    }
    return Response.json(await (await env.FLOW.get(id)).status());
  } catch(error) { return Response.json({error:String(error)}); }
} };
"#;

#[path = "workflow_support/snapshot_restore.rs"]
mod snapshot_restore;

#[path = "workflow_support/platform_process.rs"]
mod platform_process;

#[path = "workflow_support/process_crash.rs"]
mod process_crash;

#[path = "workflow_support/product_bindings.rs"]
mod product_bindings;

#[path = "workflow_support/transport_faults.rs"]
mod transport_faults;
