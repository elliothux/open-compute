use super::*;
use crate::workflow_http::tests::{Fixture, fixture};
use open_compute_core::{RequestId, SecretString, WorkflowId};
use open_compute_storage::scheduler::{WorkflowCompletion, WorkflowState};
use open_compute_storage::{
    NewDeployment, NewDeploymentProducts, WorkerRepository, WorkflowBindingRecord,
};
use serde_json::json;

fn ready(f: &Fixture) -> (WorkflowId, WorkflowBindingRecord) {
    let repository = WorkflowRepository::new(f.storage.db());
    let definition = repository
        .create_definition(f.account, "backend", 0)
        .unwrap();
    let version = repository
        .stage_version(f.account, definition.id, f.deployment, "Flow", 1)
        .unwrap();
    repository
        .finish_version(f.account, version.target.version_id, true, 2)
        .unwrap();
    (definition.id, ready_binding(f, definition.id))
}

fn ready_binding(f: &Fixture, definition: WorkflowId) -> WorkflowBindingRecord {
    let repository = WorkflowRepository::new(f.storage.db());
    let workers = WorkerRepository::new(f.storage.db());
    let (worker, _) = workers
        .create_worker(
            f.account,
            &format!("caller-{}", RequestId::generate()),
            RequestId::generate(),
            0,
            1_000_000,
        )
        .unwrap();
    let deployment = DeploymentId::generate();
    let binding = repository
        .prepare_binding(f.account, deployment, "FLOW", definition, 3)
        .unwrap();
    workers
        .insert_staging_deployment(
            &NewDeployment {
                id: deployment,
                account_id: f.account,
                worker_id: worker.id,
                content_kind: open_compute_storage::DeploymentContentKind::Worker,
                artifact_sha256: Some([3; 32]),
                artifact_size: Some(100),
                artifact_schema_version: Some(1),
                main_module: Some("index.js".into()),
                compatibility_date: "2026-08-26".into(),
                compatibility_flags: vec![],
                limits: json!({"profile":"default"}),
                worker_code_sha256: [4; 32],
                vars: Default::default(),
                secrets: Default::default(),
                request_id: RequestId::generate(),
                now_ms: 3,
            },
            &NewDeploymentProducts {
                workflow_bindings: std::slice::from_ref(&binding),
                ..Default::default()
            },
            100,
        )
        .unwrap();
    workers.begin_validation(deployment).unwrap();
    workers.mark_ready(deployment, 4).unwrap();
    binding
}

fn caller(binding: &WorkflowBindingRecord) -> HeaderMap {
    HeaderMap::from_iter([
        (
            HeaderName::from_static("x-open-compute-deployment-id"),
            HeaderValue::from_str(&binding.deployment_id.to_string()).unwrap(),
        ),
        (
            HeaderName::from_static("x-open-compute-descriptor-sha256"),
            HeaderValue::from_str(&hex::encode(binding.descriptor_sha256)).unwrap(),
        ),
        (
            HeaderName::from_static("x-open-compute-workflow-do-context"),
            HeaderValue::from_static("0"),
        ),
    ])
}

fn body(fence: &WorkflowFence, fields: Value) -> Value {
    let Value::Object(mut body) = fields else {
        panic!("test body must be an object");
    };
    body.extend(
        serde_json::to_value(fence)
            .unwrap()
            .as_object()
            .unwrap()
            .clone(),
    );
    Value::Object(body)
}

#[test]
fn workflow_caller_uses_current_definition_scope_and_strict_handles() {
    let f = fixture();
    let (definition, binding) = ready(&f);
    let second = ready_binding(&f, definition);
    let service = WorkflowBindingService::new(
        f.storage.clone(),
        f.scheduler.clone(),
        WorkflowsConfig::default(),
    )
    .unwrap();
    let path = |binding: &WorkflowBindingRecord, operation: &str| {
        format!(
            "/internal/bindings/v1/workflow/{}/{operation}",
            binding.descriptor.binding_id
        )
    };
    let created = service
        .execute(
            &path(&binding, "create"),
            &caller(&binding),
            json!({"id":"original","payloadJson":"null"}),
            10,
        )
        .unwrap();
    let instance_id: WorkflowInstanceId = created["instanceId"].as_str().unwrap().parse().unwrap();
    assert_eq!(created["id"], "original");
    assert_eq!(
        service
            .execute(
                &path(&second, "get"),
                &caller(&second),
                json!({"id":"original"}),
                11,
            )
            .unwrap(),
        json!({"id":"original","instanceId":instance_id})
    );
    assert_eq!(
        service
            .execute(
                &path(&second, "status"),
                &caller(&second),
                json!({"instanceId":instance_id}),
                11,
            )
            .unwrap(),
        json!({"status":"queued"})
    );
    for invalid in [
        json!({"id":"original"}),
        json!({"instanceId":instance_id,"id":"original"}),
    ] {
        assert_eq!(
            service
                .execute(&path(&second, "status"), &caller(&second), invalid, 11)
                .unwrap_err()
                .code(),
            ErrorCode::WorkflowSerializationUnsupported
        );
    }
    let mut do_headers = caller(&second);
    do_headers.insert(
        "x-open-compute-workflow-do-context",
        HeaderValue::from_static("1"),
    );
    for method in [
        "create",
        "send-event",
        "pause",
        "resume",
        "terminate",
        "restart",
    ] {
        assert_eq!(
            service
                .execute(&path(&second, method), &do_headers, json!({}), 11)
                .unwrap_err()
                .code(),
            ErrorCode::WorkflowDoOutputGateUnsupported
        );
    }
    assert_eq!(
        service
            .execute(
                &path(&second, "status"),
                &do_headers,
                json!({"instanceId":instance_id}),
                11,
            )
            .unwrap()["status"],
        "queued"
    );
    let repository = WorkflowRepository::new(f.storage.db());
    let foreign = repository
        .create_definition(f.account, "foreign", 12)
        .unwrap();
    assert_eq!(
        WorkflowController::new(&f.storage, &f.scheduler, &WorkflowsConfig::default())
            .status(f.account, foreign.id, instance_id, 12)
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowInstanceNotFound
    );
}

#[test]
fn workflow_backend_binding_scope_do_fence_and_private_step_protocol() {
    let f = fixture();
    let (definition, binding) = ready(&f);
    let config = WorkflowsConfig::default();
    let service =
        WorkflowBindingService::new(f.storage.clone(), f.scheduler.clone(), config.clone())
            .unwrap()
            .with_metrics(f.metrics.clone());
    let headers = caller(&binding);
    let path = format!(
        "/internal/bindings/v1/workflow/{}",
        binding.descriptor.binding_id
    );
    let created = service
        .execute(
            &format!("{path}/create"),
            &headers,
            json!({"id":"one","payloadJson":"{\"secret\":1}"}),
            10,
        )
        .unwrap();
    let instance_id: WorkflowInstanceId = created["instanceId"].as_str().unwrap().parse().unwrap();
    assert_eq!(created["id"], "one");
    assert_eq!(
        service
            .execute(&format!("{path}/get"), &headers, json!({"id":"one"}), 11)
            .unwrap(),
        json!({"id":"one","instanceId":instance_id})
    );
    assert_eq!(
        service
            .execute(
                &format!("{path}/status"),
                &headers,
                json!({"instanceId":instance_id}),
                11,
            )
            .unwrap(),
        json!({"status":"queued"})
    );
    assert_eq!(
        service
            .execute(
                &format!("{path}/create"),
                &headers,
                json!({"id":"one","payloadJson":"null"}),
                11,
            )
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowInstanceAlreadyExists
    );
    let mut do_headers = headers.clone();
    do_headers.insert(
        "x-open-compute-workflow-do-context",
        HeaderValue::from_static("1"),
    );
    assert_eq!(
        service
            .execute(
                &format!("{path}/create"),
                &do_headers,
                json!({"id":"do","payloadJson":"null"}),
                11,
            )
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowDoOutputGateUnsupported
    );
    assert_eq!(
        service
            .execute(
                &format!("{path}/status"),
                &do_headers,
                json!({"instanceId":instance_id}),
                11,
            )
            .unwrap()["status"],
        "queued"
    );
    assert_eq!(
        service
            .execute(
                &format!("{path}/create"),
                &headers,
                json!({"id":"forged","payloadJson":"null","definitionId":definition}),
                11,
            )
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowSerializationUnsupported
    );
    assert_eq!(
        service
            .execute(
                &format!("{path}/restart"),
                &headers,
                json!({"instanceId":instance_id}),
                11,
            )
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowSerializationUnsupported
    );
    let mut stale = headers.clone();
    stale.insert(
        "x-open-compute-descriptor-sha256",
        HeaderValue::from_static("bad"),
    );
    assert_eq!(
        service
            .execute(&format!("{path}/get"), &stale, json!({"id":"one"}), 11)
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowBindingStale
    );

    let controller = WorkflowController::new(&f.storage, &f.scheduler, &config);
    let run = controller
        .claim(12, &mut Default::default())
        .unwrap()
        .unwrap();
    let declaration = |ordinal, name: &str, dependencies: Vec<u32>| {
        json!({
            "ordinal":ordinal,
            "kind":"do",
            "name":name,
            "nameCount":1,
            "config":{},
            "dependencies":dependencies,
            "batchFirstOrdinal":ordinal,
            "batchSize":1
        })
    };
    let first_claim = body(
        &run.fence,
        json!({"steps":[declaration(0,"lookup",vec![])],"remainingMs":config.dispatch_timeout_ms}),
    );
    let grant = service.run("claim-batch", first_claim.clone(), 13).unwrap();
    let first = &grant["steps"][0];
    assert_eq!(first["state"], "run");
    let success = body(
        &run.fence,
        json!({
            "ordinal":0,
            "attempt":first["attempt"],
            "stepToken":first["stepToken"],
            "outputJson":"{\"value\":333333333.33333329}"
        }),
    );
    assert_eq!(
        service.run("success", success.clone(), 14).unwrap()["state"],
        "complete"
    );
    assert_eq!(
        service.run("success", success, 15).unwrap_err().code(),
        ErrorCode::WorkflowStepStale
    );
    assert_eq!(
        service.run("claim-batch", first_claim, 15).unwrap()["steps"][0]["state"],
        "complete"
    );
    assert_eq!(
        service
            .run("result", body(&run.fence, json!({"ordinal":0})), 15)
            .unwrap()["outputJson"],
        "{\"value\":333333333.3333333}"
    );

    let second_claim = body(
        &run.fence,
        json!({"steps":[declaration(1,"fail",vec![0])],"remainingMs":config.dispatch_timeout_ms}),
    );
    let second = service
        .run("claim-batch", second_claim.clone(), 16)
        .unwrap();
    let second = &second["steps"][0];
    assert_eq!(
        service
            .run(
                "failure",
                body(
                    &run.fence,
                    json!({
                        "ordinal":1,
                        "attempt":second["attempt"],
                        "stepToken":second["stepToken"],
                        "error":{"name":"Error","message":"private-stack"}
                    }),
                ),
                17,
            )
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowSerializationUnsupported
    );
    assert_eq!(
        service
            .run(
                "failure",
                body(
                    &run.fence,
                    json!({
                        "ordinal":1,
                        "attempt":second["attempt"],
                        "stepToken":second["stepToken"],
                        "code":"WORKFLOW_SERIALIZATION_UNSUPPORTED"
                    }),
                ),
                17,
            )
            .unwrap()["state"],
        "failed"
    );
    assert_eq!(
        service.run("claim-batch", second_claim, 18).unwrap()["steps"][0]["state"],
        "failed"
    );
    let failed = service
        .run("result", body(&run.fence, json!({"ordinal":1})), 18)
        .unwrap();
    assert_eq!(failed["code"], "WORKFLOW_SERIALIZATION_UNSUPPORTED");
    assert!(!failed.to_string().contains("private-stack"));
    f.scheduler
        .finish_workflow(
            &run.fence,
            &WorkflowCompletion::Complete {
                output_json: "null".into(),
                final_ordinal: 2,
            },
            19,
            &config,
        )
        .unwrap();
    assert_eq!(
        f.scheduler
            .workflow_instance(run.fence.instance_id)
            .unwrap()
            .unwrap()
            .state,
        WorkflowState::Errored
    );
    assert_eq!(
        service
            .run(
                "claim-batch",
                body(
                    &run.fence,
                    json!({"steps":[declaration(2,"late",vec![1])],"remainingMs":config.dispatch_timeout_ms}),
                ),
                20,
            )
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowRunStale
    );
    let rendered = f
        .metrics
        .render(&crate::health::HealthCoordinator::new().snapshot());
    assert!(rendered.contains("open_compute_workflow_replay_steps_total{outcome=\"complete\"} 1"));
    assert!(rendered.contains("open_compute_workflow_replay_steps_total{outcome=\"failed\"} 1"));
    assert!(rendered.contains("open_compute_workflow_steps_total{outcome=\"success\"} 2"));
    assert!(rendered.contains("open_compute_workflow_steps_total{outcome=\"error\"} 1"));
    assert!(!rendered.contains("private-stack"));
}

#[tokio::test]
async fn workflow_private_http_is_bounded_and_rechecks_startup_generation() {
    let f = fixture();
    let service = WorkflowBindingService::new(
        f.storage.clone(),
        f.scheduler.clone(),
        WorkflowsConfig {
            max_in_flight_requests: 1,
            ..Default::default()
        },
    )
    .unwrap();
    let auth = GenerationAuthRegistry::new();
    auth.activate_for_test(SecretString::new("ab".repeat(32)));
    let request = |content: &str, body: axum::body::Body| {
        Request::builder()
            .method("POST")
            .uri("/internal/workflows/runs/claim-batch")
            .header("content-type", content)
            .header("x-open-compute-binding-token", "ab".repeat(32))
            .header("x-open-compute-startup-generation", "generation-one")
            .body(body)
            .unwrap()
    };
    let response = service
        .handle(
            request("text/plain", axum::body::Body::empty()),
            auth.clone(),
        )
        .await;
    assert_eq!(
        response.headers()["x-open-compute-error-code"],
        "WORKFLOW_METHOD_UNSUPPORTED"
    );
    let response = service
        .handle(
            request("application/json", axum::body::Body::from("not json")),
            auth.clone(),
        )
        .await;
    assert_eq!(
        response.headers()["x-open-compute-error-code"],
        "WORKFLOW_SERIALIZATION_UNSUPPORTED"
    );
    let response = service
        .handle(
            request(
                "application/json",
                axum::body::Body::from("x".repeat(MAX_BODY + 1)),
            ),
            auth.clone(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let permit = service.concurrency.clone().acquire_owned().await.unwrap();
    let response = service
        .handle(
            request("application/json", axum::body::Body::empty()),
            auth.clone(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    drop(permit);
    auth.activate_for_test(SecretString::new("cd".repeat(32)));
    let response = service
        .handle(
            request("application/json", axum::body::Body::from("{}")),
            auth.clone(),
        )
        .await;
    assert_eq!(
        response.headers()["x-open-compute-error-code"],
        "WORKFLOW_RUN_STALE"
    );
    assert!(
        to_bytes(response.into_body(), 100)
            .await
            .unwrap()
            .is_empty()
    );
    for (code, status) in [
        (ErrorCode::WorkflowRuntimeUnavailable, 503),
        (ErrorCode::WorkflowStateQuotaExceeded, 429),
        (ErrorCode::WorkflowEventQueueFull, 429),
        (ErrorCode::WorkflowInstanceNotFound, 404),
        (ErrorCode::WorkflowRunStale, 409),
        (ErrorCode::WorkflowInstanceBusy, 409),
        (ErrorCode::WorkflowInstanceStateConflict, 409),
        (ErrorCode::WorkflowInstanceCleanupPending, 409),
        (ErrorCode::WorkflowResultTooLarge, 413),
        (ErrorCode::WorkflowSerializationUnsupported, 422),
    ] {
        assert_eq!(response_error(code).status().as_u16(), status);
    }
}

#[test]
fn workflow_metric_guards_count_all_outcomes_without_sensitive_labels() {
    let f = fixture();
    for outcome in [
        WorkflowOutcome::Success,
        WorkflowOutcome::Error,
        WorkflowOutcome::Unknown,
    ] {
        let mut guard = f.metrics.workflow_run();
        guard.finish(outcome);
        f.metrics.workflow_created(outcome);
        f.metrics.workflow_step(outcome, Duration::from_millis(5));
    }
    f.metrics.workflow_reconcile(true);
    f.metrics.workflow_reconcile(false);
    f.metrics.workflow_stale(true);
    f.metrics.workflow_stale(false);
    f.metrics.workflow_summary(
        &open_compute_storage::scheduler::WorkflowInspection {
            queued: 1,
            running: 2,
            complete: 3,
            errored: 4,
            state_bytes: 100,
            expired_runs: 1,
            waiting: 5,
            paused: 6,
            terminated: 7,
            retained: 8,
            buffered_events: 2,
            inbox_bytes: 64,
            consumed_events: 3,
            sleeping_steps: 2,
            event_waits: 1,
            retry_waits: 4,
            retried_steps: 3,
            exhausted_steps: 1,
            step_timeouts: 1,
            event_timeouts: 2,
            gc_receipts: 1,
        },
        0.5,
    );
    f.metrics.workflow_operations(
        &open_compute_storage::WorkflowOperationInspection {
            pending_restarts: 1,
            pending_purges: 2,
            oldest_operation_at_ms: Some(1000),
        },
        2500,
    );
    for failure in [
        None,
        Some(ErrorCode::WorkflowEventQueueFull),
        Some(ErrorCode::WorkflowInstanceBusy),
    ] {
        f.metrics.workflow_event(failure);
    }
    for operation in ["pause", "resume", "terminate", "restart", "private-label"] {
        f.metrics.workflow_lifecycle(operation, true);
        f.metrics.workflow_lifecycle(operation, false);
    }
    let output = f
        .metrics
        .render(&crate::health::HealthCoordinator::new().snapshot());
    assert!(output.contains("open_compute_workflow_in_flight 0"));
    assert!(output.contains("open_compute_workflow_runs_total{outcome=\"unknown\"} 1"));
    assert!(output.contains("open_compute_workflow_instance_status{status=\"complete\"} 3"));
    for line in [
        "open_compute_workflow_instance_status{status=\"paused\"} 6",
        "open_compute_workflow_instance_status{status=\"running\"} 2",
        "open_compute_workflow_waiting_steps{reason=\"retry\"} 4",
        "open_compute_workflow_pending_operations{phase=\"purge_receipt\"} 1",
        "open_compute_workflow_event_intake_total{outcome=\"full\"} 1",
        "open_compute_workflow_lifecycle_total{operation=\"restart\",outcome=\"error\"} 1",
        "open_compute_workflow_operation_age_seconds 1.5",
        "open_compute_workflow_consumed_events 3",
    ] {
        assert!(output.contains(line), "missing {line}");
    }
    assert!(!output.contains("private-label"));
}
