use super::*;
use crate::workers_http::v4::multipart::{MAX_BODY_BYTES, parse_worker_upload};
use axum::Router;
use axum::extract::{DefaultBodyLimit, FromRequest as _, Multipart};
use axum::http::StatusCode;
use axum::routing::post;
use open_compute_workers::BundleLimits;
use tower::ServiceExt as _;

fn string_part(name: &str, bytes: &[u8]) -> RawPart {
    RawPart {
        name: name.to_owned(),
        file_name: None,
        content_type: None,
        bytes: bytes.to_vec(),
    }
}

#[test]
fn rebuilds_pinned_sdk_fields_and_order_independent_bindings() {
    let mut parts = vec![
        string_part("metadata[main_module]", b"index.js"),
        string_part("metadata[compatibility_date]", b"2026-08-30"),
        string_part("metadata[compatibility_flags][]", b"nodejs_compat"),
        string_part("metadata[annotations][workers/tag]", b"sdk-typed"),
        string_part("metadata[bindings][][type]", b"plain_text"),
        string_part("metadata[bindings][][text]", b""),
        string_part("metadata[bindings][][name]", b"MODE"),
        string_part("metadata[bindings][][text]", b"second"),
        string_part("metadata[bindings][][name]", b"SECOND"),
        string_part("metadata[bindings][][type]", b"plain_text"),
        RawPart {
            name: "files[]".to_owned(),
            file_name: Some("index.js".to_owned()),
            content_type: Some("application/javascript+module".to_owned()),
            bytes: b"export default {}".to_vec(),
        },
    ];
    normalize_parts(&mut parts).unwrap();
    let metadata = parts.iter().find(|part| part.name == "metadata").unwrap();
    let value: Value = serde_json::from_slice(&metadata.bytes).unwrap();
    assert_eq!(value["compatibility_flags"][0], "nodejs_compat");
    assert_eq!(value["annotations"]["workers/tag"], "sdk-typed");
    assert_eq!(value["bindings"][0]["name"], "MODE");
    assert_eq!(value["bindings"][0]["text"], "");
    assert_eq!(value["bindings"][1]["name"], "SECOND");
    assert_eq!(value["bindings"][1]["text"], "second");
    assert_eq!(
        parts.iter().filter(|part| part.name == "index.js").count(),
        1
    );
}

#[test]
fn normalizes_only_the_pinned_sdk_d1_identifier() {
    let mut parts = vec![
        string_part("metadata[bindings][][type]", b"d1"),
        string_part(
            "metadata[bindings][][database_id]",
            b"00000000-0000-7000-8000-000000000001",
        ),
        string_part("metadata[bindings][][name]", b"DB"),
    ];
    normalize_parts(&mut parts).unwrap();
    let metadata = parts.iter().find(|part| part.name == "metadata").unwrap();
    let value: Value = serde_json::from_slice(&metadata.bytes).unwrap();
    assert_eq!(
        value["bindings"][0]["id"],
        "00000000-0000-7000-8000-000000000001"
    );
    assert!(value["bindings"][0].get("database_id").is_none());

    let mut ambiguous = vec![
        string_part("metadata[bindings][][name]", b"DB"),
        string_part("metadata[bindings][][type]", b"d1"),
        string_part("metadata[bindings][][id]", b"wrangler-id"),
        string_part("metadata[bindings][][database_id]", b"sdk-id"),
    ];
    assert!(normalize_parts(&mut ambiguous).is_err());
}

#[test]
fn rejects_unknown_duplicate_and_ambiguous_sdk_fields() {
    for fields in [
        vec![("metadata[unknown]", "x")],
        vec![
            ("metadata[main_module]", "a"),
            ("metadata[main_module]", "b"),
        ],
        vec![("metadata[bindings][][type]", "plain_text")],
        vec![
            ("metadata[bindings][][name]", "X"),
            ("metadata[bindings][][json]", "true"),
        ],
        vec![
            ("metadata[bindings][][name]", "FIRST"),
            ("metadata[bindings][][type]", "service"),
            ("metadata[bindings][][service]", "first"),
            ("metadata[bindings][][entrypoint]", "SecondEntrypoint"),
            ("metadata[bindings][][name]", "SECOND"),
            ("metadata[bindings][][type]", "service"),
            ("metadata[bindings][][service]", "second"),
        ],
        vec![("metadata[migrations][new_tag]", "v1")],
    ] {
        let mut parts = fields
            .into_iter()
            .map(|(name, value)| string_part(name, value.as_bytes()))
            .collect();
        assert!(normalize_parts(&mut parts).is_err());
    }
}

#[tokio::test]
async fn recovers_boundary_split_across_chunks_without_losing_bytes() {
    let boundary = "----open-compute-sdk-boundary";
    let chunks = [
        format!("--{}", &boundary[..8]).into_bytes(),
        format!("{}\r", &boundary[8..]).into_bytes(),
        b"\nContent-Disposition: form-data; name=\"metadata[main_module]\"\r\n\r\nindex.js\r\n"
            .to_vec(),
        format!("--{boundary}--\r\n").into_bytes(),
    ];
    let body = Body::from_stream(stream::iter(
        chunks.into_iter().map(Ok::<_, std::io::Error>),
    ));
    let request = Request::builder()
        .header(header::CONTENT_TYPE, "application/javascript")
        .body(body)
        .unwrap();
    let request = normalize_request(request).await.unwrap();
    assert_eq!(
        request.headers()[header::CONTENT_TYPE],
        format!("multipart/form-data; boundary={boundary}")
    );
    let mut multipart = Multipart::from_request(request, &()).await.unwrap();
    let field = multipart.next_field().await.unwrap().unwrap();
    assert_eq!(field.name(), Some("metadata[main_module]"));
    assert_eq!(field.text().await.unwrap(), "index.js");
}

#[tokio::test]
async fn bounds_standard_boundaries_and_rejects_duplicate_content_types() {
    let quoted = Request::builder()
        .header(
            header::CONTENT_TYPE,
            "multipart/form-data; charset=utf-8; boundary=\"quoted-boundary\"",
        )
        .body(Body::empty())
        .unwrap();
    assert!(normalize_request(quoted).await.is_ok());

    let oversized = Request::builder()
        .header(
            header::CONTENT_TYPE,
            format!(
                "multipart/form-data; boundary={}",
                "b".repeat(MAX_BOUNDARY_BYTES + 1)
            ),
        )
        .body(Body::empty())
        .unwrap();
    assert!(normalize_request(oversized).await.is_err());

    let mut duplicate = Request::builder().body(Body::empty()).unwrap();
    duplicate.headers_mut().append(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/javascript"),
    );
    duplicate.headers_mut().append(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/javascript"),
    );
    assert!(normalize_request(duplicate).await.is_err());
}

async fn bounded_upload(request: Request) -> StatusCode {
    let Ok(request) = normalize_request(request).await else {
        return StatusCode::BAD_REQUEST;
    };
    let Ok(multipart) = Multipart::from_request(request, &()).await else {
        return StatusCode::BAD_REQUEST;
    };
    match parse_worker_upload(multipart, BundleLimits::default()).await {
        Ok(_) => StatusCode::NO_CONTENT,
        Err(_) => StatusCode::BAD_REQUEST,
    }
}

fn multipart_body(boundary: &str, module: &[u8]) -> Vec<u8> {
    let mut body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"metadata\"\r\nContent-Type: application/json\r\n\r\n{{\"main_module\":\"index.js\",\"compatibility_date\":\"2026-08-30\"}}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"index.js\"; filename=\"index.js\"\r\nContent-Type: application/javascript+module\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(module);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

fn append_string_field(body: &mut Vec<u8>, boundary: &str, name: &str, value: &str) {
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
        )
        .as_bytes(),
    );
}

fn append_module(body: &mut Vec<u8>, boundary: &str, name: &str, bytes: &[u8]) {
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"files[]\"; filename=\"{name}\"\r\nContent-Type: application/javascript+module\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(b"\r\n");
}

fn near_limit_sdk_body(boundary: &str) -> Vec<u8> {
    let mut body = Vec::new();
    append_string_field(&mut body, boundary, "metadata[main_module]", "index.js");
    append_string_field(
        &mut body,
        boundary,
        "metadata[compatibility_date]",
        "2026-08-30",
    );
    append_string_field(&mut body, boundary, "metadata[bindings][][name]", "TARGET");
    append_string_field(&mut body, boundary, "metadata[bindings][][type]", "service");
    append_string_field(
        &mut body,
        boundary,
        "metadata[bindings][][service]",
        "target",
    );
    let segment = "p".repeat(1_000);
    for leaf in 0..(super::super::multipart::MAX_SDK_METADATA_FIELDS - 5) {
        let name = format!(
            "metadata[bindings][][props][{segment}][{segment}][{segment}][q{}][leaf{leaf}]",
            "q".repeat(850),
        );
        append_string_field(&mut body, boundary, &name, "x");
    }
    for name in ["index.js", "one.js", "two.js", "three.js"] {
        append_module(
            &mut body,
            boundary,
            name,
            &vec![b' '; BundleLimits::DEFAULT.max_module_bytes],
        );
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    body
}

#[tokio::test]
async fn explicit_worker_limit_accepts_more_than_axum_default() {
    let boundary = "open-compute-large-worker";
    let mut module = vec![b' '; 2 * 1024 * 1024 + 1];
    module.extend_from_slice(b"\nexport default {};");
    let app = Router::new()
        .route("/", post(bounded_upload))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES));
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(multipart_body(boundary, &module)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn derived_worker_wire_limit_accepts_the_bounded_sdk_shape() {
    let boundary = "open-compute-near-limit-sdk";
    let body = near_limit_sdk_body(boundary);
    assert!(body.len() > 23 * 1024 * 1024);
    assert!(body.len() <= MAX_BODY_BYTES);
    assert!(MAX_BODY_BYTES - body.len() < 4 * 1024 * 1024);
    let app = Router::new()
        .route("/", post(bounded_upload))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES));
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .header(header::CONTENT_TYPE, "application/javascript")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn explicit_worker_limit_rejects_chunked_wire_overhead() {
    let boundary = "open-compute-over-limit";
    let chunks = [
        format!("--{boundary}\r\n").into_bytes(),
        vec![b'x'; MAX_BODY_BYTES],
        format!("\r\n--{boundary}--\r\n").into_bytes(),
    ];
    let body = Body::from_stream(stream::iter(
        chunks.into_iter().map(Ok::<_, std::io::Error>),
    ));
    let app = Router::new()
        .route("/", post(bounded_upload))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES));
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(body)
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
