//! Real stock-workerd, `SigV4` artifact, two-database, and production backend Workflow Gate.

#![cfg(feature = "test-support")]

mod workflow_support;

#[allow(dead_code)]
mod p0_exit_support;

use axum::body::{Body, to_bytes};
use axum::http::Request;
use open_compute_artifacts::ArtifactRef;
use open_compute_core::{
    BindingKind, CanonicalBindingConfig, CanonicalPermissions, ErrorCode, RequestId, ResourceId,
    SchedulerClock as _, SchedulerConfig, SystemClock, SystemSchedulerClock, WorkflowsConfig,
};
use open_compute_service::runtime_bridge::{DispatchTarget, WorkflowRunRequest};
use open_compute_service::scheduler::SchedulerService;
use open_compute_service::workflow_http::WorkflowApiState;
use open_compute_service::{SqliteKvBindingExecutor, serve_binding_backend_with_scheduler};
use open_compute_storage::scheduler::{WorkflowCompletion, WorkflowState};
use open_compute_storage::{DeploymentState, SchedulerStore, WorkerRepository, WorkflowRepository};
use open_compute_workers::{
    DeploymentBindingInput, ResourcePins, WorkflowController, WorkflowReconcileCursor,
};
use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use workflow_support::Harness;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn production_workflow_binding_frozen_versions_replay_and_terminal_history() {
    let mut harness = Harness::start().await;
    let store = Arc::new(
        SchedulerStore::open(
            &harness.storage.data_dir().ensure_scheduler_db().unwrap(),
            5000,
            now(),
        )
        .unwrap(),
    );
    let config = WorkflowsConfig::default();
    let backend = start_backend(&mut harness, &store, &config);
    let account = harness.storage.identity().default_account_id;
    let repository = WorkflowRepository::new(harness.storage.db());
    let definition = repository
        .create_definition(account, "durable-pipeline", now())
        .unwrap();
    let api = WorkflowApiState::new(
        harness.storage.clone(),
        store.clone(),
        harness.transport.clone(),
        Default::default(),
    );
    let first_target = harness.deploy(FLOW, "Flow").await;
    let first = api
        .create_version(
            account,
            definition.id,
            first_target.deployment_id,
            "Flow".into(),
            1,
        )
        .await
        .unwrap();
    assert_eq!(first.state, DeploymentState::Ready);
    let caller = harness
        .deploy_bound(
            CALLER,
            "",
            BTreeMap::from([(
                "FLOW".into(),
                DeploymentBindingInput {
                    capability_version: 1,
                    kind: BindingKind::Workflow,
                    id: ResourceId::from_uuid(definition.id.as_uuid()).unwrap(),
                    permissions: CanonicalPermissions::default(),
                    config: CanonicalBindingConfig::default(),
                },
            )]),
        )
        .await;
    let a = request(
        &harness,
        &caller,
        "/create/first",
        serde_json::json!({"value":7}),
    )
    .await;
    assert_eq!(a, serde_json::json!({"id":"first","status":"queued"}));
    do_binding::verify(&harness, definition.id).await;
    let duplicate = request(&harness, &caller, "/create/first", serde_json::json!({})).await;
    assert!(
        duplicate["error"]
            .as_str()
            .unwrap()
            .contains("WORKFLOW_INSTANCE_ALREADY_EXISTS")
    );
    let missing = request(&harness, &caller, "/status/absent", serde_json::json!({})).await;
    assert!(
        missing["error"]
            .as_str()
            .unwrap()
            .contains("WORKFLOW_INSTANCE_NOT_FOUND")
    );

    // Rejected replacement cannot disturb the current version.
    let rejected = api
        .create_version(
            account,
            definition.id,
            first_target.deployment_id,
            "Missing".into(),
            1,
        )
        .await
        .unwrap();
    assert_eq!(rejected.state, DeploymentState::Rejected);
    assert_eq!(
        repository
            .definition(account, definition.id)
            .unwrap()
            .current_version_id,
        Some(first.target.version_id)
    );
    let second_target = harness
        .deploy_worker(
            first_target.worker_id,
            &FLOW.replace("revision: 1", "revision: 2"),
            "Flow",
            BTreeMap::new(),
        )
        .await;
    let second = api
        .create_version(
            account,
            definition.id,
            second_target.deployment_id,
            "Flow".into(),
            1,
        )
        .await
        .unwrap();
    assert_eq!(second.state, DeploymentState::Ready);
    assert_eq!(
        request(
            &harness,
            &caller,
            "/create/second",
            serde_json::json!({"value":11})
        )
        .await["status"],
        "queued"
    );
    let first_instance = repository.find_instance(definition.id, "first").unwrap();
    let second_instance = repository.find_instance(definition.id, "second").unwrap();
    assert_eq!(
        first_instance.identity.target.version_id,
        first.target.version_id
    );
    assert_eq!(
        second_instance.identity.target.version_id,
        second.target.version_id
    );
    assert!(
        repository
            .instance_referrers_intact(&first_instance.identity)
            .unwrap()
    );
    let deployments = WorkerRepository::new(harness.storage.db());
    deployments.prune_expired_idempotency(now(), 100).unwrap();
    // V1 is neither active nor protected by an idempotency response: the live
    // Workflow's durable version/instance pins must be the deletion barrier.
    assert_eq!(
        deployments
            .begin_deployment_delete(account, first_target.worker_id, first_target.deployment_id)
            .unwrap_err()
            .code(),
        ErrorCode::DeploymentReferenced
    );
    let artifact_refs = || {
        deployments
            .referenced_artifacts()
            .unwrap()
            .into_iter()
            .map(|(digest, size)| ArtifactRef::new(1, &hex::encode(digest), size).unwrap())
            .collect::<HashSet<_>>()
    };
    assert_eq!(
        harness
            .artifacts
            .gc_unreferenced(&artifact_refs(), SystemTime::now() + Duration::from_secs(1))
            .await
            .unwrap(),
        0
    );

    let scheduler = Arc::new(SchedulerService::new(
        store.clone(),
        harness.storage.clone(),
        harness.transport.clone(),
        SchedulerConfig::default(),
        config.clone(),
        Arc::new(SystemSchedulerClock),
    ));
    assert_eq!(scheduler.poll_once().await.unwrap(), 2);
    assert_eq!(
        request(&harness, &caller, "/status/first", serde_json::json!({})).await,
        serde_json::json!({"status":"complete","output":{"revision":1,"value":14,"mode":"frozen","name":"durable-pipeline"}})
    );
    assert_eq!(
        request(&harness, &caller, "/status/second", serde_json::json!({})).await["output"]["revision"],
        2
    );
    for identity in [&first_instance.identity, &second_instance.identity] {
        store.verify_workflow_history(identity.instance_id).unwrap();
        assert!(!repository.instance_referrers_intact(identity).unwrap());
    }
    scheduler.repair_workflows(32).unwrap();
    deployments
        .tombstone_deployment(
            account,
            first_target.worker_id,
            first_target.deployment_id,
            RequestId::generate(),
            now(),
        )
        .unwrap();
    assert_eq!(
        harness
            .artifacts
            .gc_unreferenced(&artifact_refs(), SystemTime::now() + Duration::from_secs(1))
            .await
            .unwrap(),
        1
    );
    // Terminal history remains authoritative after its frozen artifact is gone.
    assert_eq!(
        request(&harness, &caller, "/status/first", serde_json::json!({})).await["output"]["revision"],
        1
    );
    store
        .verify_workflow_history(first_instance.identity.instance_id)
        .unwrap();

    assert_eq!(
        request(
            &harness,
            &caller,
            "/create/primordials",
            serde_json::json!({"primordials":true})
        )
        .await["status"],
        "queued"
    );
    assert_eq!(scheduler.poll_once().await.unwrap(), 1);
    assert_eq!(
        request(
            &harness,
            &caller,
            "/status/primordials",
            serde_json::json!({})
        )
        .await,
        serde_json::json!({"status":"complete","output":{"observedPrivateGrant":false}})
    );

    assert_eq!(
        request(
            &harness,
            &caller,
            "/create/caught",
            serde_json::json!({"fail":true})
        )
        .await["status"],
        "queued"
    );
    assert_eq!(scheduler.poll_once().await.unwrap(), 1);
    assert_eq!(
        request(&harness, &caller, "/status/caught", serde_json::json!({})).await,
        serde_json::json!({"status":"errored","error":{"name":"Error","message":"Workflow execution failed"}})
    );
    // Capacity belongs to the real authority, not the feasibility probe's test backend.
    for (id, payload, expected_error) in [
        ("step-limit", serde_json::json!({"steps":1024}), None),
        (
            "step-overflow",
            serde_json::json!({"steps":1025}),
            Some("WORKFLOW_STEP_LIMIT_EXCEEDED"),
        ),
        (
            "result-limit",
            serde_json::json!({"resultBytes":1048576}),
            None,
        ),
        (
            "result-overflow",
            serde_json::json!({"resultBytes":1048577}),
            Some("WORKFLOW_RESULT_TOO_LARGE"),
        ),
    ] {
        assert_eq!(
            request(&harness, &caller, &format!("/create/{id}"), payload).await["status"],
            "queued"
        );
        assert_eq!(scheduler.poll_once().await.unwrap(), 1);
        let identity = repository.find_instance(definition.id, id).unwrap();
        let record = store
            .workflow_instance(identity.identity.instance_id)
            .unwrap()
            .unwrap();
        assert_eq!(record.error_code.as_deref(), expected_error);
        assert_eq!(
            record.state,
            if expected_error.is_some() {
                WorkflowState::Errored
            } else {
                WorkflowState::Complete
            }
        );
        store
            .verify_workflow_history(identity.identity.instance_id)
            .unwrap();
    }

    // Lose the terminal response after real private step commits, then replay from durable history.
    let controller = WorkflowController::new(&harness.storage, &store, &config);
    controller
        .create(
            account,
            definition.id,
            1,
            Some("lost-response"),
            open_compute_workers::WorkflowCreateInput {
                payload_json: "{\"value\":13}",
                retention: None,
            },
            now(),
        )
        .unwrap();
    let run = controller
        .claim(now(), &mut Default::default())
        .unwrap()
        .unwrap();
    let envelope = WorkflowRunRequest {
        fence: run.fence.clone(),
        external_instance_id: run.external_instance_id.clone(),
        definition_name: run.target.definition_name.clone(),
        created_at_ms: run.created_at_ms,
        payload_json: run.input_json.clone(),
    };
    let dispatch = harness
        .transport
        .dispatch_workflow(&second_target, &envelope, Duration::from_secs(30))
        .await
        .unwrap();
    assert_eq!(dispatch.outcome, "complete");
    assert_eq!(
        store
            .workflow_instance(run.fence.instance_id)
            .unwrap()
            .unwrap()
            .completed_step_count,
        2
    );
    assert_eq!(
        store
            .workflow_instance(run.fence.instance_id)
            .unwrap()
            .unwrap()
            .state,
        WorkflowState::Running
    );
    assert!(harness.cache.entry_count() > 0);
    harness.cache.evict_if_needed().await.unwrap();
    assert_eq!(harness.cache.entry_count(), 0);
    harness.restart().await;
    let recovered_at =
        now() + i64::try_from(config.lease_ms + config.recovery_backoff_ms + 1).unwrap();
    store.recover_workflows(recovered_at, &config, 32).unwrap();
    let replay = controller
        .claim(
            recovered_at + i64::try_from(config.recovery_backoff_ms).unwrap(),
            &mut Default::default(),
        )
        .unwrap()
        .unwrap();
    assert_ne!(replay.fence.run_token, run.fence.run_token);
    assert_eq!(
        store
            .finish_workflow(
                &run.fence,
                &WorkflowCompletion::Complete {
                    output_json: "null".into(),
                    final_ordinal: 2
                },
                recovered_at,
                &config
            )
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowRunStale
    );
    let replay_envelope = WorkflowRunRequest {
        fence: replay.fence.clone(),
        external_instance_id: replay.external_instance_id,
        definition_name: replay.target.definition_name,
        created_at_ms: replay.created_at_ms,
        payload_json: replay.input_json,
    };
    let result = harness
        .transport
        .dispatch_workflow(&second_target, &replay_envelope, Duration::from_secs(30))
        .await
        .unwrap();
    assert_eq!(result.output_json, dispatch.output_json);
    assert!(harness.cache.entry_count() > 0);
    store
        .finish_workflow(
            &replay.fence,
            &WorkflowCompletion::Complete {
                output_json: result.output_json.unwrap(),
                final_ordinal: result.final_ordinal,
            },
            recovered_at + 1001,
            &config,
        )
        .unwrap();
    controller
        .reconcile(
            &mut WorkflowReconcileCursor::default(),
            32,
            recovered_at + 1002,
        )
        .unwrap();
    store
        .verify_workflow_history(replay.fence.instance_id)
        .unwrap();
    assert_eq!(
        store
            .workflow_steps(replay.fence.instance_id, None, 100)
            .unwrap()
            .len(),
        2
    );
    let reopened = SchedulerStore::open(
        &harness.storage.data_dir().scheduler_db_path(),
        5000,
        recovered_at,
    )
    .unwrap();
    assert_eq!(
        reopened
            .workflow_instance(replay.fence.instance_id)
            .unwrap()
            .unwrap()
            .output_json,
        dispatch.output_json
    );
    let inspection = reopened
        .inspect_workflow_instances(account, definition.id, None, 100, recovered_at)
        .unwrap();
    let inspection = serde_json::to_string(&inspection).unwrap();
    for forbidden in [
        "payloadJson",
        "outputJson",
        "runToken",
        "stepToken",
        "creationNonce",
    ] {
        assert!(!inspection.contains(forbidden));
    }
    drop(reopened);
    harness.stop().await;
    backend.await.unwrap().unwrap();
}

fn now() -> i64 {
    SystemSchedulerClock.wall_time_ms()
}

#[path = "workflow_support/do_binding.rs"]
mod do_binding;

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
        .unwrap_or_else(|error| {
            panic!(
                "tenant request {path}: {error:?}; {:?} {:?}",
                harness.supervisor.snapshot(),
                harness.supervisor.last_diagnostics()
            )
        });
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&bytes));
    serde_json::from_slice(&bytes).unwrap()
}

const CALLER: &str = r#"
export default { async fetch(request,env) {
  try {
    const [_,method,id] = new URL(request.url).pathname.split('/');
    // This fixture sends JSON POSTs for both methods. Finish their input stream
    // before RPC so response-loss injection concerns the Workflow transport only.
    const params = await request.json();
    if (method === 'create') {
      const handle = await env.FLOW.create({id,params});
      return Response.json({id:handle.id,...await handle.status()});
    }
    return Response.json(await (await env.FLOW.get(id)).status());
  } catch(error) { return Response.json({error:String(error)}); }
} };
"#;

const FLOW: &str = concat!(
    include_str!("workflow_support/promise-probe.js"),
    r#"
import { WorkflowEntrypoint } from 'cloudflare:workers';
export class Flow extends WorkflowEntrypoint {
  async run(event,step) {
    if (event.payload.primordials) return observeWorkflowGrant(step);
    if (event.payload.steps) {
      for (let i=0;i<event.payload.steps;i++) await step.do('same-name',()=>i);
      return event.payload.steps;
    }
    if (event.payload.resultBytes) {
      return step.do('large-result',()=> 'x'.repeat(event.payload.resultBytes-2));
    }
    if (event.payload.fail) {
      try { await step.do('fail',()=>{throw new Error('private-tenant-exception');}); } catch {}
      return 'must not be accepted';
    }
    const value = await step.do('double',()=>event.payload.value * 2);
    const output = await step.do('result',()=>({revision: 1,value,mode:this.env.MODE,name:event.workflowName}));
    return output;
  }
}
export default { fetch(){ return new Response('workflow target'); } };
"#
);

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
            serve_binding_backend_with_scheduler(
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
