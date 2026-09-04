use super::*;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use open_compute_core::{PlatformId, RequestId, SecretString, VersionId};
use open_compute_storage::{NewVersion, NewVersionProducts, VersionContentKind, WorkerRepository};
use std::net::{IpAddr, Ipv4Addr};
use tower::ServiceExt as _;

async fn json(response: Response) -> serde_json::Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap()).unwrap()
}

fn post(uri: &str, token: &str, body: &serde_json::Value) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap()
}

#[test]
fn wire_filters_normalize_all_supported_script_tail_shapes() {
    let wrangler: TailCreateBody = serde_json::from_value(serde_json::json!([])).unwrap();
    assert!(wrangler.filters().is_empty());
    let version = VersionId::generate();
    let values: Vec<TailFilterWire> = serde_json::from_value(serde_json::json!([
        {"sampling_rate":0.5},
        {"outcome":["ok"]},
        {"method":["get"]},
        {"header":{"key":"X-Test","query":"value"}},
        {"client_ip":["self","192.0.2.1"]},
        {"query":"needle"},
        {"scriptVersion":version.to_string()}
    ]))
    .unwrap();
    let peer = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let filters = values
        .into_iter()
        .map(|value| tail_filter(value, Some(peer)).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(filters.len(), 7);
    assert!(matches!(&filters[2], TailFilter::Method(value) if value == &["GET"]));
    assert!(matches!(&filters[3], TailFilter::Header { key, .. } if key == "x-test"));
    assert!(matches!(&filters[4], TailFilter::ClientIp(value) if value[0] == peer));

    for value in [
        serde_json::json!({"client_ip":["self"]}),
        serde_json::json!({"client_ip":["invalid"]}),
        serde_json::json!({"scriptVersion":"invalid"}),
    ] {
        let filter: TailFilterWire = serde_json::from_value(value).unwrap();
        assert!(tail_filter(filter, None).is_err());
    }
}

#[tokio::test]
async fn script_and_live_tail_routes_create_list_heartbeat_and_revoke_sessions() {
    let (_temp, _mock, state, account, storage) =
        crate::tests::initialized_worker_http_fixture().await;
    let worker = WorkerRepository::new(storage.db())
        .create_worker(account, "tail-worker", RequestId::generate(), 1, 100)
        .unwrap()
        .0;
    let observability = state.worker_api().unwrap().observability().unwrap().clone();
    let authority =
        crate::cloudflare_v4::accounts::AccountAuthority::new(PlatformId::generate(), account, 1);
    let public_account = authority.public_id().to_owned();
    let app = crate::http::admin_router(
        state
            .with_v4_tokens(
                SecretString::new("deployer-token"),
                SecretString::new("read-token"),
            )
            .with_cloudflare_v4_account(authority),
    );
    let tails = format!("/client/v4/accounts/{public_account}/workers/scripts/tail-worker/tails");
    for request in [
        Request::builder().uri(&tails).body(Body::empty()).unwrap(),
        Request::builder()
            .method(Method::DELETE)
            .uri(format!("{tails}/missing"))
            .body(Body::empty())
            .unwrap(),
    ] {
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    let invalid_tail = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(&tails)
                .header(header::AUTHORIZATION, "Bearer read-token")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(invalid_tail.status(), StatusCode::BAD_REQUEST);
    let created = app
        .clone()
        .oneshot(post(
            &tails,
            "read-token",
            &serde_json::json!({"filters":[
                {"sampling_rate":0.5},
                {"outcome":["ok"]},
                {"method":["get"]},
                {"header":{"key":"X-Test","query":"value"}},
                {"query":"needle"},
                {"scriptVersion":VersionId::generate().to_string()}
            ]}),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let created = json(created).await;
    let tail_id = created["result"]["id"].as_str().unwrap().to_owned();
    let tail_url = created["result"]["url"].as_str().unwrap();
    assert!(tail_url.contains(&tail_id));
    let tail_ticket = url::Url::parse(tail_url)
        .unwrap()
        .path_segments()
        .unwrap()
        .next_back()
        .unwrap()
        .to_owned();
    let first_connection = observability.connect_tail(&tail_id, &tail_ticket).unwrap();
    assert!(observability.connect_tail(&tail_id, &tail_ticket).is_err());
    assert!(!observability.tail_overloaded(&tail_id));
    observability.disconnect_tail(&tail_id);
    drop(first_connection);
    let second_connection = observability.connect_tail(&tail_id, &tail_ticket).unwrap();
    observability.disconnect_tail(&tail_id);
    drop(second_connection);

    let listed = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&tails)
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    assert_eq!(json(listed).await["result"].as_array().unwrap().len(), 1);

    let live =
        format!("/client/v4/accounts/{public_account}/workers/observability/telemetry/live-tail");
    for uri in [&live, &format!("{live}/heartbeat")] {
        let unauthorized = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
        let invalid = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(uri)
                    .header(header::AUTHORIZATION, "Bearer deployer-token")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("{"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    }
    let created_live = app
        .clone()
        .oneshot(post(
            &live,
            "deployer-token",
            &serde_json::json!({
                "scriptId":"tail-worker",
                "filterCombination":"AND",
                "filters":[{"key":"source.message","operation":"includes","type":"string","value":"needle"}]
            }),
        ))
        .await
        .unwrap();
    assert_eq!(created_live.status(), StatusCode::OK);
    let created_live = json(created_live).await;
    let live_url = created_live["result"]["wsUrl"].as_str().unwrap();
    let live_segments = url::Url::parse(live_url)
        .unwrap()
        .path_segments()
        .unwrap()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let live_id = &live_segments[live_segments.len() - 2];
    let live_ticket = live_segments.last().unwrap();
    assert!(observability.connect_tail(live_id, live_ticket).is_err());
    let live_connection = observability
        .connect_live_tail(live_id, live_ticket)
        .unwrap();
    assert!(
        observability
            .connect_live_tail(live_id, live_ticket)
            .is_err()
    );

    let heartbeat = app
        .clone()
        .oneshot(post(
            &format!("{live}/heartbeat"),
            "deployer-token",
            &serde_json::json!({"scriptId":"tail-worker"}),
        ))
        .await
        .unwrap();
    assert_eq!(heartbeat.status(), StatusCode::OK);
    drop(live_connection);
    observability.close_live_tail(live_id);
    assert!(observability.tail_expired(live_id));
    observability.disconnect_tail("missing");

    for body in [
        serde_json::json!({}),
        serde_json::json!({"scriptId":"missing"}),
        serde_json::json!({"scriptId":"tail-worker","filters":[{"key":"","operation":"exists","type":"string"}]}),
    ] {
        let response = app
            .clone()
            .oneshot(post(&live, "deployer-token", &body))
            .await
            .unwrap();
        assert!(!response.status().is_success());
    }

    let deleted = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri(format!("{tails}/{tail_id}"))
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::OK);
    let missing = app
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri(format!("{tails}/{tail_id}"))
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    let extra = observability
        .create_tail(account, &worker, Vec::new(), RequestId::generate())
        .unwrap();
    assert_eq!(
        observability.list_tails(account, worker.id).unwrap().len(),
        1
    );
    assert!(
        observability
            .delete_tail(
                open_compute_core::AccountId::generate(),
                worker.id,
                &extra.id,
                RequestId::generate(),
            )
            .is_err()
    );
    observability.revoke_worker_tails(account, worker.id);
    assert_eq!(observability.session_count(), 0);
}

#[tokio::test]
async fn cursor_and_ingest_envelopes_are_cryptographically_bound_and_bounded() {
    let (_temp, _mock, state, account, _storage) =
        crate::tests::initialized_worker_http_fixture().await;
    let observability = state.worker_api().unwrap().observability().unwrap();
    let cursor = open_compute_storage::ObservabilityEventCursor {
        timestamp_ms: 123,
        event_id: "event".to_owned(),
    };
    let encoded = observability
        .encode_cursor(account, "query", 10, 20, &cursor)
        .unwrap();
    assert_eq!(
        observability
            .decode_cursor(&encoded, account, "query", 10, 20)
            .unwrap(),
        cursor
    );
    for candidate in [
        "missing-separator".to_owned(),
        format!("{encoded}x"),
        encoded.replacen('.', ".not-base64.", 1),
    ] {
        assert!(
            observability
                .decode_cursor(&candidate, account, "query", 10, 20)
                .is_err()
        );
    }
    for (candidate_account, query, from, to) in [
        (open_compute_core::AccountId::generate(), "query", 10, 20),
        (account, "other", 10, 20),
        (account, "query", 11, 20),
        (account, "query", 10, 21),
    ] {
        assert!(
            observability
                .decode_cursor(&encoded, candidate_account, query, from, to)
                .is_err()
        );
    }

    for bytes in [
        Vec::new(),
        vec![b'x'; 256 * 1024 + 1],
        b"not-json".to_vec(),
        serde_json::to_vec(&serde_json::json!({
            "schemaVersion":2,
            "collectorEventId":"event",
            "identity":{},
            "items":[{}]
        }))
        .unwrap(),
    ] {
        assert!(observability.ingest(&bytes).is_err());
    }
    observability.observe_ingest_result(true);
    observability.observe_ingest_result(false);
    observability.observe_query(true, true, Duration::from_millis(1));
    observability.observe_query(false, false, Duration::from_millis(1));
    assert_eq!(observability.tail_drop_counts(), [0, 0]);
    assert!(observability.store().is_some());
    assert!(format!("{observability:?}").contains("ObservabilityService"));
}

#[tokio::test]
async fn authorized_ingest_fans_out_persists_and_enforces_the_session_limit() {
    let (_temp, _mock, state, account, storage) =
        crate::tests::initialized_worker_http_fixture().await;
    let repository = WorkerRepository::new(storage.db());
    let (created, _) = repository
        .create_worker(account, "ingest-worker", RequestId::generate(), 1, 100)
        .unwrap();
    let version_id = VersionId::generate();
    repository
        .insert_staging_version(
            &NewVersion {
                id: version_id,
                account_id: account,
                worker_id: created.id,
                content_kind: VersionContentKind::Worker,
                artifact_sha256: Some([1; 32]),
                artifact_size: Some(100),
                artifact_schema_version: Some(1),
                main_module: Some("index.js".to_owned()),
                worker_code_sha256: [2; 32],
                compatibility_date: "2026-08-30".to_owned(),
                compatibility_flags: Vec::new(),
                vars: Default::default(),
                secrets: Default::default(),
                request_id: RequestId::generate(),
                now_ms: 2,
            },
            &NewVersionProducts::default(),
            100,
        )
        .unwrap();
    repository.begin_validation(version_id).unwrap();
    repository.mark_ready(version_id, 3).unwrap();
    let worker = repository
        .promote(
            account,
            created.id,
            version_id,
            None,
            RequestId::generate(),
            4,
        )
        .unwrap();
    let settings = repository
        .get_observability_settings(account, worker.id)
        .unwrap();
    let observability = state.worker_api().unwrap().observability().unwrap().clone();

    let tail = observability
        .create_tail(account, &worker, Vec::new(), RequestId::generate())
        .unwrap();
    let tail_ticket = url::Url::parse(&tail.url)
        .unwrap()
        .path_segments()
        .unwrap()
        .next_back()
        .unwrap()
        .to_owned();
    let mut tail_connection = observability.connect_tail(&tail.id, &tail_ticket).unwrap();
    let live = observability
        .create_live_tail(
            account,
            &worker,
            Combination::And,
            Vec::new(),
            RequestId::generate(),
        )
        .unwrap();
    let live_segments = url::Url::parse(&live.ws_url)
        .unwrap()
        .path_segments()
        .unwrap()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let live_id = &live_segments[live_segments.len() - 2];
    let live_ticket = live_segments.last().unwrap();
    let mut live_connection = observability
        .connect_live_tail(live_id, live_ticket)
        .unwrap();

    let loader_name = format!("{}/{}/{}", account, worker.id, version_id);
    let foreign_loader_name = format!(
        "{}/{}/{}",
        account,
        open_compute_core::WorkerId::generate(),
        version_id
    );
    let item = serde_json::json!({
        "outcome":"ok",
        "cpuTime":1.5,
        "wallTime":2.5,
        "event":{"request":{"method":"GET","headers":{"x-test":"value"}}},
        "logs":[{"level":"log","message":["hello",7]}],
        "exceptions":[{"message":"failure"}]
    });
    let mut same_name = item.clone();
    same_name["scriptName"] = serde_json::json!("ingest-worker");
    let mut loader_item = item.clone();
    loader_item["scriptName"] = serde_json::json!(loader_name);
    let body = serde_json::to_vec(&serde_json::json!({
        "schemaVersion":1,
        "collectorEventId":"collector-event",
        "batchTruncated":true,
        "identity":{
            "schemaVersion":1,
            "accountId":account.to_string(),
            "workerId":worker.id.to_string(),
            "scriptName":worker.name.clone(),
            "versionId":version_id.to_string(),
            "deploymentId":worker.active_deployment_id.unwrap().to_string(),
            "routeGeneration":worker.route_generation,
            "observabilityGeneration":settings.generation,
            "enabled":settings.enabled,
            "logsEnabled":settings.logs_enabled,
            "headSamplingRate":settings.effective_head_sampling_rate(),
            "invocationLogs":settings.invocation_logs,
            "persist":settings.persist
        },
        "items":[
            item,
            same_name,
            loader_item,
            {"scriptName":7},
            {"scriptName":"invalid"},
            {"scriptName":foreign_loader_name}
        ]
    }))
    .unwrap();
    observability.ingest(&body).unwrap();

    let tail_frame = tokio::time::timeout(Duration::from_secs(1), tail_connection.receiver.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(tail_frame.text.contains("ingest-worker"));
    let live_frame = tokio::time::timeout(Duration::from_secs(1), live_connection.receiver.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(live_frame.text.contains("ingest-worker"));

    let store = observability.store().unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if !store
                .query_events(
                    &account.to_string(),
                    0,
                    i64::MAX,
                    Some("ingest-worker"),
                    None,
                    100,
                )
                .unwrap()
                .is_empty()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    observability.revoke_worker_tails(account, worker.id);
    let maximum = usize::from(observability.config().max_tail_sessions_per_script);
    for _ in 0..maximum {
        observability
            .create_tail(account, &worker, Vec::new(), RequestId::generate())
            .unwrap();
    }
    assert_eq!(observability.session_count(), maximum);
    assert_eq!(
        observability
            .create_tail(account, &worker, Vec::new(), RequestId::generate())
            .unwrap_err()
            .code(),
        open_compute_core::ErrorCode::AdmissionBusy
    );
    observability.revoke_worker_tails(account, worker.id);
}
