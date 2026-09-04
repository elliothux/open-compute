use super::*;
use crate::cloudflare_v4::accounts::AccountAuthority;
use crate::health::HealthCoordinator;
use crate::metrics::MetricsRegistry;
use crate::runtime_bridge::WorkerdTransport;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use open_compute_core::config::MetricsConfig;
use open_compute_core::{
    PlatformId, RequestId, SecretString, StorageConfig, SystemClock, VersionId,
};
use open_compute_runtime::GenerationAuthRegistry;
use open_compute_storage::{
    NewVersion, NewVersionProducts, PlatformStorage, SchedulerStore, VersionContentKind,
    WorkerRepository, WorkflowRepository,
};
use open_compute_workers::{WorkflowController, WorkflowCreateInput};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use tower::ServiceExt as _;

struct Fixture {
    _temp: tempfile::TempDir,
    app: Router,
    public_account: String,
    account: AccountId,
    storage: Arc<PlatformStorage>,
    scheduler: Arc<SchedulerStore>,
    workflow_id: WorkflowId,
    workflow_version: open_compute_core::WorkflowVersionId,
}

fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data");
    let storage = Arc::new(
        PlatformStorage::bootstrap(
            &StorageConfig {
                data_dir: root.clone(),
                master_key_file: root.join("keys/master.key"),
                master_key_env: None,
                sqlite_busy_timeout_ms: 5000,
                free_space_soft_bytes: 1_073_741_824,
                free_space_hard_bytes: 268_435_456,
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
        .create_worker(account, "workflow-api", RequestId::generate(), 0, 1_000_000)
        .unwrap();
    let worker_version = VersionId::generate();
    workers
        .insert_staging_version(
            &NewVersion {
                id: worker_version,
                account_id: account,
                worker_id: worker.id,
                content_kind: VersionContentKind::Worker,
                artifact_sha256: Some([1; 32]),
                artifact_size: Some(100),
                artifact_schema_version: Some(1),
                main_module: Some("index.js".into()),
                worker_code_sha256: [2; 32],
                compatibility_date: "2026-08-30".into(),
                compatibility_flags: Vec::new(),
                vars: Default::default(),
                secrets: Default::default(),
                request_id: RequestId::generate(),
                now_ms: 1,
            },
            &NewVersionProducts::default(),
            1_000_000,
        )
        .unwrap();
    workers.begin_validation(worker_version).unwrap();
    workers.mark_ready(worker_version, 2).unwrap();
    workers
        .promote(
            account,
            worker.id,
            worker_version,
            None,
            RequestId::generate(),
            3,
        )
        .unwrap();
    let workflows = WorkflowRepository::new(storage.db());
    let definition = workflows.create_definition(account, "orders", 4).unwrap();
    let version = workflows
        .stage_version(account, definition.id, worker_version, "Flow", 5)
        .unwrap();
    let version = workflows
        .finish_version(account, version.target.workflow_version_id, true, 6)
        .unwrap();
    let authority = AccountAuthority::new(PlatformId::generate(), account, 1_000);
    let public_account = authority.public_id().to_owned();
    let metrics =
        Arc::new(MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd").unwrap());
    let state = HttpState::for_test(
        HealthCoordinator::new(),
        metrics,
        false,
        Some(SecretString::new("admin-token")),
    )
    .with_v4_tokens(
        SecretString::new("deployer-token"),
        SecretString::new("read-token"),
    )
    .with_cloudflare_v4_account(authority)
    .with_workflow_api(Some(WorkflowApiState::new(
        storage.clone(),
        scheduler.clone(),
        WorkerdTransport::new(GenerationAuthRegistry::new(), Arc::new(Mutex::new(None))),
        Default::default(),
    )));
    let app = crate::cloudflare_v4::router(state.clone(), router()).with_state(state);
    Fixture {
        _temp: temp,
        app,
        public_account,
        account,
        storage,
        scheduler,
        workflow_id: definition.id,
        workflow_version: version.target.workflow_version_id,
    }
}

#[tokio::test]
async fn official_reads_hide_reservations_and_put_can_create_the_definition() {
    let f = fixture();
    let repository = WorkflowRepository::new(f.storage.db());
    repository
        .reserve_definition(f.account, "upload-only", "Flow", "interrupted-upload", 7)
        .unwrap();
    let hidden = f
        .request("GET", &f.path("/upload-only"), Some("read-token"), None)
        .await;
    assert_eq!(hidden.0, StatusCode::NOT_FOUND);
    let list = f
        .request("GET", &f.path(""), Some("read-token"), None)
        .await;
    assert_eq!(list.1["result"].as_array().unwrap().len(), 1);

    let create = f
        .request(
            "PUT",
            &f.path("/created-by-put"),
            Some("deployer-token"),
            Some(serde_json::json!({
                "script_name":"workflow-api", "class_name":"Flow"
            })),
        )
        .await;
    // The fixture has no live workerd supervisor, so the frozen version remains validating.
    // Reaching 503 proves official PUT took the create path instead of the 10200 lookup path.
    assert_eq!(create.0, StatusCode::SERVICE_UNAVAILABLE);
    let created = repository
        .definitions(
            f.account,
            Some("created-by-put"),
            None,
            CatalogSort::Name,
            CatalogDirection::Asc,
            None,
            10,
        )
        .unwrap()
        .items;
    assert_eq!(created.len(), 1);
    assert_eq!(created[0].state, ResourceState::Creating);
}

#[tokio::test]
async fn official_delete_reports_conflict_while_an_upload_reservation_is_pending() {
    let f = fixture();
    let repository = WorkflowRepository::new(f.storage.db());
    let pending = repository
        .reserve_definition(f.account, "orders", "Replacement", "pending-upload", 7)
        .unwrap();
    let conflict = f
        .request("DELETE", &f.path("/orders"), Some("deployer-token"), None)
        .await;
    assert_eq!(conflict.0, StatusCode::CONFLICT);
    repository
        .release_definition_reservation(f.account, &pending, 8)
        .unwrap();
    let deleted = f
        .request("DELETE", &f.path("/orders"), Some("deployer-token"), None)
        .await;
    assert_eq!(deleted.0, StatusCode::OK);
}

#[tokio::test]
async fn pending_upload_delete_conflict_does_not_mutate_a_live_instance() {
    let f = fixture();
    let config = open_compute_core::WorkflowsConfig::default();
    let identity = WorkflowController::new(&f.storage, &f.scheduler, &config)
        .create(
            f.account,
            f.workflow_id,
            open_compute_core::WorkflowOperationId::generate(),
            Some("delete-conflict-live"),
            WorkflowCreateInput {
                payload_base64: "T0NEVgECAA==",
                retention: None,
                schedule: None,
            },
            7,
        )
        .unwrap();
    let before = f
        .scheduler
        .workflow_instance(identity.instance_id)
        .unwrap()
        .unwrap();
    let repository = WorkflowRepository::new(f.storage.db());
    repository
        .reserve_definition(f.account, "orders", "Replacement", "pending-upload", 8)
        .unwrap();

    let conflict = f
        .request("DELETE", &f.path("/orders"), Some("deployer-token"), None)
        .await;
    assert_eq!(conflict.0, StatusCode::CONFLICT);
    let after = f
        .scheduler
        .workflow_instance(identity.instance_id)
        .unwrap()
        .unwrap();
    assert_eq!(after.state, before.state);
    assert_eq!(after.updated_at_ms, before.updated_at_ms);
    assert_eq!(
        repository
            .reservation(identity.instance_id)
            .unwrap()
            .unwrap()
            .state,
        open_compute_storage::WorkflowRefState::Live
    );
    assert_eq!(
        repository
            .definition(f.account, f.workflow_id)
            .unwrap()
            .state,
        ResourceState::Ready
    );
}

#[tokio::test]
async fn delete_intent_purges_live_instances_before_finalizing_the_definition() {
    let f = fixture();
    let config = open_compute_core::WorkflowsConfig::default();
    let identity = WorkflowController::new(&f.storage, &f.scheduler, &config)
        .create(
            f.account,
            f.workflow_id,
            open_compute_core::WorkflowOperationId::generate(),
            Some("delete-intent-live"),
            WorkflowCreateInput {
                payload_base64: "T0NEVgECAA==",
                retention: None,
                schedule: None,
            },
            7,
        )
        .unwrap();

    let deleted = f
        .request("DELETE", &f.path("/orders"), Some("deployer-token"), None)
        .await;
    assert_eq!(deleted.0, StatusCode::OK);
    assert!(
        f.scheduler
            .workflow_instance(identity.instance_id)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        WorkflowRepository::new(f.storage.db())
            .definition(f.account, f.workflow_id)
            .unwrap()
            .state,
        ResourceState::Tombstoned
    );
}

impl Fixture {
    fn path(&self, suffix: &str) -> String {
        format!("/accounts/{}/workflows{suffix}", self.public_account)
    }

    async fn request(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder().method(method).uri(path);
        if let Some(token) = token {
            request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        let body = if let Some(body) = body {
            request = request.header(header::CONTENT_TYPE, "application/json");
            Body::from(body.to_string())
        } else {
            Body::empty()
        };
        let response = self
            .app
            .clone()
            .oneshot(request.body(body).unwrap())
            .await
            .unwrap();
        assert!(
            response
                .headers()
                .contains_key(crate::http::REQUEST_ID_HEADER)
        );
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }
}

#[tokio::test]
async fn definitions_versions_and_strict_permissions_use_v4_contracts() {
    let f = fixture();
    assert_eq!(
        f.request("GET", &f.path(""), None, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    let list = f
        .request(
            "GET",
            &f.path("?page=1&per_page=10&search=orders"),
            Some("read-token"),
            None,
        )
        .await;
    assert_eq!(list.0, StatusCode::OK);
    assert_eq!(list.1["result"][0]["id"], f.workflow_id.to_string());
    assert_eq!(list.1["result_info"]["total_count"], 1);
    let detail = f
        .request("GET", &f.path("/orders"), Some("read-token"), None)
        .await;
    assert_eq!(detail.1["result"]["script_name"], "workflow-api");
    let missing = f
        .request("GET", &f.path("/missing"), Some("read-token"), None)
        .await;
    assert_eq!(missing.0, StatusCode::NOT_FOUND);
    assert_eq!(missing.1["errors"][0]["code"], 10_200);
    let versions = f
        .request("GET", &f.path("/orders/versions"), Some("read-token"), None)
        .await;
    assert_eq!(
        versions.1["result"][0]["id"],
        f.workflow_version.to_string()
    );
    let version = f
        .request(
            "GET",
            &f.path(&format!("/orders/versions/{}", f.workflow_version)),
            Some("read-token"),
            None,
        )
        .await;
    assert_eq!(
        version.1["result"]["workflow_id"],
        f.workflow_id.to_string()
    );
    let update = f
        .request(
            "PUT",
            &f.path("/orders"),
            Some("deployer-token"),
            Some(serde_json::json!({
                "script_name":"workflow-api", "class_name":"Flow"
            })),
        )
        .await;
    assert_eq!(update.0, StatusCode::OK);
    let unsupported = f
        .request(
            "PUT",
            &f.path("/orders"),
            Some("deployer-token"),
            Some(serde_json::json!({
                "script_name":"workflow-api", "class_name":"Flow", "concurrency":{"limit":2}
            })),
        )
        .await;
    assert_eq!(unsupported.0, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(
        f.request("GET", &f.path("?page=1&page=2"), Some("read-token"), None)
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn instance_create_batch_list_detail_status_and_event_share_authority() {
    let f = fixture();
    let created = f
        .request(
            "POST",
            &f.path("/orders/instances"),
            Some("deployer-token"),
            Some(serde_json::json!({
                "instance_id":"official-one", "params":{"value":7}
            })),
        )
        .await;
    assert_eq!(created.0, StatusCode::OK);
    assert_eq!(created.1["result"]["id"], "official-one");
    let batch = f
        .request(
            "POST",
            &f.path("/orders/instances/batch"),
            Some("deployer-token"),
            Some(serde_json::json!([
                {"instance_id":"batch-one","params":"{\"batch\":1}"},
                {"instance_id":"batch-two","params":{"batch":2}}
            ])),
        )
        .await;
    assert_eq!(batch.1["result"].as_array().unwrap().len(), 2);
    let list = f
        .request(
            "GET",
            &f.path("/orders/instances?per_page=2&direction=asc"),
            Some("read-token"),
            None,
        )
        .await;
    assert_eq!(list.1["result"].as_array().unwrap().len(), 2);
    assert!(list.1["result_info"]["cursor"].is_string());
    let detail = f
        .request(
            "GET",
            &f.path("/orders/instances/official-one?simple=false&order=asc"),
            Some("read-token"),
            None,
        )
        .await;
    assert_eq!(detail.1["result"]["params"], serde_json::json!({"value":7}));
    let paused = f
        .request(
            "PATCH",
            &f.path("/orders/instances/official-one/status"),
            Some("deployer-token"),
            Some(serde_json::json!({"status":"pause"})),
        )
        .await;
    assert_eq!(paused.1["result"]["status"], "paused");
    let event = f
        .request(
            "POST",
            &f.path("/orders/instances/official-one/events/approval"),
            Some("deployer-token"),
            Some(serde_json::json!({"approved":true})),
        )
        .await;
    assert_eq!(event.0, StatusCode::OK);
    assert_eq!(event.1["result"]["instanceId"], "official-one");
    let resumed = f
        .request(
            "PATCH",
            &f.path("/orders/instances/official-one/status"),
            Some("deployer-token"),
            Some(serde_json::json!({"status":"resume"})),
        )
        .await;
    assert_eq!(resumed.0, StatusCode::OK);
    let simple = f
        .request(
            "GET",
            &f.path("/orders/instances/official-one?simple=true&order=desc"),
            Some("read-token"),
            None,
        )
        .await;
    assert_eq!(simple.0, StatusCode::OK);
    assert_eq!(simple.1["result"]["steps"], serde_json::json!([]));

    for id in ["rollback-one", "restart-one", "terminate-one"] {
        assert_eq!(
            f.request(
                "POST",
                &f.path("/orders/instances"),
                Some("deployer-token"),
                Some(serde_json::json!({"instance_id":id})),
            )
            .await
            .0,
            StatusCode::OK
        );
    }
    assert_eq!(
        f.request(
            "PATCH",
            &f.path("/orders/instances/rollback-one/status"),
            Some("deployer-token"),
            Some(serde_json::json!({"status":"terminate","rollback":true})),
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        f.request(
            "PATCH",
            &f.path("/orders/instances/restart-one/status"),
            Some("deployer-token"),
            Some(serde_json::json!({"status":"restart"})),
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        f.request(
            "PATCH",
            &f.path("/orders/instances/terminate-one/status"),
            Some("deployer-token"),
            Some(serde_json::json!({"status":"terminate"})),
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        f.request(
            "GET",
            &f.path("/orders/instances?status=terminated&date_start=1970-01-01T00%3A00%3A00Z&date_end=2100-01-01T00%3A00%3A00Z"),
            Some("read-token"),
            None,
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        f.request(
            "PATCH",
            &f.path("/orders/instances/official-one/status"),
            Some("read-token"),
            Some(serde_json::json!({"status":"resume"}))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn content_type_query_and_delete_boundaries_are_strict() {
    let f = fixture();
    assert_eq!(
        f.request(
            "GET",
            &f.path("/orders/instances?unknown=1"),
            Some("read-token"),
            None
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let response = f
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(f.path("/orders/instances"))
                .header(header::AUTHORIZATION, "Bearer deployer-token")
                .header(header::CONTENT_TYPE, "text/plain")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let deleted = f
        .request("DELETE", &f.path("/orders"), Some("deployer-token"), None)
        .await;
    assert_eq!(deleted.0, StatusCode::OK);
    assert_eq!(deleted.1["result"]["status"], "ok");
}

#[tokio::test]
async fn instance_routes_reject_unsupported_and_malformed_variants_before_mutation() {
    let f = fixture();
    let instances = f.path("/orders/instances");
    for (path, body, expected) in [
        (
            format!("{instances}?unexpected=true"),
            serde_json::json!({"instance_id":"query"}),
            StatusCode::BAD_REQUEST,
        ),
        (
            instances.clone(),
            serde_json::json!({"instance_id":"located","location_hint":"wnam"}),
            StatusCode::NOT_IMPLEMENTED,
        ),
        (
            instances.clone(),
            serde_json::json!({"instance_id":format!("cf_{}", "a".repeat(64))}),
            StatusCode::BAD_REQUEST,
        ),
        (
            instances.clone(),
            serde_json::json!({
                "instance_id":"retention",
                "instance_retention":{"success_retention":"not-a-duration"}
            }),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        assert_eq!(
            f.request("POST", &path, Some("deployer-token"), Some(body))
                .await
                .0,
            expected
        );
    }
    assert_eq!(
        f.request(
            "POST",
            &format!("{instances}/batch"),
            Some("deployer-token"),
            Some(serde_json::json!([])),
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        f.request(
            "POST",
            &format!("{instances}/batch"),
            Some("deployer-token"),
            Some(serde_json::json!([{"location_hint":"wnam"}])),
        )
        .await
        .0,
        StatusCode::NOT_IMPLEMENTED
    );
    assert_eq!(
        f.request(
            "POST",
            &instances,
            None,
            Some(serde_json::json!({"instance_id":"unauthorized"})),
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );

    for id in ["page-one", "page-two", "page-three"] {
        assert_eq!(
            f.request(
                "POST",
                &instances,
                Some("deployer-token"),
                Some(serde_json::json!({"instance_id":id})),
            )
            .await
            .0,
            StatusCode::OK
        );
    }
    let first = f
        .request(
            "GET",
            &format!("{instances}?per_page=1&direction=desc"),
            Some("read-token"),
            None,
        )
        .await;
    assert_eq!(first.0, StatusCode::OK);
    let cursor = first.1["result_info"]["cursor"].as_str().unwrap();
    assert_eq!(
        f.request(
            "GET",
            &format!("{instances}?per_page=1&direction=desc&cursor={cursor}"),
            Some("read-token"),
            None,
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        f.request(
            "GET",
            &format!("{instances}?page=2&per_page=1&direction=asc"),
            Some("read-token"),
            None,
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        f.request(
            "GET",
            &format!("{instances}/page-one?simple=invalid"),
            Some("read-token"),
            None,
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        f.request(
            "PATCH",
            &format!("{instances}/page-one/status?unexpected=true"),
            Some("deployer-token"),
            Some(serde_json::json!({"status":"pause"})),
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        f.request(
            "PATCH",
            &format!("{instances}/page-one/status"),
            Some("deployer-token"),
            Some(serde_json::json!({"status":"invalid"})),
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        f.request(
            "POST",
            &format!("{instances}/page-one/events/bad%20event"),
            Some("deployer-token"),
            Some(serde_json::json!({})),
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        f.request(
            "POST",
            &format!("{instances}/page-one/events/event"),
            Some("deployer-token"),
            Some(serde_json::json!(true)),
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}
