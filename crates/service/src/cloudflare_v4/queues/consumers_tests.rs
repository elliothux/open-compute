use super::*;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use open_compute_core::{
    DeterministicSchedulerClock, PlatformId, RequestId, SchedulerConfig, SecretString, VersionId,
    WorkflowsConfig,
};
use open_compute_runtime::GenerationAuthRegistry;
use open_compute_storage::{
    NewVersion, NewVersionProducts, QueueConfig, QueueProjection, QueueRepository, SchedulerStore,
    VersionContentKind,
};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use tower::ServiceExt as _;

fn seed_active_worker(storage: &PlatformStorage, account: AccountId) {
    let repository = WorkerRepository::new(storage.db());
    let worker = repository
        .create_worker(account, "consumer-worker", RequestId::generate(), 1, 100)
        .unwrap()
        .0;
    let version = VersionId::generate();
    repository
        .insert_staging_version(
            &NewVersion {
                id: version,
                account_id: account,
                worker_id: worker.id,
                content_kind: VersionContentKind::Worker,
                artifact_sha256: Some([1; 32]),
                artifact_size: Some(1),
                artifact_schema_version: Some(1),
                main_module: Some("index.js".to_owned()),
                worker_code_sha256: [2; 32],
                compatibility_date: "2026-08-30".to_owned(),
                compatibility_flags: Vec::new(),
                vars: BTreeMap::new(),
                secrets: BTreeMap::new(),
                request_id: RequestId::generate(),
                now_ms: 2,
            },
            &NewVersionProducts::default(),
            100,
        )
        .unwrap();
    repository.begin_validation(version).unwrap();
    repository.mark_ready(version, 3).unwrap();
    repository
        .promote(account, worker.id, version, None, RequestId::generate(), 4)
        .unwrap();
}

fn seed_queue(
    storage: &PlatformStorage,
    scheduler: &SchedulerStore,
    account: AccountId,
    name: &str,
) -> QueueId {
    let id = QueueId::generate();
    let config = QueueConfig::default();
    QueueRepository::new(storage.db())
        .insert_creating(account, id, name, config, 1)
        .unwrap();
    scheduler
        .create_queue_projection(&QueueProjection {
            queue_id: id,
            account_id: account,
            lifecycle_generation: 1,
            config_generation: 1,
            config,
            created_at_ms: 1,
            updated_at_ms: 1,
        })
        .unwrap();
    QueueRepository::new(storage.db())
        .mark_ready(account, id, 2)
        .unwrap();
    id
}

async fn json(response: Response) -> serde_json::Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap()).unwrap()
}

#[tokio::test]
async fn consumer_routes_cover_create_read_update_delete_and_validation() {
    let (_temp, _mock, state, account, storage) =
        crate::tests::initialized_worker_http_fixture().await;
    seed_active_worker(&storage, account);
    WorkerRepository::new(storage.db())
        .create_worker(account, "inactive-worker", RequestId::generate(), 2, 100)
        .unwrap();
    let scheduler_store = Arc::new(
        SchedulerStore::open(&storage.data_dir().ensure_scheduler_db().unwrap(), 100, 1).unwrap(),
    );
    let queue = seed_queue(&storage, &scheduler_store, account, "source-queue");
    seed_queue(&storage, &scheduler_store, account, "dead-letter");
    let transport = crate::runtime_bridge::WorkerdTransport::new(
        GenerationAuthRegistry::new(),
        Arc::new(Mutex::new(None)),
    );
    let scheduler = Arc::new(crate::SchedulerService::new(
        scheduler_store,
        storage.clone(),
        transport,
        SchedulerConfig::default(),
        WorkflowsConfig::default(),
        Arc::new(DeterministicSchedulerClock::new(10)),
    ));
    let api = crate::QueueApiState::new(storage.clone(), scheduler.clone(), 8)
        .with_metrics(state.metrics().clone());
    let pending_queue = QueueId::generate();
    QueueRepository::new(storage.db())
        .insert_creating(
            account,
            pending_queue,
            "pending-queue",
            QueueConfig::default(),
            3,
        )
        .unwrap();
    assert_eq!(api.reconcile_pending().await.unwrap(), 1);
    assert_eq!(
        QueueRepository::new(storage.db())
            .get(account, pending_queue)
            .unwrap()
            .state,
        open_compute_storage::QueueState::Ready
    );
    let authority =
        super::super::super::accounts::AccountAuthority::new(PlatformId::generate(), account, 1);
    let public_account = authority.public_id().to_owned();
    let public_queue = authority.public_queue_id(queue);
    let app = crate::http::admin_router(
        state
            .with_queue_api(Some(api.clone()))
            .with_scheduler(Some(scheduler))
            .with_platform_storage(storage.clone())
            .with_v4_tokens(
                SecretString::new("deployer-token"),
                SecretString::new("read-token"),
            )
            .with_cloudflare_v4_account(authority),
    );
    let prefix = format!("/client/v4/accounts/{public_account}/queues/{public_queue}/consumers");
    let json_request = |method: Method, path: &str, body: serde_json::Value| {
        Request::builder()
            .method(method)
            .uri(path)
            .header(header::AUTHORIZATION, "Bearer deployer-token")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap()
    };

    let catalog = format!("/client/v4/accounts/{public_account}/queues");
    let listed_queues = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("{catalog}?page=1&name=source-queue"))
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(listed_queues.status(), StatusCode::OK);
    assert_eq!(
        json(listed_queues).await["result"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let fetched_queue = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("{catalog}/{public_queue}"))
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(fetched_queue.status(), StatusCode::OK);

    let updated_queue = app
        .clone()
        .oneshot(json_request(
            Method::PUT,
            &format!("{catalog}/{public_queue}"),
            serde_json::json!({
                "queue_name":"source-renamed",
                "settings":{
                    "delivery_delay":3,
                    "message_retention_period":3600,
                    "delivery_paused":true
                }
            }),
        ))
        .await
        .unwrap();
    assert_eq!(updated_queue.status(), StatusCode::OK);
    assert_eq!(
        json(updated_queue).await["result"]["queue_name"],
        "source-renamed"
    );

    let created_queue = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &catalog,
            serde_json::json!({
                "queue_name":"catalog-created",
                "settings":{"delivery_paused":true,"delivery_delay":2}
            }),
        ))
        .await
        .unwrap();
    assert_eq!(created_queue.status(), StatusCode::OK);
    let created_queue = json(created_queue).await["result"]["queue_id"]
        .as_str()
        .unwrap()
        .to_owned();

    for (method, uri, body) in [
        (
            Method::POST,
            format!("{catalog}?query=true"),
            serde_json::json!({"queue_name":"bad"}),
        ),
        (
            Method::POST,
            catalog.clone(),
            serde_json::json!({"queue_name":""}),
        ),
        (
            Method::PUT,
            format!("{catalog}/{public_queue}?query=true"),
            serde_json::json!({}),
        ),
    ] {
        let response = app
            .clone()
            .oneshot(json_request(method, &uri, body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    let deleted_queue = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri(format!("{catalog}/{created_queue}"))
                .header(header::AUTHORIZATION, "Bearer deployer-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(deleted_queue.status(), StatusCode::OK);

    let status = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/client/v4/open-compute/scheduler")
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(status.status(), StatusCode::OK);
    for operation in ["pause", "resume", "repair"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/client/v4/open-compute/scheduler/{operation}"))
                    .header(header::AUTHORIZATION, "Bearer admin-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "scheduler {operation}");
    }

    for body in [
        serde_json::json!({"type":"http_pull","script_name":"consumer-worker"}),
        serde_json::json!({"type":"worker","script_name":"consumer-worker","environment_name":"production"}),
        serde_json::json!({"type":"worker","script_name":"missing"}),
        serde_json::json!({"type":"worker","script_name":"inactive-worker"}),
        serde_json::json!({"type":"worker","script_name":"consumer-worker","dead_letter_queue":"source-renamed"}),
        serde_json::json!({"type":"worker","script_name":"consumer-worker","settings":{"max_wait_time_ms":1}}),
    ] {
        let response = app
            .clone()
            .oneshot(json_request(Method::POST, &prefix, body))
            .await
            .unwrap();
        assert!(!response.status().is_success());
    }

    let created = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &prefix,
            serde_json::json!({
                "type":"worker",
                "script_name":"consumer-worker",
                "dead_letter_queue":"dead-letter",
                "environment_name":"",
                "settings":{
                    "batch_size":25,
                    "max_concurrency":4,
                    "max_retries":7,
                    "max_wait_time_ms":10_000,
                    "retry_delay":3
                }
            }),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let created = json(created).await;
    let consumer_id = created["result"]["consumer_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(created["result"]["dead_letter_queue"], "dead-letter");
    assert_eq!(created["result"]["settings"]["batch_size"], 25);

    let internal_consumer = QueueConsumerRepository::new(storage.db())
        .live_for_queue(queue)
        .unwrap()
        .unwrap();
    assert_eq!(
        api.delete_consumer(
            AccountId::generate(),
            internal_consumer.id,
            RequestId::generate(),
            10,
        )
        .unwrap_err()
        .code(),
        ErrorCode::ResourceNotFound
    );

    for paused in [false, true] {
        let response = app
            .clone()
            .oneshot(json_request(
                Method::PUT,
                &format!("{catalog}/{public_queue}"),
                serde_json::json!({"settings":{"delivery_paused":paused}}),
            ))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "delivery_paused={paused}"
        );
    }

    let duplicate = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            &prefix,
            serde_json::json!({"type":"worker","script_name":"consumer-worker"}),
        ))
        .await
        .unwrap();
    assert_eq!(duplicate.status(), StatusCode::CONFLICT);

    let listed = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&prefix)
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    assert_eq!(json(listed).await["result"].as_array().unwrap().len(), 1);

    let detail = format!("{prefix}/{consumer_id}");
    let fetched = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&detail)
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(fetched.status(), StatusCode::OK);

    let updated = app
        .clone()
        .oneshot(json_request(
            Method::PUT,
            &detail,
            serde_json::json!({
                "type":"worker",
                "script_name":"consumer-worker",
                "settings":{"batch_size":5,"max_concurrency":2}
            }),
        ))
        .await
        .unwrap();
    assert_eq!(updated.status(), StatusCode::OK);
    assert_eq!(json(updated).await["result"]["settings"]["batch_size"], 5);

    let bad_query = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("{detail}?unexpected=true"))
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bad_query.status(), StatusCode::BAD_REQUEST);
    let missing = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("{prefix}/00000000000000000000000000000000"))
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    let deleted = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri(&detail)
                .header(header::AUTHORIZATION, "Bearer deployer-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::OK);
    assert_eq!(json(deleted).await["result"]["success"], true);

    let after = app
        .oneshot(
            Request::builder()
                .uri(&prefix)
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(json(after).await["result"].as_array().unwrap().is_empty());
}

#[test]
fn helper_error_responses_and_timestamps_are_sanitized() {
    assert!(timestamp(0).is_ok());
    assert!(timestamp(i64::MAX).is_err());
    let context = crate::cloudflare_v4::V4RequestContext {
        role: crate::cloudflare_v4::V4Role::Admin,
        request_id: RequestId::generate(),
    };
    assert_eq!(
        respond::<()>(
            Ok(Err(PlatformError::new(ErrorCode::QueueNotFound, "test"))),
            context
        )
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        respond_consumer(
            Ok(Err(PlatformError::new(ErrorCode::LimitInvalid, "test"))),
            context
        )
        .status(),
        StatusCode::BAD_REQUEST
    );
}
