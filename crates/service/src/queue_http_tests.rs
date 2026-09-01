use super::*;
use crate::health::HealthCoordinator;
use crate::http;
use crate::metrics::MetricsRegistry;
use axum::body::to_bytes;
use axum::http::Request;
use open_compute_core::config::{MetricsConfig, StorageConfig};
use open_compute_core::{SecretString, SystemClock};
use open_compute_storage::{
    QueueContentType, QueueEnqueueRequest, QueueMessageInput, QueueProjection,
};
use serde_json::{Value, json};
use tempfile::TempDir;
use tower::ServiceExt as _;

struct Fixture {
    _temp: TempDir,
    router: Router,
    account: AccountId,
    api: QueueApiState,
}

async fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data");
    let storage = Arc::new(
        PlatformStorage::bootstrap(
            &StorageConfig {
                data_dir: root.clone(),
                master_key_file: root.join("keys/master.key"),
                master_key_env: None,
                sqlite_busy_timeout_ms: 5_000,
                free_space_soft_bytes: 1_073_741_824,
                free_space_hard_bytes: 268_435_456,
            },
            &SystemClock,
        )
        .unwrap(),
    );
    let account = storage.identity().default_account_id;
    let scheduler_path = storage.data_dir().ensure_scheduler_db().unwrap();
    let scheduler = Arc::new(SchedulerStore::open(&scheduler_path, 5_000, 1).unwrap());
    let metrics =
        Arc::new(MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd").unwrap());
    let api = QueueApiState::new(storage, scheduler)
        .with_metrics(metrics.clone())
        .with_default_max_backlog_bytes(4096);
    assert_eq!(api.reconcile_pending().await.unwrap(), 0);
    let state = HttpState::for_test(HealthCoordinator::new(), metrics, false, None)
        .with_queue_api(Some(api.clone()));
    Fixture {
        _temp: temp,
        router: http::admin_router(state),
        account,
        api,
    }
}

#[tokio::test]
async fn queue_running_config_and_delete_intents_reconcile_after_transaction_crashes() {
    let fixture = fixture().await;
    let collection = format!("/v1/accounts/{}/queues", fixture.account);
    let (_, created) = response_json(
        fixture
            .router
            .clone()
            .oneshot(request(
                "POST",
                &collection,
                json!({ "name": "recoverable" }),
                Some("create-recovery"),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    let queue_id = QueueId::from_str(created["queue"]["id"].as_str().unwrap()).unwrap();

    let patch = PatchQueueBody {
        expected_config_generation: 1,
        name: None,
        delivery_delay_seconds: Some(7),
        retention_seconds: None,
        max_backlog_bytes: None,
    };
    let canonical = serde_json::to_vec(&patch).unwrap();
    let request_id = RequestId::generate();
    let intent = QueueMutationIntent::Patch {
        version: 1,
        request_id,
        body: patch,
    };
    let patch_mutation = RunningQueueMutation {
        account_id: fixture.account,
        scope: format!("queue.patch:{queue_id}"),
        idempotency_key: "crash-config".to_owned(),
        request_fingerprint: mutation_fingerprint(
            &fixture.api.storage,
            b"patch",
            queue_id,
            &canonical,
        ),
        queue_id,
        intent_json: serde_json::to_vec(&intent).unwrap(),
    };
    assert_eq!(
        reserve_mutation(&fixture.api.storage, &patch_mutation).unwrap(),
        IdempotencyReservation::Reserved
    );
    fixture
        .api
        .scheduler
        .begin_queue_config(queue_id, 1, 1, now_ms())
        .unwrap();
    assert_eq!(fixture.api.reconcile_pending().await.unwrap(), 1);
    let configured = QueueRepository::new(fixture.api.storage.db())
        .get(fixture.account, queue_id)
        .unwrap();
    assert_eq!(configured.config_generation, 2);
    assert_eq!(configured.config.delivery_delay_seconds, 7);
    assert_eq!(configured.availability, QueueAvailability::Healthy);

    let delete_request_id = RequestId::generate();
    let delete_intent = QueueMutationIntent::Delete {
        version: 1,
        request_id: delete_request_id,
        expected_lifecycle_generation: 1,
        force: false,
        purged_messages: None,
        purged_bytes: None,
    };
    let canonical = [1_u64.to_be_bytes().as_slice(), &[0_u8]].concat();
    let delete_mutation = RunningQueueMutation {
        account_id: fixture.account,
        scope: format!("queue.delete:{queue_id}"),
        idempotency_key: "crash-delete".to_owned(),
        request_fingerprint: mutation_fingerprint(
            &fixture.api.storage,
            b"delete",
            queue_id,
            &canonical,
        ),
        queue_id,
        intent_json: serde_json::to_vec(&delete_intent).unwrap(),
    };
    assert_eq!(
        reserve_mutation(&fixture.api.storage, &delete_mutation).unwrap(),
        IdempotencyReservation::Reserved
    );
    QueueRepository::new(fixture.api.storage.db())
        .begin_delete(fixture.account, queue_id, 1, now_ms())
        .unwrap();
    assert_eq!(fixture.api.reconcile_pending().await.unwrap(), 1);
    let deleted = QueueRepository::new(fixture.api.storage.db())
        .get(fixture.account, queue_id)
        .unwrap();
    assert_eq!(deleted.state, QueueState::Tombstoned);
}

#[tokio::test]
async fn queue_startup_reconcile_persists_final_failures_and_rejects_corrupt_intents() {
    let fixture = fixture().await;
    let account = fixture.account;
    let queue_id = QueueId::generate();
    let repository = QueueRepository::new(fixture.api.storage.db());
    let created = repository
        .insert_creating(
            account,
            queue_id,
            "reconcile-failure",
            QueueConfig::default(),
            1,
        )
        .unwrap();
    fixture
        .api
        .scheduler
        .create_queue_projection(&QueueProjection {
            queue_id,
            account_id: account,
            lifecycle_generation: 1,
            config_generation: 1,
            config: created.config,
            created_at_ms: 1,
            updated_at_ms: 1,
        })
        .unwrap();
    repository.mark_ready(account, queue_id, 2).unwrap();

    let patch = PatchQueueBody {
        expected_config_generation: 99,
        name: Some("never".to_owned()),
        delivery_delay_seconds: None,
        retention_seconds: None,
        max_backlog_bytes: None,
    };
    let canonical = serde_json::to_vec(&patch).unwrap();
    let intent = QueueMutationIntent::Patch {
        version: 1,
        request_id: RequestId::generate(),
        body: patch,
    };
    let fingerprint = mutation_fingerprint(&fixture.api.storage, b"patch", queue_id, &canonical);
    let mutation = RunningQueueMutation {
        account_id: account,
        scope: format!("queue.patch:{queue_id}"),
        idempotency_key: "startup-final-failure".to_owned(),
        request_fingerprint: fingerprint,
        queue_id,
        intent_json: serde_json::to_vec(&intent).unwrap(),
    };
    assert_eq!(
        reserve_mutation(&fixture.api.storage, &mutation).unwrap(),
        IdempotencyReservation::Reserved
    );
    assert_eq!(fixture.api.reconcile_pending().await.unwrap(), 1);
    assert!(matches!(
        reserve_mutation(&fixture.api.storage, &mutation).unwrap(),
        IdempotencyReservation::Failed(_)
    ));

    let corrupt = RunningQueueMutation {
        idempotency_key: "startup-corrupt-intent".to_owned(),
        request_fingerprint: [9; 32],
        intent_json: b"not-json".to_vec(),
        ..mutation
    };
    assert_eq!(
        reserve_mutation(&fixture.api.storage, &corrupt).unwrap(),
        IdempotencyReservation::Reserved
    );
    assert_eq!(
        fixture.api.reconcile_pending().await.unwrap_err().code(),
        ErrorCode::Internal
    );
}

#[allow(clippy::needless_pass_by_value)]
fn request(
    method: &str,
    uri: &str,
    body: Value,
    key: Option<&str>,
    expected_lifecycle: Option<u64>,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(key) = key {
        builder = builder.header(IDEMPOTENCY_HEADER, key);
    }
    if let Some(generation) = expected_lifecycle {
        builder = builder.header(EXPECTED_LIFECYCLE_HEADER, generation);
    }
    builder
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

async fn response_json(response: Response) -> (StatusCode, Value) {
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn queue_control_crud_replay_config_list_metrics_and_delete_round_trip() {
    let fixture = fixture().await;
    let collection = format!("/v1/accounts/{}/queues", fixture.account);
    let create = json!({
        "name": "events",
        "deliveryDelaySeconds": 2,
        "retentionSeconds": 120,
        "maxBacklogBytes": 4096
    });
    let (status, created) = response_json(
        fixture
            .router
            .clone()
            .oneshot(request(
                "POST",
                &collection,
                create.clone(),
                Some("create"),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let queue_id = created["queue"]["id"].as_str().unwrap();
    let (status, replay) = response_json(
        fixture
            .router
            .clone()
            .oneshot(request("POST", &collection, create, Some("create"), None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay["queue"]["id"], queue_id);

    let item = format!("{collection}/{queue_id}");
    let (status, got) = response_json(
        fixture
            .router
            .clone()
            .oneshot(request("GET", &item, json!(null), None, None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(got["metrics"]["backlogCount"], 0);
    let (status, patched) = response_json(
        fixture
            .router
            .clone()
            .oneshot(request(
                "PATCH",
                &item,
                json!({
                    "expectedConfigGeneration": 1,
                    "retentionSeconds": 180,
                    "maxBacklogBytes": 8192
                }),
                Some("patch"),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(patched["queue"]["configGeneration"], 2);
    assert_eq!(patched["queue"]["retentionSeconds"], 180);

    let (status, listed) = response_json(
        fixture
            .router
            .clone()
            .oneshot(request(
                "GET",
                &format!("{collection}?limit=1"),
                json!(null),
                None,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["queues"].as_array().unwrap().len(), 1);
    assert!(listed["nextCursor"].is_string());

    let (status, deleted) = response_json(
        fixture
            .router
            .clone()
            .oneshot(request(
                "DELETE",
                &item,
                json!(null),
                Some("delete"),
                Some(1),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(deleted["queue"]["state"], "tombstoned");
}

#[tokio::test]
async fn queue_control_rejects_missing_idempotency_ambiguous_patch_and_bad_query() {
    let fixture = fixture().await;
    let collection = format!("/v1/accounts/{}/queues", fixture.account);
    let (status, body) = response_json(
        fixture
            .router
            .clone()
            .oneshot(request(
                "POST",
                &collection,
                json!({ "name": "events" }),
                None,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "CONFIG_INVALID");
    let response = fixture
        .router
        .clone()
        .oneshot(request(
            "GET",
            &format!("{collection}?limit=0"),
            json!(null),
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(parse_cursor("not-a-cursor").is_err());
    assert!(parse_list_query(Some("limit=100&limit=1")).is_err());
}

#[tokio::test]
async fn queue_control_rename_failure_replay_and_force_delete_are_restart_safe() {
    let fixture = fixture().await;
    let collection = format!("/v1/accounts/{}/queues", fixture.account);
    let (status, created) = response_json(
        fixture
            .router
            .clone()
            .oneshot(request(
                "POST",
                &collection,
                json!({ "name": "mutable" }),
                Some("create-mutable"),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let queue_id = QueueId::from_str(created["queue"]["id"].as_str().unwrap()).unwrap();
    let item = format!("{collection}/{queue_id}");

    let rename = json!({ "expectedConfigGeneration": 1, "name": "renamed" });
    for expected in [StatusCode::OK, StatusCode::OK] {
        let (status, body) = response_json(
            fixture
                .router
                .clone()
                .oneshot(request(
                    "PATCH",
                    &item,
                    rename.clone(),
                    Some("rename"),
                    None,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, expected);
        assert_eq!(body["queue"]["name"], "renamed");
    }

    let stale = json!({ "expectedConfigGeneration": 2, "name": "never" });
    for _ in 0..2 {
        let (status, body) = response_json(
            fixture
                .router
                .clone()
                .oneshot(request(
                    "PATCH",
                    &item,
                    stale.clone(),
                    Some("stale-rename"),
                    None,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(
            body.pointer("/error/code")
                .or_else(|| body.get("code"))
                .and_then(Value::as_str),
            Some("QUEUE_CONFIG_PENDING")
        );
    }

    let queue = QueueRepository::new(fixture.api.storage.db())
        .get(fixture.account, queue_id)
        .unwrap();
    fixture
        .api
        .scheduler
        .enqueue_queue(
            &QueueEnqueueRequest {
                queue_id,
                request_id: uuid::Uuid::now_v7(),
                output_gate: false,
                lifecycle_generation: queue.lifecycle_generation,
                config_generation: queue.config_generation,
                batch_delay_seconds: None,
                messages: vec![QueueMessageInput {
                    content_type: QueueContentType::Text,
                    body: b"retained".to_vec(),
                    delay_seconds: None,
                }],
            },
            10,
        )
        .unwrap();

    for _ in 0..2 {
        let (status, body) = response_json(
            fixture
                .router
                .clone()
                .oneshot(request(
                    "DELETE",
                    &item,
                    json!(null),
                    Some("non-force"),
                    Some(1),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(
            body.pointer("/error/code")
                .or_else(|| body.get("code"))
                .and_then(Value::as_str),
            Some("QUEUE_NOT_EMPTY")
        );
    }

    let (status, body) = response_json(
        fixture
            .router
            .clone()
            .oneshot(request(
                "DELETE",
                &format!("{item}?force=true"),
                json!(null),
                Some("force"),
                Some(1),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(body["purgedMessages"], 1);
    assert_eq!(body["purgedBytes"], 8);
}

#[tokio::test]
async fn queue_control_helpers_cover_protocol_and_error_boundaries() {
    let fixture = fixture().await;
    let queue = QueueRepository::new(fixture.api.storage.db())
        .insert_creating(
            fixture.account,
            QueueId::generate(),
            "cursor",
            QueueConfig::default(),
            42,
        )
        .unwrap();
    let cursor = queue_cursor(&queue);
    assert_eq!(parse_cursor(&cursor).unwrap(), (42, queue.id));
    assert_eq!(parse_list_query(None).unwrap(), (None, 100));
    assert_eq!(
        parse_list_query(Some("limit=1&cursor="))
            .unwrap_err()
            .code(),
        ErrorCode::ConfigInvalid
    );
    assert_eq!(
        parse_list_query(Some(&format!("cursor={cursor}&limit=1"))).unwrap(),
        (Some((42, queue.id)), 1)
    );
    for invalid in ["limit", "unknown=1", "cursor=x&cursor=x", "limit=1001"] {
        assert!(parse_list_query(Some(invalid)).is_err());
    }
    assert!(!parse_force(None).unwrap());
    assert!(!parse_force(Some("")).unwrap());
    assert!(!parse_force(Some("force=false")).unwrap());
    assert!(parse_force(Some("force=true")).unwrap());
    assert!(parse_force(Some("force=1")).is_err());

    let valid = request("GET", "/", json!(null), Some("valid-key"), Some(9));
    assert_eq!(idempotency_key(&valid).unwrap(), "valid-key");
    assert_eq!(expected_lifecycle_generation(&valid).unwrap(), 9);
    assert_eq!(
        parse_account(&fixture.account.to_string()).unwrap(),
        fixture.account
    );
    assert_eq!(
        parse_ids(&fixture.account.to_string(), &queue.id.to_string()).unwrap(),
        (fixture.account, queue.id)
    );
    assert!(parse_account("bad").is_err());
    assert!(parse_ids(&fixture.account.to_string(), "bad").is_err());
    for invalid in ["", "white space"] {
        let request = request("GET", "/", json!(null), Some(invalid), None);
        assert!(idempotency_key(&request).is_err());
    }
    let long = "x".repeat(129);
    let long_request = request("GET", "/", json!(null), Some(&long), None);
    assert!(idempotency_key(&long_request).is_err());
    let missing = request("GET", "/", json!(null), None, None);
    assert!(expected_lifecycle_generation(&missing).is_err());
    let mut with_id = request("GET", "/", json!(null), None, None);
    let fixed = RequestId::generate();
    with_id.extensions_mut().insert(fixed);
    assert_eq!(request_id(&with_id), fixed);
    assert_ne!(request_id(&missing), request_id(&missing));

    let parsed: CreateQueueBody = read_json(request(
        "POST",
        "/",
        json!({ "name": "parsed" }),
        None,
        None,
    ))
    .await
    .unwrap();
    assert_eq!(parsed.name, "parsed");
    assert!(
        read_json::<CreateQueueBody>(Request::new(Body::from("{")))
            .await
            .is_err()
    );
    assert!(
        read_json::<CreateQueueBody>(Request::new(Body::from(vec![b'x'; MAX_JSON_BODY + 1])))
            .await
            .is_err()
    );

    for (code, status) in [
        (ErrorCode::QueueNotFound, StatusCode::NOT_FOUND),
        (ErrorCode::AdminAuthRequired, StatusCode::UNAUTHORIZED),
        (ErrorCode::BindingPermissionDenied, StatusCode::FORBIDDEN),
        (ErrorCode::QueueNameConflict, StatusCode::CONFLICT),
        (
            ErrorCode::QueueMessageTooLarge,
            StatusCode::PAYLOAD_TOO_LARGE,
        ),
        (
            ErrorCode::QueueBacklogLimitExceeded,
            StatusCode::TOO_MANY_REQUESTS,
        ),
        (ErrorCode::StoragePressure, StatusCode::INSUFFICIENT_STORAGE),
        (
            ErrorCode::QueueStorageUnavailable,
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (ErrorCode::Internal, StatusCode::INTERNAL_SERVER_ERROR),
    ] {
        assert_eq!(
            error_response(PlatformError::new(code, "test"), fixed).status(),
            status
        );
    }
    for code in [
        ErrorCode::QueueNotFound,
        ErrorCode::QueueNameConflict,
        ErrorCode::QueueNotReady,
        ErrorCode::QueueConfigPending,
        ErrorCode::QueueReferenced,
        ErrorCode::QueueNotEmpty,
        ErrorCode::ConfigInvalid,
        ErrorCode::LimitInvalid,
        ErrorCode::QuotaExceeded,
    ] {
        assert!(is_final_mutation_failure(code));
    }
    assert!(!is_final_mutation_failure(ErrorCode::Internal));
    assert_eq!(idempotency_running().code(), ErrorCode::IdempotencyConflict);
    assert_eq!(internal().code(), ErrorCode::Internal);

    assert_eq!(
        mutation_response(
            Ok(Ok(MutationOutcome::Applied(b"{}".to_vec()))),
            fixed,
            StatusCode::ACCEPTED,
        )
        .status(),
        StatusCode::ACCEPTED
    );
    assert_eq!(
        mutation_response(
            Ok(Ok(MutationOutcome::Replay(b"{}".to_vec()))),
            fixed,
            StatusCode::ACCEPTED,
        )
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        mutation_response(
            Ok(Ok(MutationOutcome::Failed(b"{}".to_vec()))),
            fixed,
            StatusCode::ACCEPTED,
        )
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        mutation_response(
            Ok(Err(PlatformError::new(ErrorCode::QueueNotFound, "test"))),
            fixed,
            StatusCode::OK,
        )
        .status(),
        StatusCode::NOT_FOUND
    );

    let metrics =
        Arc::new(MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd").unwrap());
    let unavailable = HttpState::for_test(HealthCoordinator::new(), metrics.clone(), false, None);
    assert_eq!(
        unauthorized_or_unavailable(&unavailable, &missing, fixed).status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    let protected = HttpState::for_test(
        HealthCoordinator::new(),
        metrics,
        false,
        Some(SecretString::new("secret")),
    );
    assert_eq!(
        unauthorized_or_unavailable(&protected, &missing, fixed).status(),
        StatusCode::UNAUTHORIZED
    );
}
