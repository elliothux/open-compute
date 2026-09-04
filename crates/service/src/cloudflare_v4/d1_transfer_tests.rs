use super::*;
use axum::body::Body;
use axum::body::to_bytes;
use axum::http::{Method, Request};
use md5::Digest as _;
use open_compute_artifacts::{
    ArtifactStore, MapEnv, S3ArtifactClient, resolve_s3_credentials_with,
};
use open_compute_core::{D1Config, PlatformConfig, PlatformId, SecretString};
use open_compute_workers::{
    CreateResourceOutcome, CreateResourceRequest, D1ResourceDriver, ResourceController,
    ResourcePins,
};
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt as _;

fn context(role: super::super::V4Role) -> V4RequestContext {
    V4RequestContext {
        role,
        request_id: RequestId::generate(),
    }
}

#[test]
fn query_and_scalar_parsers_enforce_the_wire_contract() {
    assert_eq!(
        restore_query(Some("bookmark=abc%2Fdef")),
        Ok(D1TimeTravelTarget::Bookmark("abc/def".to_owned()))
    );
    assert!(matches!(
        restore_query(Some("timestamp=2024-01-02T03%3A04%3A05Z")),
        Ok(D1TimeTravelTarget::TimestampMs(_))
    ));
    for query in [
        None,
        Some("bookmark="),
        Some("bookmark=a&timestamp=2024-01-02T03%3A04%3A05Z"),
        Some("bookmark=a&extra=b"),
        Some("bookmark=a&bookmark=b"),
        Some("timestamp=invalid"),
    ] {
        assert!(restore_query(query).is_err(), "query {query:?}");
    }

    assert_eq!(one_query(None, "cursor", false), Ok(None));
    assert_eq!(
        one_query(Some("cursor=next%20page"), "cursor", true),
        Ok(Some("next page".to_owned()))
    );
    for query in [
        None,
        Some("cursor="),
        Some("other=value"),
        Some("cursor=a&other=value"),
        Some("cursor=a&cursor=b"),
    ] {
        assert!(one_query(query, "cursor", true).is_err(), "query {query:?}");
    }

    assert_eq!(parse_query(None).unwrap(), BTreeMap::new());
    assert_eq!(parse_timestamp_ms("1970-01-01T00:00:01Z"), Ok(1_000));
    assert!(parse_timestamp_ms("yesterday").is_err());
    assert_eq!(
        parse_md5("000102030405060708090a0b0c0d0e0f"),
        Ok([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15])
    );
    assert!(parse_md5("short").is_err());
    assert!(parse_md5("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz").is_err());
}

#[test]
fn table_and_header_validation_rejects_ambiguous_input() {
    assert_eq!(
        validate_tables(vec!["users".to_owned(), "orders".to_owned()])
            .unwrap()
            .len(),
        2
    );
    assert!(validate_tables(vec![String::new()]).is_err());
    assert!(validate_tables(vec!["x".repeat(256)]).is_err());
    assert!(validate_tables(vec!["users".to_owned(), "users".to_owned()]).is_err());

    let mut headers = HeaderMap::new();
    assert_eq!(one_header(&headers, "x-test", false), Ok(None));
    assert!(one_header(&headers, "x-test", true).is_err());
    headers.insert("x-test", HeaderValue::from_static("value"));
    assert_eq!(one_header(&headers, "x-test", true), Ok(Some("value")));
    headers.append("x-test", HeaderValue::from_static("second"));
    assert!(one_header(&headers, "x-test", true).is_err());

    let mut invalid = HeaderMap::new();
    invalid.insert("x-test", HeaderValue::from_bytes(b"\xff").unwrap());
    assert!(one_header(&invalid, "x-test", true).is_err());

    let mut host = HeaderMap::new();
    host.insert(header::HOST, HeaderValue::from_static("localhost:8787"));
    assert_eq!(request_host(&host).unwrap().as_str(), "localhost:8787");
    host.append(header::HOST, HeaderValue::from_static("example.com"));
    assert!(request_host(&host).is_err());
    assert!(request_host(&HeaderMap::new()).is_err());
}

#[test]
fn local_error_mapping_and_request_ids_are_stable() {
    let cases = [
        (
            ErrorCode::ArtifactIntegrityError,
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            ErrorCode::D1DatabaseCorrupt,
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            ErrorCode::D1IdentityMismatch,
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (ErrorCode::D1LimitError, StatusCode::BAD_REQUEST),
        (ErrorCode::LimitInvalid, StatusCode::BAD_REQUEST),
        (ErrorCode::D1Overloaded, StatusCode::TOO_MANY_REQUESTS),
        (ErrorCode::AdmissionBusy, StatusCode::TOO_MANY_REQUESTS),
        (ErrorCode::D1Timeout, StatusCode::SERVICE_UNAVAILABLE),
        (ErrorCode::D1ResultUnknown, StatusCode::SERVICE_UNAVAILABLE),
        (
            ErrorCode::ResourceUnavailable,
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (ErrorCode::IdempotencyConflict, StatusCode::CONFLICT),
        (ErrorCode::ResourceInvariantViolation, StatusCode::CONFLICT),
        (ErrorCode::ResourceNotFound, StatusCode::NOT_FOUND),
        (ErrorCode::D1SqlInvalid, StatusCode::BAD_REQUEST),
        (ErrorCode::D1AuthorizerDenied, StatusCode::BAD_REQUEST),
        (ErrorCode::ConfigInvalid, StatusCode::BAD_REQUEST),
    ];
    for (code, expected) in cases {
        let error = PlatformError::new(code, "test error");
        assert_eq!(platform_status(&error), expected, "{code:?}");
    }

    let direct = [
        (V4Error::AuthenticationRequired, StatusCode::UNAUTHORIZED),
        (V4Error::PermissionDenied, StatusCode::FORBIDDEN),
        (V4Error::InvalidRequest, StatusCode::BAD_REQUEST),
        (V4Error::InvalidField("/field"), StatusCode::BAD_REQUEST),
        (V4Error::NotFound, StatusCode::NOT_FOUND),
        (V4Error::Unavailable, StatusCode::SERVICE_UNAVAILABLE),
        (V4Error::Conflict, StatusCode::CONFLICT),
        (V4Error::IntegrityFailure, StatusCode::UNPROCESSABLE_ENTITY),
        (V4Error::Unsupported, StatusCode::NOT_IMPLEMENTED),
        (V4Error::RateLimited, StatusCode::TOO_MANY_REQUESTS),
        (V4Error::Internal, StatusCode::INTERNAL_SERVER_ERROR),
        (
            V4Error::Official(super::super::V4OfficialError::WorkerNotFound),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    ];
    for (error, expected) in direct {
        assert_eq!(error_status(error), expected);
    }

    let supplied = RequestId::generate();
    let mut request = Request::new(Body::empty());
    request.extensions_mut().insert(supplied);
    assert_eq!(request_id(&request), supplied);
    let generated = request_id(&Request::new(Body::empty()));
    assert!(!generated.to_string().is_empty());

    let response = raw_error(StatusCode::BAD_GATEWAY, supplied);
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(
        response.headers()[REQUEST_ID_HEADER].to_str().unwrap(),
        supplied.to_string()
    );
    let mapped = platform_error(
        &PlatformError::new(ErrorCode::D1Timeout, "test error"),
        context(super::super::V4Role::Admin),
    );
    assert_eq!(mapped.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[test]
fn authentication_context_enforces_each_role() {
    for (role, permission, allowed) in [
        (super::super::V4Role::Admin, V4Permission::Maintenance, true),
        (
            super::super::V4Role::Deployer,
            V4Permission::ProductWrite,
            true,
        ),
        (
            super::super::V4Role::Deployer,
            V4Permission::Maintenance,
            false,
        ),
        (super::super::V4Role::ReadOnly, V4Permission::Read, true),
        (
            super::super::V4Role::ReadOnly,
            V4Permission::ProductWrite,
            false,
        ),
    ] {
        let mut request = Request::new(Body::empty());
        request.extensions_mut().insert(context(role));
        let result = authenticated_context(&request, permission);
        if allowed {
            assert_eq!(result.unwrap().role(), role);
        } else {
            assert_eq!(
                result.unwrap_err().into_response().status(),
                StatusCode::FORBIDDEN
            );
        }
    }
    assert_eq!(
        authenticated_context(&Request::new(Body::empty()), V4Permission::Read)
            .unwrap_err()
            .into_response()
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn body_decoders_accept_only_canonical_shapes() {
    let context = context(super::super::V4Role::Admin);
    let valid = Request::builder()
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .body(Body::from(r#"{"value":1}"#))
        .unwrap();
    let value: serde_json::Value = json_body(valid, context).await.unwrap();
    assert_eq!(value["value"], 1);

    for request in [
        Request::new(Body::from(r#"{"value":1}"#)),
        Request::builder()
            .header(header::CONTENT_TYPE, "text/plain")
            .body(Body::from(r#"{"value":1}"#))
            .unwrap(),
        Request::builder()
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("not json"))
            .unwrap(),
        Request::builder()
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("x".repeat(MAX_JSON_BODY + 1)))
            .unwrap(),
    ] {
        assert_eq!(
            json_body::<serde_json::Value>(request, context)
                .await
                .unwrap_err()
                .into_response()
                .status(),
            StatusCode::BAD_REQUEST
        );
    }

    assert!(bodyless(Request::new(Body::empty()), context).await.is_ok());
    let with_type = Request::builder()
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        bodyless(with_type, context)
            .await
            .unwrap_err()
            .into_response()
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        bodyless(Request::new(Body::from("x")), context)
            .await
            .unwrap_err()
            .into_response()
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        bodyless(Request::new(Body::from("xx")), context)
            .await
            .unwrap_err()
            .into_response()
            .status(),
        StatusCode::BAD_REQUEST
    );
}

#[test]
fn signed_transfer_urls_escape_capabilities() {
    let host: Authority = "localhost:8787".parse().unwrap();
    assert_eq!(
        signed_url(&host, "/transfer/download", "a b/+?"),
        "http://localhost:8787/client/v4/transfer/download?token=a+b%2F%2B%3F"
    );
}

fn artifact_store(mock: &open_compute_artifacts::MockS3) -> ArtifactStore {
    let config = PlatformConfig::from_toml_str(&format!(
        r#"
[s3]
endpoint = "{}"
bucket = "open-compute"
prefix = "system/"
connect_timeout_ms = 100
request_timeout_ms = 1000
"#,
        mock.endpoint
    ))
    .unwrap()
    .s3;
    let env = MapEnv::new()
        .with("S3_ACCESS_KEY_ID", "test-access")
        .with("S3_SECRET_ACCESS_KEY", "test-secret");
    let credentials = resolve_s3_credentials_with(&config, &env).unwrap();
    ArtifactStore::new(S3ArtifactClient::connect(&config, &credentials, 1024 * 1024).unwrap())
}

fn create_database(
    storage: &Arc<open_compute_storage::PlatformStorage>,
    account: AccountId,
    name: &str,
    now_ms: i64,
) -> ResourceId {
    match ResourceController::new(
        storage.as_ref(),
        ResourcePins::new(),
        D1ResourceDriver::new(storage.as_ref(), 256 * 1024 * 1024),
    )
    .create(&CreateResourceRequest {
        account_id: account,
        kind: BindingKind::D1Database,
        name: name.to_owned(),
        idempotency_key: name.to_owned(),
        driver_schema_version: open_compute_storage::D1_DATABASE_SCHEMA_VERSION,
        request_id: RequestId::generate(),
        now_ms,
    })
    .unwrap()
    {
        CreateResourceOutcome::Applied(value) => value.resource_id,
        CreateResourceOutcome::Replay(_) => unreachable!(),
    }
}

async fn response_json(response: Response) -> serde_json::Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), 4 * 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap()
}

fn transfer_request(method: Method, uri: &str, body: Body) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, "Bearer deployer-token")
        .header(header::HOST, "localhost:8787")
        .header(header::CONTENT_TYPE, "application/json")
        .body(body)
        .unwrap()
}

fn local_uri(absolute: &str) -> String {
    let url = url::Url::parse(absolute).unwrap();
    match url.query() {
        Some(query) => format!("{}?{query}", url.path()),
        None => url.path().to_owned(),
    }
}

#[tokio::test]
async fn transfer_routes_round_trip_export_import_and_time_travel() {
    let (_temp, mock, state, account, storage) =
        crate::tests::initialized_worker_http_fixture().await;
    let source = create_database(&storage, account, "transfer-source-http", 10);
    let destination = create_database(&storage, account, "transfer-destination-http", 11);
    let backend = Arc::new(crate::D1BindingService::new(
        storage.clone(),
        ResourcePins::new(),
        D1Config::default(),
    ));
    let sql = "CREATE TABLE notes(id INTEGER PRIMARY KEY, body TEXT NOT NULL);\
               INSERT INTO notes VALUES (1, 'before')";
    backend
        .apply_migrations(
            account,
            source,
            vec![open_compute_storage::D1Migration {
                id: 1,
                name: "0001_notes.sql".to_owned(),
                sql: sql.to_owned(),
                sha256: sha2::Sha256::digest(sql.as_bytes()).into(),
            }],
            20,
        )
        .await
        .unwrap();
    backend.user_version(account, destination).await.unwrap();
    let api = crate::D1ApiState::new(
        storage.clone(),
        artifact_store(&mock),
        ResourcePins::new(),
        backend.clone(),
        D1Config::default(),
        100,
        Duration::from_millis(10),
    );
    let authority =
        super::super::accounts::AccountAuthority::new(PlatformId::generate(), account, 1);
    let public_account = authority.public_id().to_owned();
    let public_source = authority.public_resource_id(V4ResourceKind::D1Database, source);
    let public_destination = authority.public_resource_id(V4ResourceKind::D1Database, destination);
    let app = crate::http::admin_router(
        state
            .with_d1_api(api)
            .with_platform_storage(storage.clone())
            .with_v4_tokens(
                SecretString::new("deployer-token"),
                SecretString::new("read-token"),
            )
            .with_cloudflare_v4_account(authority),
    );
    let source_prefix = format!("/client/v4/accounts/{public_account}/d1/database/{public_source}");
    let destination_prefix =
        format!("/client/v4/accounts/{public_account}/d1/database/{public_destination}");

    let listed = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/client/v4/accounts/{public_account}/d1/database?page=1&per_page=1&name=transfer"
                ))
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    assert_eq!(response_json(listed).await["result_info"]["count"], 1);
    let fetched = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("{source_prefix}?fields=uuid,name,read_replication"))
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(fetched.status(), StatusCode::OK);
    assert_eq!(
        response_json(fetched).await["result"]["name"],
        "transfer-source-http"
    );
    let updated = app
        .clone()
        .oneshot(transfer_request(
            Method::PATCH,
            &source_prefix,
            Body::from(r#"{"read_replication":{"mode":"disabled"}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(updated.status(), StatusCode::OK);
    for body in [
        r#"{}"#,
        r#"{"read_replication":{"mode":"invalid"}}"#,
        r#"{"read_replication":{"mode":"auto"}}"#,
    ] {
        let response = app
            .clone()
            .oneshot(transfer_request(
                Method::PUT,
                &source_prefix,
                Body::from(body),
            ))
            .await
            .unwrap();
        assert!(!response.status().is_success());
    }
    for (suffix, body) in [
        ("query", r#"{"sql":"SELECT ? AS value","params":["text"]}"#),
        (
            "raw",
            r#"{"batch":[{"sql":"SELECT 1 AS integer_value, 1.5 AS real_value, NULL AS null_value, X'0102' AS blob_value"}]}"#,
        ),
    ] {
        let response = app
            .clone()
            .oneshot(transfer_request(
                Method::POST,
                &format!("{source_prefix}/{suffix}"),
                Body::from(body),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "D1 {suffix}");
        assert_eq!(response_json(response).await["success"], true);
    }
    let created_database = app
        .clone()
        .oneshot(transfer_request(
            Method::POST,
            &format!("/client/v4/accounts/{public_account}/d1/database"),
            Body::from(r#"{"name":"created-via-http","read_replication":{"mode":"disabled"}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(created_database.status(), StatusCode::OK);
    let created_database = response_json(created_database).await["result"]["uuid"]
        .as_str()
        .unwrap()
        .to_owned();

    let exported = app
        .clone()
        .oneshot(transfer_request(
            Method::POST,
            &format!("{source_prefix}/export"),
            Body::from(r#"{"output_format":"polling","dump_options":{}}"#),
        ))
        .await
        .unwrap();
    assert_eq!(exported.status(), StatusCode::OK);
    let exported = response_json(exported).await;
    let export_id = exported["result"]["at_bookmark"]
        .as_str()
        .unwrap()
        .to_owned();
    let download_url = exported["result"]["result"]["signed_url"]
        .as_str()
        .unwrap()
        .to_owned();
    let downloaded = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(local_uri(&download_url))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(downloaded.status(), StatusCode::OK);
    let export_sql = to_bytes(downloaded.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    assert!(!export_sql.is_empty());
    let etag = hex::encode(md5::Md5::digest(&export_sql));

    let polled_export = app
        .clone()
        .oneshot(transfer_request(
            Method::POST,
            &format!("{source_prefix}/export"),
            Body::from(format!(
                r#"{{"output_format":"polling","current_bookmark":"{export_id}"}}"#
            )),
        ))
        .await
        .unwrap();
    assert_eq!(polled_export.status(), StatusCode::OK);

    let initialized = app
        .clone()
        .oneshot(transfer_request(
            Method::POST,
            &format!("{destination_prefix}/import"),
            Body::from(format!(r#"{{"action":"init","etag":"{etag}"}}"#)),
        ))
        .await
        .unwrap();
    assert_eq!(initialized.status(), StatusCode::OK);
    let initialized = response_json(initialized).await;
    let transfer_id = initialized["result"]["at_bookmark"]
        .as_str()
        .unwrap()
        .to_owned();
    let filename = initialized["result"]["filename"]
        .as_str()
        .unwrap()
        .to_owned();
    let upload_url = initialized["result"]["upload_url"]
        .as_str()
        .unwrap()
        .to_owned();
    let uploaded = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(local_uri(&upload_url))
                .body(Body::from(export_sql.clone()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(uploaded.status(), StatusCode::OK);
    assert!(uploaded.headers().contains_key(header::ETAG));

    let ingested = app
        .clone()
        .oneshot(transfer_request(
            Method::POST,
            &format!("{destination_prefix}/import"),
            Body::from(format!(
                r#"{{"action":"ingest","filename":"{filename}","etag":"{etag}"}}"#
            )),
        ))
        .await
        .unwrap();
    assert_eq!(ingested.status(), StatusCode::OK);
    assert_eq!(
        response_json(ingested).await["result"]["status"],
        "complete"
    );

    let polled_import = app
        .clone()
        .oneshot(transfer_request(
            Method::POST,
            &format!("{destination_prefix}/import"),
            Body::from(format!(
                r#"{{"action":"poll","current_bookmark":"{transfer_id}"}}"#
            )),
        ))
        .await
        .unwrap();
    assert_eq!(polled_import.status(), StatusCode::OK);

    let bookmark = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("{source_prefix}/time_travel/bookmark"))
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bookmark.status(), StatusCode::OK);
    let bookmark = response_json(bookmark).await["result"]["bookmark"]
        .as_str()
        .unwrap()
        .to_owned();
    backend
        .operator_query(
            account,
            source,
            "INSERT INTO notes VALUES (2, 'after')".to_owned(),
        )
        .await
        .unwrap();
    let restored = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "{source_prefix}/time_travel/restore?bookmark={}",
                    form_urlencoded::byte_serialize(bookmark.as_bytes()).collect::<String>()
                ))
                .header(header::AUTHORIZATION, "Bearer deployer-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(restored.status(), StatusCode::OK);

    let unauthorized = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/client/v4/accounts/{public_account}/d1/database/{public_source}/transfer/{export_id}/download?token=wrong"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::NOT_FOUND);

    let backups = format!(
        "/client/v4/accounts/{public_account}/open-compute/d1/databases/{public_source}/backups"
    );
    let backup = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(&backups)
                .header(header::AUTHORIZATION, "Bearer admin-token")
                .header("idempotency-key", "d1-backup")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(backup.status(), StatusCode::OK);
    let backup_id = response_json(backup).await["result"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let listed_backups = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&backups)
                .header(header::AUTHORIZATION, "Bearer read-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(listed_backups.status(), StatusCode::OK);
    assert_eq!(
        response_json(listed_backups).await["result"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let restored_backup = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/client/v4/accounts/{public_account}/open-compute/d1/backups/{backup_id}/restore"
                ))
                .header(header::AUTHORIZATION, "Bearer admin-token")
                .header(header::CONTENT_TYPE, "application/json")
                .header("idempotency-key", "d1-restore")
                .body(Body::from(r#"{"name":"restored-d1"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(restored_backup.status(), StatusCode::OK);
    assert_eq!(
        response_json(restored_backup).await["result"]["name"],
        "restored-d1"
    );

    let deleted_database = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri(format!(
                    "/client/v4/accounts/{public_account}/d1/database/{created_database}"
                ))
                .header(header::AUTHORIZATION, "Bearer deployer-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(deleted_database.status(), StatusCode::OK);

    for body in [Body::from("x"), Body::empty()] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(&backups)
                    .header(header::AUTHORIZATION, "Bearer admin-token")
                    .header("idempotency-key", "has whitespace")
                    .body(body)
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
