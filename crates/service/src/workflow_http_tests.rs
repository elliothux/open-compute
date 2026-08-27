use super::*;
use crate::health::HealthCoordinator;
use crate::metrics::MetricsRegistry;
use open_compute_core::{MetricsConfig, SecretString, StorageConfig, SystemClock};
use open_compute_runtime::GenerationAuthRegistry;
use open_compute_storage::{NewDeployment, WorkerRepository};
use std::sync::Mutex;
use tower::ServiceExt as _;

pub(crate) struct Fixture {
    pub(crate) _temp: tempfile::TempDir,
    pub(crate) storage: Arc<PlatformStorage>,
    pub(crate) scheduler: Arc<SchedulerStore>,
    pub(crate) account: AccountId,
    pub(crate) deployment: DeploymentId,
    pub(crate) metrics: Arc<MetricsRegistry>,
    router: Router,
}

pub(crate) fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data");
    let storage = Arc::new(
        PlatformStorage::bootstrap(
            &StorageConfig {
                data_dir: root.clone(),
                master_key_file: root.join("keys/master.key"),
                master_key_env: None,
                sqlite_busy_timeout_ms: 5000,
                free_space_soft_bytes: 1073741824,
                free_space_hard_bytes: 268435456,
            },
            &SystemClock,
        )
        .unwrap(),
    );
    let scheduler = Arc::new(
        SchedulerStore::open(&storage.data_dir().ensure_scheduler_db().unwrap(), 5000, 0).unwrap(),
    );
    let account = storage.identity().default_account_id;
    let workers = WorkerRepository::new(storage.db());
    let (worker, _) = workers
        .create_worker(account, "workflow-api", RequestId::generate(), 0)
        .unwrap();
    let deployment = DeploymentId::generate();
    workers
        .insert_staging_deployment(&NewDeployment {
            id: deployment,
            account_id: account,
            worker_id: worker.id,
            artifact_sha256: [1; 32],
            artifact_size: 100,
            artifact_schema_version: 1,
            main_module: "index.js".into(),
            compatibility_date: "2026-08-26".into(),
            compatibility_flags: Vec::new(),
            limits: serde_json::json!({"profile":"default"}),
            worker_code_sha256: [2; 32],
            vars: Default::default(),
            secrets: Default::default(),
            request_id: RequestId::generate(),
            now_ms: 0,
        })
        .unwrap();
    workers.begin_validation(deployment).unwrap();
    workers.mark_ready(deployment, 1).unwrap();
    let transport =
        WorkerdTransport::new(GenerationAuthRegistry::new(), Arc::new(Mutex::new(None)));
    let api = WorkflowApiState::new(storage.clone(), scheduler.clone(), transport.clone());
    let metrics =
        Arc::new(MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd").unwrap());
    let service = Arc::new(
        crate::scheduler::SchedulerService::new(
            scheduler.clone(),
            storage.clone(),
            transport,
            Default::default(),
            Default::default(),
            Arc::new(SystemSchedulerClock),
        )
        .with_metrics(metrics.clone()),
    );
    let state = HttpState::for_test(
        HealthCoordinator::new(),
        metrics.clone(),
        true,
        Some(SecretString::new("workflow-test-admin")),
    )
    .with_workflow_api(Some(api.clone()))
    .with_scheduler(Some(service));
    Fixture {
        _temp: temp,
        storage,
        scheduler,
        account,
        deployment,
        metrics,
        router: crate::http::admin_router(state),
    }
}

impl Fixture {
    async fn request(
        &self,
        method: &str,
        path: &str,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let response = self
            .router
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method(method)
                    .uri(path)
                    .header("authorization", "Bearer workflow-test-admin")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        )
    }
    fn collection(&self) -> String {
        format!("/v1/accounts/{}/workflows", self.account)
    }
}

#[tokio::test]
async fn workflow_control_catalog_validation_recovery_and_inspection() {
    let f = fixture();
    let collection = f.collection();
    let (status, created) = f
        .request("POST", &collection, serde_json::json!({"name":"orders"}))
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let definition: WorkflowId = created["id"].as_str().unwrap().parse().unwrap();
    let detail = format!("{collection}/{definition}");
    assert_eq!(
        f.request("POST", &collection, serde_json::json!({"name":"orders"}))
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        f.request("GET", &collection, serde_json::Value::Null)
            .await
            .1
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        f.request("PATCH", &detail, serde_json::json!({"name":"renamed"}))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        f.request("GET", &detail, serde_json::Value::Null).await.1["definition"]["name"],
        "renamed"
    );
    let versions = format!("{detail}/versions");
    let (status, version) = f
        .request(
            "POST",
            &versions,
            serde_json::json!({"deploymentId":f.deployment,"className":"Flow"}),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{version}");
    assert_eq!(version["state"], "validating");
    let repo = WorkflowRepository::new(f.storage.db());
    let version_id = version["target"]["versionId"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(repo.pending_versions(None, 10).unwrap().len(), 1);
    assert_eq!(
        f.request(
            "POST",
            "/v1/operator/workflows/reconcile",
            serde_json::Value::Null
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        repo.pending_versions(None, 10).unwrap().len(),
        1,
        "Unknown probe must remain recoverable"
    );
    repo.finish_version(f.account, version_id, true, 10)
        .unwrap();
    assert_eq!(
        f.request(
            "GET",
            &format!("{versions}?after=0&limit=1"),
            serde_json::Value::Null
        )
        .await
        .1
        .as_array()
        .unwrap()
        .len(),
        1
    );
    let config = Default::default();
    let controller =
        open_compute_workers::WorkflowController::new(&f.storage, &f.scheduler, &config);
    controller
        .create(
            f.account,
            definition,
            Some("instance"),
            "{\"private\":\"payload-marker\"}",
            now_ms(),
        )
        .unwrap();
    let instances = format!("{detail}/instances");
    let (status, rows) = f.request("GET", &instances, serde_json::Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{rows}");
    assert_eq!(rows[0]["status"], "queued");
    assert!(!rows.to_string().contains("payload-marker"));
    let instance = rows[0]["id"].as_str().unwrap();
    let steps = format!("{instances}/{instance}/steps");
    assert_eq!(
        f.request("GET", &steps, serde_json::Value::Null).await.1,
        serde_json::json!([])
    );
    assert_eq!(
        f.request("GET", "/v1/operator/workflows", serde_json::Value::Null)
            .await
            .1["queued"],
        1
    );
    assert_eq!(
        f.request("DELETE", &detail, serde_json::Value::Null)
            .await
            .0,
        StatusCode::CONFLICT
    );
    let view = open_compute_storage::scheduler::inspect_workflow_databases(
        &f.storage.data_dir().control_db_path(),
        &f.storage.data_dir().scheduler_db_path(),
        5000,
        32,
    )
    .unwrap();
    assert!(view.is_valid(), "{view:?}");
    assert_eq!(view.inspected_instances, 1);
    assert!(!view.sampled);
    let run = controller.claim(now_ms()).unwrap().unwrap();
    f.scheduler
        .finish_workflow(
            &run.fence,
            &open_compute_storage::scheduler::WorkflowCompletion::Errored {
                code: ErrorCode::WorkflowExecutionFailed,
            },
            now_ms(),
            &config,
        )
        .unwrap();
    controller
        .reconcile(&mut Default::default(), 32, now_ms())
        .unwrap();
    assert_eq!(
        f.request("DELETE", &detail, serde_json::Value::Null)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        f.request("POST", &collection, serde_json::json!({"name":"renamed"}))
            .await
            .0,
        StatusCode::CREATED
    );
}

#[tokio::test]
async fn workflow_operator_rejects_untrusted_scope_pagination_and_bodies() {
    let f = fixture();
    let repo = WorkflowRepository::new(f.storage.db());
    let definition = repo.create_definition(f.account, "operator", 0).unwrap();
    let detail = format!("{}/{}", f.collection(), definition.id);
    for suffix in [
        "?limit=0",
        "?limit=1001",
        "?limit=a",
        "?after=bad",
        "?limit=1&limit=2",
        "?extra=1",
        "?broken",
    ] {
        assert_eq!(
            f.request(
                "GET",
                &format!("{detail}/instances{suffix}"),
                serde_json::Value::Null
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        f.request(
            "POST",
            &f.collection(),
            serde_json::json!({"name":"valid","definitionId":definition.id})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        f.request("GET", "/v1/accounts/bad/workflows", serde_json::Value::Null)
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        f.request(
            "GET",
            &format!(
                "/v1/accounts/{}/workflows/{}",
                AccountId::generate(),
                definition.id
            ),
            serde_json::Value::Null
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        f.request(
            "POST",
            &format!("{detail}/versions"),
            serde_json::json!({"deploymentId":f.deployment,"className":"__private"})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let unauthorized = f
        .router
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .uri("/v1/operator/workflows")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    let state = HttpState::for_test(HealthCoordinator::new(), f.metrics.clone(), false, None);
    let unavailable = crate::http::admin_router(state)
        .oneshot(
            axum::http::Request::builder()
                .uri("/v1/operator/workflows")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unavailable.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        failure(
            &error(ErrorCode::WorkflowStateQuotaExceeded),
            RequestId::generate()
        )
        .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    let poisoned = tokio::spawn(async {
        panic!("operator task failed");
        #[allow(unreachable_code)]
        Ok::<(), PlatformError>(())
    })
    .await;
    assert_eq!(
        response(poisoned, RequestId::generate(), StatusCode::OK).status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    f.storage.begin_draining();
    assert_eq!(
        f.request(
            "POST",
            &f.collection(),
            serde_json::json!({"name":"draining"})
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
}
