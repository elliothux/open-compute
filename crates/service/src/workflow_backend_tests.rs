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
    let workers = WorkerRepository::new(f.storage.db());
    let (worker, _) = workers
        .create_worker(f.account, "caller", RequestId::generate(), 0)
        .unwrap();
    let deployment = DeploymentId::generate();
    let binding = repository
        .prepare_binding(f.account, deployment, "FLOW", definition.id, 3)
        .unwrap();
    workers
        .insert_staging_deployment_with_products_and_limit(
            &NewDeployment {
                id: deployment,
                account_id: f.account,
                worker_id: worker.id,
                artifact_sha256: [3; 32],
                artifact_size: 100,
                artifact_schema_version: 1,
                main_module: "index.js".into(),
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
    (definition.id, binding)
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
    assert_eq!(
        service
            .execute(
                &format!("{path}/create"),
                &headers,
                json!({"id":"one","payloadJson":"{\"secret\":1}"}),
                10
            )
            .unwrap(),
        json!({"id":"one"})
    );
    assert_eq!(
        service
            .execute(&format!("{path}/get"), &headers, json!({"id":"one"}), 11)
            .unwrap(),
        json!({"id":"one"})
    );
    assert_eq!(
        service
            .execute(&format!("{path}/status"), &headers, json!({"id":"one"}), 11)
            .unwrap(),
        json!({"status":"queued"})
    );
    assert_eq!(
        service
            .execute(
                &format!("{path}/create"),
                &headers,
                json!({"id":"one","payloadJson":"null"}),
                11
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
                11
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
                json!({"id":"one"}),
                11
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
                11
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
                json!({"id":"one"}),
                11
            )
            .unwrap_err()
            .code(),
        ErrorCode::WorkflowMethodUnsupported
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
    let run = controller.claim(12).unwrap().unwrap();
    let step = body(
        &run.fence,
        json!({"ordinal":0,"name":"lookup","nameCount":1,"configJson":"null"}),
    );
    let grant = service.run("claim", step.clone(), 13).unwrap();
    let token = grant["stepToken"].clone();
    let success = body(
        &run.fence,
        json!({"ordinal":0,"stepToken":token,"outputJson":"{\"value\":333333333.33333329}"}),
    );
    assert_eq!(
        service.run("success", success.clone(), 14).unwrap(),
        json!({"ok":true})
    );
    assert_eq!(
        service.run("success", success, 15).unwrap_err().code(),
        ErrorCode::WorkflowStepStale
    );
    assert_eq!(
        service.run("claim", step, 15).unwrap()["outputJson"],
        "{\"value\":333333333.3333333}"
    );
    let second = service
        .run(
            "claim",
            body(
                &run.fence,
                json!({"ordinal":1,"name":"fail","nameCount":1,"configJson":"null"}),
            ),
            16,
        )
        .unwrap();
    assert_eq!(service.run("failure",body(&run.fence,json!({"ordinal":1,"stepToken":second["stepToken"],"error":{"name":"Error","message":"private-stack"}})),17).unwrap_err().code(),ErrorCode::WorkflowSerializationUnsupported);
    service.run("failure",body(&run.fence,json!({"ordinal":1,"stepToken":second["stepToken"],"error":WorkflowFailure::default(),"errorCode":"WORKFLOW_SERIALIZATION_UNSUPPORTED"})),17).unwrap();
    let failed = service
        .run(
            "claim",
            body(
                &run.fence,
                json!({"ordinal":1,"name":"fail","nameCount":1,"configJson":"null"}),
            ),
            18,
        )
        .unwrap();
    assert_eq!(failed["errorCode"], "WORKFLOW_SERIALIZATION_UNSUPPORTED");
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
                "claim",
                body(
                    &run.fence,
                    json!({"ordinal":2,"name":"late","nameCount":1,"configJson":"null"})
                ),
                20
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
            .uri("/internal/workflows/v1/runs/claim")
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
        (ErrorCode::WorkflowInstanceNotFound, 404),
        (ErrorCode::WorkflowRunStale, 409),
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
        },
        0.5,
    );
    let output = f
        .metrics
        .render(&crate::health::HealthCoordinator::new().snapshot());
    assert!(output.contains("open_compute_workflow_in_flight 0"));
    assert!(output.contains("open_compute_workflow_runs_total{outcome=\"unknown\"} 1"));
    assert!(output.contains("open_compute_workflow_instance_status{status=\"complete\"} 3"));
}
