//! Real HTTP observations around production step transactions, never a mock persistence engine.

use super::*;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse as _, Response};
use open_compute_runtime::GenerationAuthRegistry;
use open_compute_service::workflow_backend::WorkflowBindingService;
use open_compute_storage::scheduler::ClaimedWorkflowRun;
use serde_json::{Value, json};
use std::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Observation {
    Known,
    Unknown,
}

#[derive(Clone, Copy, Debug)]
struct Fault {
    operation: &'static str,
    after_commit: bool,
    observation: Observation,
}

#[derive(Default)]
struct Trace {
    fault: Option<Fault>,
    // Every executed callback sends exactly one success/failure request. Replay sends neither.
    callback_reports: usize,
}

#[derive(Clone)]
struct Backend {
    service: WorkflowBindingService,
    auth: GenerationAuthRegistry,
    trace: Arc<Mutex<Trace>>,
}

async fn handle(State(state): State<Backend>, request: Request<Body>) -> Response {
    let operation = request.uri().path().rsplit('/').next().unwrap();
    let fault = {
        let mut trace = state.trace.lock().unwrap();
        if matches!(operation, "success" | "failure") {
            trace.callback_reports += 1;
        }
        if trace
            .fault
            .is_some_and(|fault| fault.operation == operation)
        {
            trace.fault.take()
        } else {
            None
        }
    };
    if let Some(fault) = fault
        && !fault.after_commit
    {
        // Consume the request even when cutting before the transaction: keep-alive must not
        // accidentally turn the intended Known response into an unrelated transport failure.
        to_bytes(request.into_body(), 2 * 1024 * 1024 + 8192)
            .await
            .unwrap();
        return cut(fault.observation);
    }
    let response = state.service.handle(request, state.auth).await;
    if let Some(fault) = fault {
        assert_eq!(response.status(), StatusCode::OK, "{fault:?}");
        if fault.observation == Observation::Unknown {
            // The real authority committed, but its successful HTTP reply never reaches workerd.
            return cut(Observation::Unknown);
        }
    }
    response
}

fn cut(observation: Observation) -> Response {
    match observation {
        Observation::Known => (
            StatusCode::TOO_MANY_REQUESTS,
            [("x-open-compute-error-code", "WORKFLOW_STATE_QUOTA_EXCEEDED")],
        )
            .into_response(),
        Observation::Unknown => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

fn envelope(run: &ClaimedWorkflowRun) -> WorkflowRunRequest {
    WorkflowRunRequest {
        fence: run.fence.clone(),
        external_instance_id: run.external_instance_id.clone(),
        definition_name: run.target.definition_name.clone(),
        created_at_ms: run.created_at_ms,
        payload_base64: run.input_json.clone(),
        rollback: run.rollback,
        schedule: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workflow_fixture_drop_waits_for_child_reaping() {
    let harness = Harness::start().await;
    let supervisor = harness.supervisor.clone();
    let pid = supervisor.snapshot().pid;
    assert!(pid.is_some());
    drop(harness);
    assert_eq!(
        supervisor.snapshot().state,
        open_compute_runtime::SupervisorState::Stopped
    );
    assert_eq!(supervisor.owner_registry_len(), 0);
    open_compute_runtime::process::assert_reaped(pid).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn workflow_production_step_http_known_unknown_commit_matrix() {
    // Each operation owns a fresh supervisor: its two intentional crashes must not
    // consume another operation's rolling restart budget.
    for operation in ["claim-batch", "success", "failure"] {
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
        let trace = Arc::new(Mutex::new(Trace::default()));
        let backend = Backend {
            service: WorkflowBindingService::new(
                harness.storage.clone(),
                store.clone(),
                config.clone(),
            )
            .unwrap(),
            auth: harness.binding_auth.clone(),
            trace: trace.clone(),
        };
        let mut shutdown = harness.shutdown.subscribe();
        let listener = harness.binding_listener.take().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, Router::new().fallback(handle).with_state(backend))
                .with_graceful_shutdown(async move {
                    let _ = shutdown.changed().await;
                })
                .await
                .unwrap();
        });
        let account = harness.storage.identity().default_account_id;
        let definition = WorkflowRepository::new(harness.storage.db())
            .create_definition(account, "transport-matrix", now())
            .unwrap();
        let target = harness.deploy(SOURCE, "Flow").await;
        let version = WorkflowApiState::new(
            harness.storage.clone(),
            store.clone(),
            harness.transport.clone(),
            Default::default(),
        )
        .create_version(account, definition.id, target.version_id, "Flow".into())
        .await
        .unwrap();
        assert_eq!(version.state, VersionState::Ready);

        for after_commit in [false, true] {
            for observation in [Observation::Known, Observation::Unknown] {
                let fault = Fault {
                    operation,
                    after_commit,
                    observation,
                };
                eprintln!("Workflow step transport {fault:?}");
                *trace.lock().unwrap() = Trace {
                    fault: Some(fault),
                    ..Trace::default()
                };
                let controller = WorkflowController::new(&harness.storage, &store, &config);
                let input = if operation == "failure" {
                    json!({"fail":true})
                } else {
                    Value::Null
                };
                let input = encode_workflow_json(&input);
                let id = controller
                    .create(
                        account,
                        definition.id,
                        open_compute_core::WorkflowOperationId::generate(),
                        None,
                        open_compute_workers::WorkflowCreateInput {
                            payload_base64: &input,
                            retention: None,
                            schedule: None,
                        },
                        now(),
                    )
                    .unwrap()
                    .external_instance_id;
                let run = controller
                    .claim(now(), &mut Default::default())
                    .unwrap()
                    .unwrap();
                assert_eq!(run.external_instance_id, id);
                let result = harness
                    .transport
                    .dispatch_workflow(
                        &version.target,
                        &envelope(&run),
                        Duration::from_millis(config.dispatch_timeout_ms),
                    )
                    .await;
                assert!(trace.lock().unwrap().fault.is_none(), "{fault:?}");
                let record = store
                    .workflow_instance(run.fence.instance_id)
                    .unwrap()
                    .unwrap();
                assert_eq!(record.state, WorkflowState::Running, "{fault:?}");
                assert_eq!(record.run_token.as_ref(), Some(&run.fence.run_token));

                let (active_run, result) = if observation == Observation::Unknown {
                    assert!(
                        result.is_err(),
                        "Unknown must not become terminal: {fault:?}"
                    );
                    let steps = store
                        .workflow_steps(run.fence.instance_id, None, 10)
                        .unwrap();
                    let expected_state = match (operation, after_commit) {
                        ("claim-batch", false) => None,
                        ("success", true) => Some("complete"),
                        ("failure", true) => Some("failed"),
                        _ => Some("running"),
                    };
                    assert_eq!(
                        steps.first().map(|step| step.state.as_str()),
                        expected_state
                    );
                    assert!(
                        controller
                            .claim(now(), &mut Default::default())
                            .unwrap()
                            .is_none()
                    );
                    store
                        .verify_workflow_history(run.fence.instance_id)
                        .unwrap();
                    // Drop the entire loaded isolate, then recover only after the persisted lease.
                    harness.restart().await;
                    let expired_at = record.run_lease_until_ms.unwrap();
                    assert_eq!(store.recover_workflows(expired_at, &config, 32).unwrap(), 1);
                    let replay = WorkflowController::new(&harness.storage, &store, &config)
                        .claim(
                            expired_at + i64::try_from(config.recovery_backoff_ms).unwrap(),
                            &mut Default::default(),
                        )
                        .unwrap()
                        .unwrap();
                    assert_ne!(replay.fence.run_token, run.fence.run_token);
                    assert_eq!(
                        store
                            .finish_workflow(
                                &run.fence,
                                &WorkflowCompletion::Errored {
                                    code: ErrorCode::WorkflowExecutionFailed,
                                },
                                now(),
                                &config,
                            )
                            .unwrap_err()
                            .code(),
                        ErrorCode::WorkflowRunStale
                    );
                    let result = harness
                        .transport
                        .dispatch_workflow(
                            &version.target,
                            &envelope(&replay),
                            Duration::from_millis(config.dispatch_timeout_ms),
                        )
                        .await
                        .unwrap();
                    let expected_reports = if operation != "claim-batch" && !after_commit {
                        2
                    } else {
                        1
                    };
                    assert_eq!(
                        trace.lock().unwrap().callback_reports,
                        expected_reports,
                        "{fault:?}"
                    );
                    (replay, result)
                } else {
                    let result = result.unwrap();
                    let expected_reports = usize::from(operation != "claim-batch" || after_commit);
                    assert_eq!(
                        trace.lock().unwrap().callback_reports,
                        expected_reports,
                        "{fault:?}"
                    );
                    (run, result)
                };
                let expected_error =
                    operation == "failure" || (observation == Observation::Known && !after_commit);
                let completion = match result.result {
                    WorkflowOutcome::Errored { error_code, .. } => {
                        assert!(expected_error, "{fault:?}");
                        WorkflowCompletion::Errored {
                            code: open_compute_core::workflow::terminal_error_code(&error_code)
                                .unwrap(),
                        }
                    }
                    WorkflowOutcome::Complete {
                        output_base64,
                        final_ordinal,
                    } => {
                        assert!(!expected_error, "{fault:?}");
                        let output = decode_workflow_json(&output_base64);
                        assert_eq!(output["id"], id);
                        let replayed = observation == Observation::Unknown
                            && operation == "success"
                            && after_commit;
                        assert_eq!(output["callbacks"], if replayed { 0.0 } else { 1.0 });
                        WorkflowCompletion::Complete {
                            output_json: output_base64,
                            final_ordinal,
                        }
                    }
                    outcome => panic!("unexpected Workflow outcome {outcome:?}: {fault:?}"),
                };
                let state = store
                    .finish_workflow(&active_run.fence, &completion, now(), &config)
                    .unwrap();
                assert!(state.is_terminal());
                let repository = WorkflowRepository::new(harness.storage.db());
                let identity = repository
                    .find_instance(definition.id, &id)
                    .unwrap()
                    .identity;
                assert!(repository.instance_referrers_intact(&identity).unwrap());
                WorkflowController::new(&harness.storage, &store, &config)
                    .reconcile(&mut WorkflowReconcileCursor::default(), 32, now())
                    .unwrap();
                assert!(repository.instance_referrers_intact(&identity).unwrap());
                store.verify_workflow_history(identity.instance_id).unwrap();
                let reopened = SchedulerStore::open(
                    &harness.storage.data_dir().scheduler_db_path(),
                    5000,
                    now(),
                )
                .unwrap();
                assert_eq!(
                    reopened
                        .workflow_instance(identity.instance_id)
                        .unwrap()
                        .unwrap()
                        .state,
                    state
                );
            }
        }
        // The public stable ID remains recoverable when the create response is lost;
        // a failure before the actual transaction must leave no reservation behind.
        let caller = harness
            .deploy_bound(
                CALLER,
                "",
                BTreeMap::from([(
                    "FLOW".into(),
                    VersionBindingInput {
                        kind: BindingKind::Workflow,
                        id: ResourceId::from_uuid(definition.id.as_uuid()).unwrap(),
                        permissions: CanonicalPermissions::default(),
                        config: CanonicalBindingConfig::default(),
                    },
                )]),
            )
            .await;
        for after_commit in [false, true] {
            for observation in [Observation::Known, Observation::Unknown] {
                let id = RequestId::generate().to_string();
                let fault = Fault {
                    operation: "create",
                    after_commit,
                    observation,
                };
                eprintln!("Workflow create transport {fault:?}");
                trace.lock().unwrap().fault = Some(fault);
                let response =
                    request(&harness, &caller, &format!("/create/{id}"), Value::Null).await;
                assert!(trace.lock().unwrap().fault.is_none());
                let repository = WorkflowRepository::new(harness.storage.db());
                if after_commit {
                    let reservation = repository.find_instance(definition.id, &id).unwrap();
                    assert_eq!(
                        reservation.state,
                        open_compute_storage::WorkflowRefState::Live
                    );
                    assert!(
                        repository
                            .instance_referrers_intact(&reservation.identity)
                            .unwrap()
                    );
                    assert_eq!(
                        store
                            .workflow_instance(reservation.identity.instance_id)
                            .unwrap()
                            .unwrap()
                            .state,
                        WorkflowState::Queued
                    );
                    let duplicate =
                        request(&harness, &caller, &format!("/create/{id}"), Value::Null).await;
                    assert!(
                        duplicate["error"]
                            .as_str()
                            .unwrap()
                            .contains("WORKFLOW_INSTANCE_ALREADY_EXISTS")
                    );
                } else {
                    assert_eq!(
                        repository
                            .find_instance(definition.id, &id)
                            .unwrap_err()
                            .code(),
                        ErrorCode::WorkflowInstanceNotFound
                    );
                    let retry =
                        request(&harness, &caller, &format!("/create/{id}"), Value::Null).await;
                    assert_eq!(retry["status"], "queued", "{retry}");
                }
                assert_eq!(
                    request(&harness, &caller, &format!("/status/{id}"), Value::Null).await["status"],
                    "queued"
                );
                if after_commit && observation == Observation::Known {
                    assert_eq!(response["status"], "queued");
                } else {
                    assert!(response["error"].is_string(), "{fault:?}: {response}");
                }
            }
        }
        harness.stop().await;
        server.await.unwrap();
    }
}

const SOURCE: &str = r#"
import { WorkflowEntrypoint } from 'cloudflare:workers';
export class Flow extends WorkflowEntrypoint {
  async run(event, step) {
    let callbacks = 0;
    const value = await step.do('durable', {timeout:240000,retries:{limit:0,delay:0}}, () => {
      callbacks++;
      if (event.payload?.fail) throw new Error('private callback error');
      return { id: event.instanceId, nonce: crypto.randomUUID() };
    });
    return { ...value, callbacks };
  }
}
export default { fetch() { return new Response('workflow'); } };
"#;
