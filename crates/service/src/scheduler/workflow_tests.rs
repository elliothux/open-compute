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
        .stage_version(f.account, definition.id, f.deployment, "Flow", 1)
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
        .create(f.account, definition.id, Some("short-frontier"), "null", 10)
        .unwrap();
    let run = controller.claim(11).unwrap().unwrap();
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
