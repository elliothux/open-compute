use super::*;
#[path = "do_http_support_tests.rs"]
mod support;
use support::*;

use crate::health::HealthCoordinator;
use crate::http;
use crate::metrics::MetricsRegistry;
use open_compute_core::config::MetricsConfig;
use open_compute_runtime::GenerationAuthRegistry;
use open_compute_storage::{
    AlarmProjection, AuthorizedDurableObjectDelete, SchedulerStore, SchedulerSummary,
};
use serde_json::{Value, json};
use std::sync::Mutex;
use std::sync::atomic::Ordering;
use tower::ServiceExt as _;

#[tokio::test]
async fn namespace_crud_is_idempotent_bounded_and_hides_storage_identity() {
    let fixture = fixture();
    let collection = format!(
        "/v1/accounts/{}/durable-objects/namespaces",
        fixture.account
    );
    let create_body = json!({
        "name": "counters",
        "workerId": fixture.worker,
        "className": "Counter"
    });
    let (status, created) = json_response(
        fixture
            .router
            .clone()
            .oneshot(request(
                "POST",
                &collection,
                create_body.clone(),
                Some("create-counter"),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let namespace: ResourceId = created["resourceId"].as_str().unwrap().parse().unwrap();

    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request(
                "POST",
                &collection,
                create_body,
                Some("create-counter"),
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );

    let (status, listed) = json_response(
        fixture
            .router
            .clone()
            .oneshot(request("GET", &collection, Value::Null, None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["namespaces"].as_array().unwrap().len(), 1);
    let rendered = listed.to_string();
    assert!(!rendered.contains("namespaceStorageKey"));
    assert!(!rendered.contains("doStorageId"));

    let item = format!("{collection}/{namespace}");
    let (status, renamed) = json_response(
        fixture
            .router
            .clone()
            .oneshot(request("PATCH", &item, json!({"name": "renamed"}), None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(renamed["name"], "renamed");

    let oversized = "é".repeat(65);
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request("PATCH", &item, json!({"name": oversized}), None,))
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request(
                "POST",
                &collection,
                json!({
                    "name":"é".repeat(65),
                    "workerId":fixture.worker,
                    "className":"OversizedCounter"
                }),
                Some("oversized-name"),
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );

    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request("DELETE", &item, Value::Null, None))
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
    let (status, listed) = json_response(
        fixture
            .router
            .clone()
            .oneshot(request("GET", &collection, Value::Null, None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(listed["namespaces"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn namespace_create_rejects_duplicate_class_and_invalid_boundaries() {
    let fixture = fixture();
    let collection = format!(
        "/v1/accounts/{}/durable-objects/namespaces",
        fixture.account
    );
    let body =
        |name: &str| json!({"name": name, "workerId": fixture.worker, "className": "Counter"});
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request("POST", &collection, body("one"), Some("one"),))
            .await
            .unwrap()
            .status(),
        StatusCode::CREATED
    );
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request("POST", &collection, body("two"), Some("two"),))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request(
                "POST",
                &collection,
                json!({
                    "name": "one",
                    "workerId": fixture.worker,
                    "className": "OtherCounter"
                }),
                Some("one"),
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request("POST", &collection, body("missing-key"), None))
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    let (status, listed) = json_response(
        fixture
            .router
            .clone()
            .oneshot(request("GET", &collection, Value::Null, None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["namespaces"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn object_reconcile_get_delete_and_force_namespace_delete_converge() {
    let mut fixture = fixture();
    let scheduler_path = fixture.storage.data_dir().ensure_scheduler_db().unwrap();
    let scheduler = Arc::new(SchedulerStore::open(&scheduler_path, 100, 1).unwrap());
    fixture.api = fixture.api.clone().with_scheduler(Some(scheduler.clone()));
    fixture.router = http::admin_router(
        HttpState::for_test(
            HealthCoordinator::new(),
            Arc::new(MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd").unwrap()),
            false,
            None,
        )
        .with_do_api(fixture.api.clone()),
    );
    let namespace = create_namespace_fixture(&fixture, "objects", "Counter").await;
    let item = format!(
        "/v1/accounts/{}/durable-objects/namespaces/{namespace}",
        fixture.account
    );
    let (status, fetched) = json_response(
        fixture
            .router
            .clone()
            .oneshot(request("GET", &item, Value::Null, None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fetched["namespace"]["resourceId"], namespace.to_string());

    let object = object_id(namespace, 3);
    insert_object(
        &fixture.storage,
        namespace,
        object,
        DurableObjectState::Creating,
        20,
    );
    assert_eq!(fixture.api.reconcile_pending().await.unwrap(), 1);

    let objects = format!("{item}/objects");
    let (status, listed) = json_response(
        fixture
            .router
            .clone()
            .oneshot(request("GET", &objects, Value::Null, None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["objects"][0]["state"], "ready");

    let object_item = format!("{objects}/{object}");
    let (status, fetched) = json_response(
        fixture
            .router
            .clone()
            .oneshot(request("GET", &object_item, Value::Null, None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fetched["objectId"], object.to_string());
    scheduler
        .upsert_alarm(
            &AlarmProjection {
                namespace_resource_id: namespace,
                object_id: object,
                object_generation: 1,
                row_token: "delete-fence-token".to_owned(),
                due_at_ms: 100,
                target_deployment_id: open_compute_core::DeploymentId::generate(),
                execution_generation: 1,
                retry_count: 0,
            },
            50,
        )
        .unwrap();
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request("DELETE", &item, Value::Null, None))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request("DELETE", &object_item, Value::Null, None))
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(fixture.deletes.calls.lock().unwrap().len(), 1);
    assert_eq!(scheduler.summary(50).unwrap(), SchedulerSummary::default());
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request("DELETE", &item, Value::Null, None))
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );

    let forced = create_namespace_fixture(&fixture, "forced", "ForcedCounter").await;
    for fill in [4, 5] {
        insert_object(
            &fixture.storage,
            forced,
            object_id(forced, fill),
            DurableObjectState::Ready,
            i64::from(fill),
        );
    }
    let forced_item = format!(
        "/v1/accounts/{}/durable-objects/namespaces/{forced}?force=true",
        fixture.account
    );
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request("DELETE", &forced_item, Value::Null, None))
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(fixture.deletes.calls.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn control_errors_and_deleting_reconciliation_are_stable() {
    let fixture = fixture();
    assert!(format!("{:?}", fixture.api).contains("DoApiState"));
    let collection = format!(
        "/v1/accounts/{}/durable-objects/namespaces",
        fixture.account
    );
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request(
                "POST",
                "/v1/accounts/invalid/durable-objects/namespaces",
                json!({"name":"bad","workerId":fixture.worker,"className":"Counter"}),
                Some("bad-account"),
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request(
                "GET",
                "/v1/accounts/invalid/durable-objects/namespaces",
                Value::Null,
                None,
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    let wrong_content_type = Request::builder()
        .method("POST")
        .uri(&collection)
        .header(IDEMPOTENCY_HEADER, "wrong-content-type")
        .body(Body::from("{}"))
        .unwrap();
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(wrong_content_type)
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request(
                "POST",
                &collection,
                json!({"oversized":"x".repeat(MAX_JSON_BODY + 1)}),
                Some("oversized"),
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );

    for (method, suffix) in [
        ("GET", "/invalid"),
        ("PATCH", "/invalid"),
        ("GET", "/invalid/objects"),
        ("GET", "/invalid/objects/bad"),
        ("DELETE", "/invalid/objects/bad"),
        ("DELETE", "/invalid"),
    ] {
        assert_eq!(
            fixture
                .router
                .clone()
                .oneshot(request(
                    method,
                    &format!("{collection}{suffix}"),
                    json!({"name":"x"}),
                    None,
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST,
            "{method} {suffix}"
        );
    }

    let namespace = create_namespace_fixture(&fixture, "failure", "FailureCounter").await;
    let missing_namespace = ResourceId::generate();
    let missing_object = object_id(missing_namespace, 1);
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request(
                "PATCH",
                &format!("{collection}/{missing_namespace}"),
                json!({"name":"missing"}),
                None,
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    for uri in [
        format!("{collection}/{missing_namespace}"),
        format!("{collection}/{missing_namespace}/objects"),
        format!("{collection}/{missing_namespace}/objects/{missing_object}"),
    ] {
        assert_eq!(
            fixture
                .router
                .clone()
                .oneshot(request("GET", &uri, Value::Null, None))
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request(
                "DELETE",
                &format!("{collection}/{missing_namespace}"),
                Value::Null,
                None,
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request(
                "PATCH",
                &format!("{collection}/{namespace}"),
                json!({"unexpected":true}),
                None,
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    let occupied = create_namespace_fixture(&fixture, "occupied", "OccupiedCounter").await;
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request(
                "PATCH",
                &format!("{collection}/{namespace}"),
                json!({"name":"occupied"}),
                None,
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    let valid = object_id(namespace, 7);
    let objects = format!("{collection}/{namespace}/objects");
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request(
                "GET",
                &format!("{objects}/{valid}"),
                Value::Null,
                None,
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    for method in ["GET", "DELETE"] {
        assert_eq!(
            fixture
                .router
                .clone()
                .oneshot(request(
                    method,
                    &format!("{objects}/bad"),
                    Value::Null,
                    None,
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request(
                "DELETE",
                &format!("{objects}/{valid}"),
                Value::Null,
                None,
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );

    insert_object(
        &fixture.storage,
        namespace,
        valid,
        DurableObjectState::Ready,
        30,
    );
    fixture.deletes.fail_next.store(true, Ordering::SeqCst);
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request(
                "DELETE",
                &format!("{objects}/{valid}"),
                Value::Null,
                None,
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(fixture.api.reconcile_pending().await.unwrap(), 1);

    let referenced = create_namespace_fixture(&fixture, "referenced", "ReferencedCounter").await;
    let connection =
        rusqlite::Connection::open(fixture.storage.data_dir().root().join("control.sqlite"))
            .unwrap();
    connection
        .execute(
            "INSERT INTO resource_referrers(resource_id, referrer_kind, referrer_id, created_at_ms) \
             VALUES (?1, 'do_class', 'test', 32)",
            [referenced.to_string()],
        )
        .unwrap();
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request(
                "DELETE",
                &format!("{collection}/{referenced}"),
                Value::Null,
                None,
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    connection
        .execute(
            "DELETE FROM resource_referrers WHERE resource_id = ?1",
            [referenced.to_string()],
        )
        .unwrap();

    let force_failure =
        create_namespace_fixture(&fixture, "force-failure", "ForceFailureCounter").await;
    insert_object(
        &fixture.storage,
        force_failure,
        object_id(force_failure, 9),
        DurableObjectState::Ready,
        33,
    );
    fixture.deletes.fail_next.store(true, Ordering::SeqCst);
    let force_failure_uri = format!("{collection}/{force_failure}?force=true");
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request("DELETE", &force_failure_uri, Value::Null, None,))
            .await
            .unwrap()
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(fixture.api.reconcile_pending().await.unwrap(), 1);
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request("DELETE", &force_failure_uri, Value::Null, None,))
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );

    let pinned = create_namespace_fixture(&fixture, "pinned", "PinnedCounter").await;
    let pin = fixture.api.pins.try_pin(pinned).unwrap();
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request(
                "DELETE",
                &format!("{collection}/{pinned}"),
                Value::Null,
                None,
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    drop(pin);

    let deleting_namespace =
        create_namespace_fixture(&fixture, "deleting", "DeletingCounter").await;
    insert_object(
        &fixture.storage,
        deleting_namespace,
        object_id(deleting_namespace, 8),
        DurableObjectState::Deleting,
        31,
    );
    assert_eq!(
        fixture
            .router
            .clone()
            .oneshot(request(
                "DELETE",
                &format!("{collection}/{deleting_namespace}?force=true"),
                Value::Null,
                None,
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );

    let no_api_metrics =
        Arc::new(MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd").unwrap());
    let no_api = HttpState::for_test(HealthCoordinator::new(), no_api_metrics, false, None);
    let no_api_router = http::admin_router(no_api);
    for (method, uri) in [
        ("GET", collection.clone()),
        ("POST", collection.clone()),
        ("GET", format!("{collection}/{occupied}")),
        ("PATCH", format!("{collection}/{occupied}")),
        ("DELETE", format!("{collection}/{occupied}")),
        ("GET", format!("{collection}/{occupied}/objects")),
        ("GET", format!("{collection}/{occupied}/objects/{valid}")),
        ("DELETE", format!("{collection}/{occupied}/objects/{valid}")),
    ] {
        assert_eq!(
            no_api_router
                .clone()
                .oneshot(request(method, &uri, Value::Null, None))
                .await
                .unwrap()
                .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }

    let protected_metrics =
        Arc::new(MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd").unwrap());
    let protected = http::admin_router(
        HttpState::for_test(
            HealthCoordinator::new(),
            protected_metrics,
            false,
            Some(open_compute_core::SecretString::new("secret")),
        )
        .with_do_api(fixture.api.clone()),
    );
    assert_eq!(
        protected
            .oneshot(request("GET", &collection, Value::Null, None))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );

    for (code, status) in [
        (ErrorCode::DoNamespaceNotFound, StatusCode::NOT_FOUND),
        (ErrorCode::DoObjectDeleting, StatusCode::CONFLICT),
        (
            ErrorCode::DoStorageUnavailable,
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (
            ErrorCode::ResourceInvariantViolation,
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
    ] {
        assert_eq!(
            error_response(PlatformError::new(code, "test"), RequestId::generate()).status(),
            status
        );
    }
    assert_eq!(internal().code(), ErrorCode::DoStorageUnavailable);

    struct BadJson;
    impl Serialize for BadJson {
        fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            Err(serde::ser::Error::custom("test"))
        }
    }
    assert_eq!(
        json(&BadJson, StatusCode::OK).status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );

    let workerd = WorkerdTransport::new(GenerationAuthRegistry::new(), Arc::new(Mutex::new(None)));
    let authority = AuthorizedDurableObjectDelete {
        object_id: valid,
        object_generation: 1,
        host_key: "a".repeat(43),
    };
    assert!(
        DurableObjectDeleteTransport::delete(&workerd, &authority)
            .await
            .is_err()
    );
}
