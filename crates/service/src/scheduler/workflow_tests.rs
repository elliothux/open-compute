use super::*;
use crate::workflow_http::tests::fixture;
use axum::{Json, Router, routing::post};

use open_compute_storage::scheduler::WorkflowState;
use serde_json::json;

#[tokio::test]
async fn workflow_metrics_follow_durable_replay_verdict_not_transport_success() {
    let f = fixture();
    let repository = WorkflowRepository::new(f.storage.db());
    let definition = repository
        .create_definition(f.account, "verdict", 0)
        .unwrap();
    let version = repository
        .stage_version(f.account, definition.id, f.version, "Flow", 1)
        .unwrap();
    repository
        .finish_version(f.account, version.target.workflow_version_id, true, 2)
        .unwrap();
    let listener_dir = tempfile::tempdir_in("/tmp").unwrap();
    let listener_path = listener_dir.path().join("runtime.sock");
    let listener = tokio::net::UnixListener::bind(&listener_path).unwrap();
    let port = listener_path.clone();
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route(
                "/internal/workflow",
                post(|| async {
                    // A successful transport cannot manufacture missing durable steps.
                    Json(json!({"result":{"outcome":"complete","finalOrdinal":1,
                        "outputBase64":"T0NEVgECAA=="},"loaderOutcome":"warm","drainIncomplete":false}))
                }),
            ),
        )
        .with_graceful_shutdown(async {
            let _ = stopped.await;
        })
        .await
        .unwrap();
    });

    let config = open_compute_core::WorkflowsConfig::default();
    let controller = WorkflowController::new(&f.storage, &f.scheduler, &config);
    controller
        .create(
            f.account,
            definition.id,
            open_compute_core::WorkflowOperationId::generate(),
            Some("short-frontier"),
            open_compute_workers::WorkflowCreateInput {
                payload_base64: "T0NEVgECAA==",
                retention: None,
                schedule: None,
            },
            10,
        )
        .unwrap();
    let run = controller
        .claim(11, &mut Default::default())
        .unwrap()
        .unwrap();
    let id = run.fence.instance_id;
    let service = Arc::new(
        SchedulerService::new(
            f.scheduler.clone(),
            f.storage.clone(),
            WorkerdTransport::for_test_endpoint(&port),
            Default::default(),
            config,
            Arc::new(open_compute_core::DeterministicSchedulerClock::new(12)),
        )
        .with_metrics(f.metrics.clone()),
    );
    service.dispatch_workflow_run(run).await;
    let record = f.scheduler.workflow_instance(id).unwrap().unwrap();
    assert_eq!(record.state, WorkflowState::Errored);
    assert_eq!(
        record.error_code.as_deref(),
        Some("WORKFLOW_NON_DETERMINISTIC")
    );
    assert!(
        repository
            .instance_referrers_intact(&record.identity)
            .unwrap()
    );
    let metrics = f
        .metrics
        .render(&crate::health::HealthCoordinator::new().snapshot());
    assert!(metrics.contains("open_compute_workflow_runs_total{outcome=\"error\"} 1"));
    assert!(metrics.contains("open_compute_workflow_runs_total{outcome=\"success\"} 0"));
    stop.send(()).unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn quarantined_generation_stops_claims_even_after_operator_resume() {
    let f = fixture();
    let repository = WorkflowRepository::new(f.storage.db());
    let definition = repository
        .create_definition(f.account, "admission", 0)
        .unwrap();
    let version = repository
        .stage_version(f.account, definition.id, f.version, "Flow", 1)
        .unwrap();
    repository
        .finish_version(f.account, version.target.workflow_version_id, true, 2)
        .unwrap();
    let config = open_compute_core::WorkflowsConfig::default();
    let controller = WorkflowController::new(&f.storage, &f.scheduler, &config);
    for name in ["active", "pending"] {
        controller
            .create(
                f.account,
                definition.id,
                open_compute_core::WorkflowOperationId::generate(),
                Some(name),
                open_compute_workers::WorkflowCreateInput {
                    payload_base64: "T0NEVgECAA==",
                    retention: None,
                    schedule: None,
                },
                10,
            )
            .unwrap();
    }
    let listener_dir = tempfile::tempdir_in("/tmp").unwrap();
    let listener_path = listener_dir.path().join("runtime.sock");
    let listener = tokio::net::UnixListener::bind(&listener_path).unwrap();
    let port = listener_path.clone();
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, Router::new().route("/internal/workflow", post(|| async {
            Json(json!({"result":{"outcome":"unknown","finalOrdinal":0},"loaderOutcome":"warm","drainIncomplete":true}))
        }))).with_graceful_shutdown(async {let _ = stopped.await;}).await.unwrap();
    });

    let service = Arc::new(SchedulerService::new(
        f.scheduler.clone(),
        f.storage.clone(),
        WorkerdTransport::for_test_endpoint(&port),
        Default::default(),
        config,
        Arc::new(open_compute_core::DeterministicSchedulerClock::new(12)),
    ));
    let run = service.claim_workflows(1).await.unwrap().remove(0);
    service.clone().dispatch_workflow_run(run).await;
    for resume in [false, true] {
        if resume {
            service.resume_kind(SchedulerKind::Workflow).unwrap();
        }
        assert_eq!(
            service.claim_workflows(1).await.err().unwrap().code(),
            ErrorCode::WorkflowRuntimeUnavailable
        );
        assert_eq!(
            service.poll_once().await.unwrap_err().code(),
            ErrorCode::WorkflowRuntimeUnavailable
        );
        assert_eq!(f.scheduler.inspect_workflows(12).unwrap().queued, 1);
    }

    let mut rotated = service.transport.clone();
    rotated.test_generation = Some(open_compute_core::StartupId::generate());
    rotated.ensure_workflow_admission().unwrap();
    let mut service = Arc::try_unwrap(service).ok().unwrap();
    service.transport = rotated;
    assert_eq!(service.claim_workflows(1).await.unwrap().len(), 1);
    stop.send(()).unwrap();
    server.await.unwrap();
}
