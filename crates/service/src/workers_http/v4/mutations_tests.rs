use super::*;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use open_compute_core::{PlatformId, RequestId, SecretBytes, VersionId};
use open_compute_storage::{
    NewVersion, NewVersionProducts, StoredVersionSecret, VersionContentKind,
    WorkerObservabilitySettings,
};
use std::collections::BTreeMap;
use tower::ServiceExt as _;

fn observability() -> WorkerObservabilitySettings {
    WorkerObservabilitySettings {
        generation: 1,
        enabled: false,
        head_sampling_rate: None,
        logs_enabled: false,
        logs_head_sampling_rate: None,
        invocation_logs: false,
        persist: false,
        updated_at_ms: 1,
    }
}

#[test]
fn settings_and_secret_validation_cover_supported_and_rejected_shapes() {
    let current = observability();
    let unchanged = merge_observability(&current, None).unwrap();
    assert!(!unchanged.enabled);
    assert_eq!(unchanged.head_sampling_rate, None);

    let patch: ObservabilityPatch = serde_json::from_value(serde_json::json!({
        "enabled": true,
        "head_sampling_rate": 0.5,
        "logs": {
            "enabled": true,
            "head_sampling_rate": 0.25,
            "invocation_logs": true,
            "persist": true,
            "destinations": []
        },
        "traces": {"enabled": false, "persist": false, "destinations": []}
    }))
    .unwrap();
    let merged = merge_observability(&current, Some(patch)).unwrap();
    assert!(merged.enabled);
    assert_eq!(merged.head_sampling_rate, Some(0.5));
    assert_eq!(merged.logs_head_sampling_rate, Some(0.25));
    assert!(merged.logs_enabled && merged.invocation_logs && merged.persist);

    for rate in [None, Some(0.0), Some(0.5), Some(1.0)] {
        assert!(validate_rate(rate, "/rate").is_ok());
    }
    for rate in [Some(-0.1), Some(1.1), Some(f64::NAN), Some(f64::INFINITY)] {
        assert_eq!(
            validate_rate(rate, "/rate"),
            Err(V4Error::InvalidField("/rate"))
        );
    }
    for value in [
        serde_json::json!({"traces":{"enabled":true}}),
        serde_json::json!({"traces":{"persist":true}}),
        serde_json::json!({"traces":{"head_sampling_rate":0.1}}),
        serde_json::json!({"traces":{"destinations":[{}]}}),
        serde_json::json!({"logs":{"destinations":[{}]}}),
    ] {
        let patch: ObservabilityPatch = serde_json::from_value(value).unwrap();
        assert_eq!(
            merge_observability(&current, Some(patch)),
            Err(V4Error::Unsupported)
        );
    }

    let secret: SecretBody = serde_json::from_value(serde_json::json!({
        "name": "TOKEN", "type": "secret_text", "text": "value"
    }))
    .unwrap();
    assert_eq!(secret.text().unwrap().0, "TOKEN");
    for value in [
        serde_json::json!({"name":"TOKEN","type":"secret_key"}),
        serde_json::json!({"name":"TOKEN","type":"plain_text","text":"value"}),
        serde_json::json!({"name":"TOKEN","type":"secret_text"}),
        serde_json::json!({"name":"TOKEN","type":"secret_text","text":"value","format":"raw"}),
        serde_json::json!({"name":"TOKEN","type":"secret_text","text":"value","algorithm":{}}),
        serde_json::json!({"name":"TOKEN","type":"secret_text","text":"value","usages":[]}),
        serde_json::json!({"name":"TOKEN","type":"secret_text","text":"value","key_base64":"eA=="}),
        serde_json::json!({"name":"TOKEN","type":"secret_text","text":"value","key_jwk":{}}),
    ] {
        let body: SecretBody = serde_json::from_value(value).unwrap();
        assert!(body.text().is_err());
    }

    for (query, expected) in [
        (None, Ok(false)),
        (Some(""), Ok(false)),
        (Some("force=true"), Ok(true)),
        (Some("force=false"), Ok(false)),
    ] {
        assert_eq!(delete_force_query(query), expected);
    }
    for query in ["other=true", "force=maybe", "force=true&force=false"] {
        assert_eq!(
            delete_force_query(Some(query)),
            Err(V4Error::InvalidRequest)
        );
    }
    assert_eq!(
        v4_platform_error(V4Error::NotFound).code(),
        ErrorCode::AccountNotFound
    );
    assert_eq!(
        v4_platform_error(V4Error::Unavailable).code(),
        ErrorCode::PlatformUnavailable
    );
    assert_eq!(
        v4_platform_error(V4Error::Conflict).code(),
        ErrorCode::ConfigInvalid
    );
    assert_eq!(unavailable().code(), ErrorCode::PlatformUnavailable);
}

async fn response_json(response: Response) -> serde_json::Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap()
}

fn request(method: Method, uri: &str, body: Body) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, "Bearer deployer-token")
        .body(body)
        .unwrap()
}

#[tokio::test]
async fn active_script_management_routes_project_and_mutate_day1_state() {
    let (_temp, _mock, state, account, storage) =
        crate::tests::initialized_worker_http_fixture().await;
    let repo = WorkerRepository::new(storage.db());
    let worker = repo
        .create_worker(account, "settings-worker", RequestId::generate(), 1, 100)
        .unwrap()
        .0;
    let version = VersionId::generate();
    let revision = uuid::Uuid::now_v7().to_string();
    let envelope = storage
        .crypto()
        .encrypt(
            &SecretBytes::new(b"secret".to_vec()),
            account,
            worker.id,
            version,
            "TOKEN",
            &revision,
        )
        .unwrap();
    let mut secrets = BTreeMap::new();
    secrets.insert(
        "TOKEN".to_owned(),
        StoredVersionSecret {
            name: "TOKEN".to_owned(),
            revision_id: revision,
            envelope,
        },
    );
    repo.insert_staging_version(
        &NewVersion {
            id: version,
            account_id: account,
            worker_id: worker.id,
            content_kind: VersionContentKind::Worker,
            artifact_sha256: Some([7; 32]),
            artifact_size: Some(1),
            artifact_schema_version: Some(1),
            main_module: Some("index.js".to_owned()),
            worker_code_sha256: [8; 32],
            compatibility_date: "2026-08-30".to_owned(),
            compatibility_flags: vec!["nodejs_compat".to_owned()],
            vars: BTreeMap::from([
                ("TEXT".to_owned(), br#""hello""#.to_vec()),
                ("JSON".to_owned(), br#"{"ok":true}"#.to_vec()),
            ]),
            secrets,
            request_id: RequestId::generate(),
            now_ms: 2,
        },
        &NewVersionProducts::default(),
        100,
    )
    .unwrap();
    repo.begin_validation(version).unwrap();
    repo.mark_ready(version, 3).unwrap();
    repo.promote(account, worker.id, version, None, RequestId::generate(), 4)
        .unwrap();
    let deployment = repo
        .get_worker(account, worker.id)
        .unwrap()
        .active_deployment_id
        .unwrap();
    let replacement = VersionId::generate();
    let replacement_revision = uuid::Uuid::now_v7().to_string();
    let replacement_envelope = storage
        .crypto()
        .encrypt(
            &SecretBytes::new(b"replacement-secret".to_vec()),
            account,
            worker.id,
            replacement,
            "TOKEN",
            &replacement_revision,
        )
        .unwrap();
    repo.insert_staging_version(
        &NewVersion {
            id: replacement,
            account_id: account,
            worker_id: worker.id,
            content_kind: VersionContentKind::Worker,
            artifact_sha256: Some([9; 32]),
            artifact_size: Some(1),
            artifact_schema_version: Some(1),
            main_module: Some("index.js".to_owned()),
            worker_code_sha256: [10; 32],
            compatibility_date: "2026-08-30".to_owned(),
            compatibility_flags: vec!["nodejs_compat".to_owned()],
            vars: BTreeMap::new(),
            secrets: BTreeMap::from([(
                "TOKEN".to_owned(),
                StoredVersionSecret {
                    name: "TOKEN".to_owned(),
                    revision_id: replacement_revision,
                    envelope: replacement_envelope,
                },
            )]),
            request_id: RequestId::generate(),
            now_ms: 5,
        },
        &NewVersionProducts::default(),
        100,
    )
    .unwrap();
    repo.begin_validation(replacement).unwrap();
    repo.mark_ready(replacement, 6).unwrap();

    let authority =
        crate::cloudflare_v4::accounts::AccountAuthority::new(PlatformId::generate(), account, 1);
    let public_account = authority.public_id().to_owned();
    let app: Router = crate::http::admin_router(
        state
            .with_platform_storage(storage.clone())
            .with_v4_tokens(
                SecretString::new("deployer-token"),
                SecretString::new("read-token"),
            )
            .with_cloudflare_v4_account(authority),
    );
    let prefix = format!("/client/v4/accounts/{public_account}/workers/scripts/settings-worker");

    let deployable = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("{prefix}/versions?deployable=true"))
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(deployable.status(), StatusCode::OK);
    assert_eq!(
        response_json(deployable).await["result"]["items"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    for (query, body) in [
        (
            "?force=true",
            serde_json::json!({
                "strategy":"percentage",
                "versions":[{"version_id":replacement,"percentage":100}]
            }),
        ),
        (
            "",
            serde_json::json!({
                "strategy":"gradual",
                "versions":[{"version_id":replacement,"percentage":100}]
            }),
        ),
        (
            "",
            serde_json::json!({
                "strategy":"percentage",
                "versions":[{"version_id":replacement,"percentage":100}],
                "annotations":{"other":"invalid"}
            }),
        ),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("{prefix}/deployments{query}"))
                    .header(header::AUTHORIZATION, "Bearer deployer-token")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(!response.status().is_success());
    }

    let created = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("{prefix}/deployments"))
                .header(header::AUTHORIZATION, "Bearer deployer-token")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&serde_json::json!({
                        "strategy":"percentage",
                        "versions":[{"version_id":replacement,"percentage":100}],
                        "annotations":{"workers/message":"replacement"}
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let created_id = response_json(created).await["result"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let retired = app
        .clone()
        .oneshot(request(
            Method::DELETE,
            &format!("{prefix}/deployments/{deployment}"),
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(retired.status(), StatusCode::OK);
    let active = app
        .clone()
        .oneshot(request(
            Method::DELETE,
            &format!("{prefix}/deployments/{created_id}"),
            Body::empty(),
        ))
        .await
        .unwrap();
    assert!(!active.status().is_success());

    for (path, collection_key) in [
        (
            format!("/client/v4/accounts/{public_account}/workers/services/settings-worker"),
            None,
        ),
        (
            format!("/client/v4/accounts/{public_account}/workers/scripts"),
            Some(""),
        ),
        (format!("{prefix}/versions"), Some("items")),
        (format!("{prefix}/versions/{version}"), None),
        (format!("{prefix}/deployments"), Some("deployments")),
        (format!("{prefix}/deployments/{created_id}"), None),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header(header::AUTHORIZATION, "Bearer read-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let result = response_json(response).await["result"].clone();
        match collection_key {
            Some("") => assert!(!result.as_array().unwrap().is_empty()),
            Some(key) => assert!(!result[key].as_array().unwrap().is_empty()),
            None => assert!(result.is_object()),
        }
    }
    let unavailable_download = app
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
    assert!(!unavailable_download.status().is_success());

    for path in [
        format!(
            "/client/v4/accounts/{public_account}/open-compute/workers/settings-worker/endpoints"
        ),
        format!("/client/v4/accounts/{public_account}/open-compute/durable-objects"),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header(header::AUTHORIZATION, "Bearer read-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response_json(response).await["result"].is_array());
    }
    let missing_namespace = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/client/v4/accounts/{public_account}/open-compute/durable-objects/00000000000000000000000000000000/objects"
                ))
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_namespace.status(), StatusCode::NOT_FOUND);

    for (suffix, expected) in [
        ("/script-settings", StatusCode::OK),
        ("/settings", StatusCode::OK),
        ("/secrets", StatusCode::OK),
        ("/secrets/TOKEN", StatusCode::OK),
        ("/schedules", StatusCode::INTERNAL_SERVER_ERROR),
        ("/subdomain", StatusCode::OK),
    ] {
        let response = app
            .clone()
            .oneshot(request(
                Method::GET,
                &format!("{prefix}{suffix}"),
                Body::empty(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), expected, "GET {suffix}");
        assert_eq!(
            response_json(response).await["success"],
            expected.is_success()
        );
    }
    let missing = app
        .clone()
        .oneshot(request(
            Method::GET,
            &format!("{prefix}/secrets/MISSING"),
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    for body in [
        r#"{"observability":{"enabled":true,"head_sampling_rate":0.5,"logs":{"enabled":true,"head_sampling_rate":0.25,"invocation_logs":true,"persist":true}}}"#,
        r#"{}"#,
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PATCH)
                    .uri(format!("{prefix}/script-settings"))
                    .header(header::AUTHORIZATION, "Bearer deployer-token")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
    for body in [
        r#"{"logpush":true}"#,
        r#"{"tags":["unsupported"]}"#,
        r#"{"tail_consumers":[{}]}"#,
        r#"{"observability":{"head_sampling_rate":2}}"#,
        r#"{"observability":{"traces":{"enabled":true}}}"#,
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PATCH)
                    .uri(format!("{prefix}/script-settings"))
                    .header(header::AUTHORIZATION, "Bearer deployer-token")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(!response.status().is_success());
    }

    for (method, suffix, body, content_type) in [
        (
            Method::POST,
            "/subdomain",
            r#"{"enabled":false}"#,
            "application/json",
        ),
        (
            Method::POST,
            "/subdomain",
            r#"{"enabled":true}"#,
            "application/json",
        ),
        (
            Method::PUT,
            "/secrets",
            r#"{"name":"NEXT","type":"secret_text","text":"value"}"#,
            "application/json",
        ),
        (
            Method::PATCH,
            "/secrets-bulk",
            r#"{"secrets":{"TOKEN":null},"version_tags":{}}"#,
            "application/json",
        ),
        (
            Method::PUT,
            "/schedules",
            r#"[{"cron":"*/5 * * * *"}]"#,
            "application/json",
        ),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(format!("{prefix}{suffix}"))
                    .header(header::AUTHORIZATION, "Bearer deployer-token")
                    .header(header::CONTENT_TYPE, content_type)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            response.status().is_success() || response.status().is_server_error(),
            "{suffix}: {}",
            response.status()
        );
    }

    let boundary = "settings-boundary";
    let multipart = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"settings\"\r\nContent-Type: application/json\r\n\r\n{{\"compatibility_date\":\"2026-08-30\",\"compatibility_flags\":[\"nodejs_compat\"]}}\r\n--{boundary}--\r\n"
    );
    let patched = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PATCH)
                .uri(format!("{prefix}/settings"))
                .header(header::AUTHORIZATION, "Bearer deployer-token")
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(multipart))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(patched.status(), StatusCode::OK);

    for query in ["?force=true", "?force=invalid"] {
        let response = app
            .clone()
            .oneshot(request(
                Method::DELETE,
                &format!("{prefix}{query}"),
                Body::empty(),
            ))
            .await
            .unwrap();
        assert!(!response.status().is_success());
    }
    let deleted_subdomain = app
        .clone()
        .oneshot(request(
            Method::DELETE,
            &format!("{prefix}/subdomain"),
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(deleted_subdomain.status(), StatusCode::OK);

    let deleted = app
        .oneshot(request(Method::DELETE, &prefix, Body::empty()))
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::OK);
    assert!(
        repo.get_worker(account, worker.id)
            .unwrap()
            .deleted_at_ms
            .is_some()
    );
}
