use super::*;
use crate::workflow_http::tests::fixture;
use axum::{Json, Router, routing::post};
use open_compute_core::SecretString;
use open_compute_runtime::GenerationAuthRegistry;
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
        .stage_version(f.account, definition.id, f.deployment, "Flow", 1, 1)
        .unwrap();
    repository
        .finish_version(f.account, version.target.version_id, true, 2)
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route(
                "/internal/workflow",
                post(|| async {
                    // A successful transport cannot manufacture missing durable steps.
                    Json(json!({"outcome":"complete","finalOrdinal":1,
                        "outputJson":"null","loaderOutcome":"warm"}))
                }),
            ),
        )
        .with_graceful_shutdown(async {
            let _ = stopped.await;
        })
        .await
        .unwrap();
    });
    let auth = GenerationAuthRegistry::new();
    auth.activate_for_test(SecretString::new("aa".repeat(32)));
    let config = open_compute_core::WorkflowsConfig::default();
    let controller = WorkflowController::new(&f.storage, &f.scheduler, &config);
    controller
        .create(
            f.account,
            definition.id,
            1,
            Some("short-frontier"),
            open_compute_workers::WorkflowCreateInput {
                payload_json: "null",
                retention: None,
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
            WorkerdTransport::for_test_endpoint(auth, port),
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
        !repository
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
        .stage_version(f.account, definition.id, f.deployment, "Flow", 2, 1)
        .unwrap();
    repository
        .finish_version(f.account, version.target.version_id, true, 2)
        .unwrap();
    let config = open_compute_core::WorkflowsConfig::default();
    let controller = WorkflowController::new(&f.storage, &f.scheduler, &config);
    for name in ["active", "pending"] {
        controller
            .create(
                f.account,
                definition.id,
                2,
                Some(name),
                open_compute_workers::WorkflowCreateInput {
                    payload_json: "null",
                    retention: None,
                },
                10,
            )
            .unwrap();
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, Router::new().route("/internal/workflow-v2", post(|| async {
            Json(json!({"result":{"outcome":"unknown","finalOrdinal":0},"loaderOutcome":"warm","drainIncomplete":true}))
        }))).with_graceful_shutdown(async {let _ = stopped.await;}).await.unwrap();
    });
    let auth = GenerationAuthRegistry::new();
    auth.activate_for_test(SecretString::new("11".repeat(32)));
    let service = Arc::new(SchedulerService::new(
        f.scheduler.clone(),
        f.storage.clone(),
        WorkerdTransport::for_test_endpoint(auth.clone(), port),
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
    auth.activate_for_test(SecretString::new("22".repeat(32)));
    assert_eq!(service.claim_workflows(1).await.unwrap().len(), 1);
    stop.send(()).unwrap();
    server.await.unwrap();
}
