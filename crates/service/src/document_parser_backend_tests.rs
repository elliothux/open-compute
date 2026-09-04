use super::*;
use crate::p3_3_test_support::RuntimeFeatureFixture;
use axum::body::Body;
use axum::http::HeaderMap;
use open_compute_document_parser::{
    DocumentErrorCode, DocumentFormat, DocumentMetadata, ParseOutput, ParseSuccess,
    encode_output_frame,
};
use open_compute_workers::{VersionAiInput, VersionRuntimeFeatures};
use std::os::unix::fs::PermissionsExt as _;

async fn fixture() -> (RuntimeFeatureFixture, DocumentParserBindingService) {
    let fixture = RuntimeFeatureFixture::create(VersionRuntimeFeatures {
        ai: Some(VersionAiInput {
            binding: "AI".to_owned(),
        }),
        ..VersionRuntimeFeatures::default()
    })
    .await;
    let service = DocumentParserBindingService::with_executable(
        fixture.storage.clone(),
        DocumentParserConfig::default(),
        PathBuf::from("/usr/bin/false"),
    );
    (fixture, service)
}

fn request(fixture: &RuntimeFeatureFixture, method: Method, path: &str, body: Body) -> Request {
    Request::builder()
        .method(method)
        .uri(path)
        .header(ACCOUNT_HEADER, fixture.account.to_string())
        .header(WORKER_HEADER, fixture.worker.to_string())
        .header(VERSION_HEADER, fixture.version.to_string())
        .header(
            DESCRIPTOR_HEADER,
            fixture.ai_descriptor_sha256.as_deref().unwrap(),
        )
        .body(body)
        .unwrap()
}

async fn response_json(response: Response) -> serde_json::Value {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn supported_reports_only_admitted_formats_in_deterministic_order() {
    let (fixture, service) = fixture().await;
    let response = service
        .handle(request(
            &fixture,
            Method::GET,
            "/internal/ai/to-markdown/v1/supported",
            Body::empty(),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let value = response_json(response).await;
    assert_eq!(value["schemaVersion"], 1);
    let formats = value["result"].as_array().unwrap();
    assert_eq!(formats.len(), 13);
    assert_eq!(formats[0]["extension"], ".csv");
    assert!(formats.iter().any(|item| item["extension"] == ".pdf"));
    assert!(!formats.iter().any(|item| item["extension"] == ".xlsb"));
    assert!(!formats.iter().any(|item| item["extension"] == ".numbers"));
}

#[tokio::test]
async fn legal_html_options_are_admitted_and_forged_authority_fails_closed() {
    let (fixture, service) = fixture().await;
    let response = service
        .handle(request(
            &fixture,
            Method::POST,
            "/internal/ai/to-markdown/v1/transform",
            Body::from(
                serde_json::to_vec(&serde_json::json!({
                    "schemaVersion": 1,
                    "files": [],
                    "options": {"html": {"hostname": "example.com/base/", "cssSelector": "main"}}
                }))
                .unwrap(),
            ),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);

    let response = service
        .handle(request(
            &fixture,
            Method::POST,
            "/internal/ai/to-markdown/v1/transform",
            Body::from(
                serde_json::to_vec(&serde_json::json!({
                    "schemaVersion": 1,
                    "files": [],
                    "options": {"html": {"cssSelector": "@import url(x)"}}
                }))
                .unwrap(),
            ),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response.headers()[ERROR_HEADER],
        ErrorCode::DocumentOptionUnsupported.as_str()
    );

    let mut forged = request(
        &fixture,
        Method::GET,
        "/internal/ai/to-markdown/v1/supported",
        Body::empty(),
    );
    forged.headers_mut().insert(
        HeaderName::from_static(DESCRIPTOR_HEADER),
        HeaderValue::from_static("00"),
    );
    let response = service.handle(forged).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response.headers()[ERROR_HEADER],
        ErrorCode::DocumentProtocolError.as_str()
    );
}

#[tokio::test]
async fn child_failure_is_a_per_document_error_and_keeps_the_service_live() {
    let (fixture, service) = fixture().await;
    let payload = serde_json::json!({
        "schemaVersion": 1,
        "files": [{
            "name": "fixture.txt",
            "mimeType": "text/plain",
            "dataBase64": base64::engine::general_purpose::STANDARD.encode("fixture text")
        }],
        "options": {}
    });
    let response = service
        .handle(request(
            &fixture,
            Method::POST,
            "/internal/ai/to-markdown/v1/transform",
            Body::from(serde_json::to_vec(&payload).unwrap()),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let value = response_json(response).await;
    assert_eq!(value["result"][0]["format"], "error");
    assert_eq!(value["result"][0]["error"], "DOCUMENT_UNAVAILABLE");

    let response = service
        .handle(request(
            &fixture,
            Method::GET,
            "/internal/ai/to-markdown/v1/supported",
            Body::empty(),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[test]
fn markdown_postprocessing_is_bounded_and_deterministic() {
    assert_eq!(
        markdown_to_text("# Heading\n\n- item\n\n`code`"),
        "Heading\nitem\ncode"
    );
    assert_eq!(estimate_tokens("alpha beta 世界"), 5);
    assert_eq!(yaml_scalar("a: b"), "\"a: b\"");
    assert!(validate_logical_name("../secret.txt").is_err());
    assert!(validate_logical_name("line\nbreak").is_err());
    assert!(validate_mime("text/plain").is_ok());
    assert!(validate_mime("text").is_err());
    assert!(validate_mime("text//plain").is_err());
    assert!(validate_mime("text/plain; charset=utf-8").is_err());
}

#[test]
fn protocol_helpers_cover_metadata_text_tokens_headers_and_error_mapping() {
    let metadata = DocumentMetadata {
        title: Some("Title: quoted".to_string()),
        authors: Some(vec!["Ada".to_string(), "Grace".to_string()]),
        subject: Some("Search".to_string()),
        language: Some("en".to_string()),
    };
    let rendered = markdown_with_metadata(&metadata, "# Body");
    assert!(rendered.starts_with("---\ntitle: \"Title: quoted\""));
    assert!(rendered.contains("authors: \"Ada, Grace\""));
    assert!(rendered.contains("subject: \"Search\""));
    assert!(rendered.contains("language: \"en\""));
    assert_eq!(
        markdown_with_metadata(&DocumentMetadata::default(), "body"),
        "body"
    );
    assert_eq!(
        markdown_to_text(
            "## Heading\n> quote\n\n[link](https://example.com)\n```rust\nlet_value = **1**;\n```"
        ),
        "Heading\nquote\nlink\nletvalue = 1;"
    );
    assert_eq!(estimate_tokens("abcdefgh ! 世"), 4);
    assert_eq!(estimate_tokens("tail"), 1);

    for name in [
        "",
        ".",
        "..",
        "a/b",
        "a\\b",
        "line\nbreak",
        &"x".repeat(256),
    ] {
        assert_eq!(
            validate_logical_name(name).unwrap_err().code(),
            ErrorCode::DocumentInputInvalid,
            "{name:?}"
        );
    }
    for mime in [
        "text/plain",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "x-test/a+b.c_d-$!#&^",
    ] {
        validate_mime(mime).unwrap();
    }
    for mime in [
        "",
        "/plain",
        "text/",
        "Text/plain",
        "text/pl@in",
        "text/plain/extra",
        &format!("text/{}", "x".repeat(129)),
    ] {
        assert_eq!(
            validate_mime(mime).unwrap_err().code(),
            ErrorCode::DocumentInputInvalid,
            "{mime:?}"
        );
    }

    for (source, expected) in [
        (
            DocumentErrorCode::InvalidFrame,
            ErrorCode::DocumentProtocolError,
        ),
        (
            DocumentErrorCode::ContentDigestMismatch,
            ErrorCode::DocumentProtocolError,
        ),
        (
            DocumentErrorCode::ParserContractMismatch,
            ErrorCode::DocumentProtocolError,
        ),
        (
            DocumentErrorCode::InvalidRequest,
            ErrorCode::DocumentInputInvalid,
        ),
        (
            DocumentErrorCode::ContentTypeMismatch,
            ErrorCode::DocumentInputInvalid,
        ),
        (
            DocumentErrorCode::DocumentInvalid,
            ErrorCode::DocumentInputInvalid,
        ),
        (
            DocumentErrorCode::DocumentLimitExceeded,
            ErrorCode::DocumentLimitExceeded,
        ),
        (
            DocumentErrorCode::UnsupportedContentType,
            ErrorCode::DocumentFormatUnsupported,
        ),
        (
            DocumentErrorCode::DocumentEncrypted,
            ErrorCode::DocumentEncrypted,
        ),
        (DocumentErrorCode::DocumentEmpty, ErrorCode::DocumentEmpty),
        (
            DocumentErrorCode::DocumentOcrRequired,
            ErrorCode::DocumentOcrRequired,
        ),
        (
            DocumentErrorCode::DocumentParseFailed,
            ErrorCode::DocumentParseFailed,
        ),
    ] {
        assert_eq!(map_document_code(source), expected);
    }
    assert_eq!(
        map_parser_protocol(open_compute_document_parser::decode_input_frame(b"bad").unwrap_err()),
        ErrorCode::DocumentProtocolError
    );

    let mut headers = HeaderMap::new();
    headers.insert("x-test", HeaderValue::from_static("42"));
    assert_eq!(text_header(&headers, "x-test").unwrap(), "42");
    assert_eq!(parse_header::<u32>(&headers, "x-test").unwrap(), 42);
    headers.insert("x-test", HeaderValue::from_static("not-a-number"));
    assert_eq!(
        parse_header::<u32>(&headers, "x-test").unwrap_err().code(),
        ErrorCode::DocumentProtocolError
    );
    for value in [None, Some(""), Some(&"x".repeat(257))] {
        let mut headers = HeaderMap::new();
        if let Some(value) = value {
            headers.insert("x-test", HeaderValue::from_str(value).unwrap());
        }
        assert_eq!(
            text_header(&headers, "x-test").unwrap_err().code(),
            ErrorCode::DocumentProtocolError
        );
    }

    for (error, status) in [
        (limit(), StatusCode::PAYLOAD_TOO_LARGE),
        (timeout(), StatusCode::GATEWAY_TIMEOUT),
        (unavailable(), StatusCode::SERVICE_UNAVAILABLE),
        (protocol(), StatusCode::BAD_REQUEST),
        (input(), StatusCode::BAD_REQUEST),
        (option_unsupported(), StatusCode::BAD_REQUEST),
    ] {
        let response = document_error(&error);
        assert_eq!(response.status(), status);
        assert_eq!(response.headers()[ERROR_HEADER], error.code().as_str());
    }
}

#[test]
fn conversion_options_only_forward_html_settings_for_html_documents() {
    let options: ConversionOptions = serde_json::from_value(serde_json::json!({
        "html": {"hostname": "example.com/base/", "cssSelector": "main"},
        "output": {"format": "text"},
        "pdf": {"metadata": false}
    }))
    .unwrap();
    options.validate().unwrap();
    let html = options.html_options("text/html").unwrap();
    assert_eq!(html.hostname.as_deref(), Some("example.com/base/"));
    assert_eq!(html.css_selector.as_deref(), Some("main"));
    assert!(options.html_options("text/plain").is_none());
}

#[tokio::test]
async fn transform_rejects_malformed_and_bounded_payloads_before_child_spawn() {
    let (fixture, base_service) = fixture().await;
    assert!(format!("{base_service:?}").contains("DocumentParserBindingService"));
    DocumentParserBindingService::new(fixture.storage.clone(), DocumentParserConfig::default())
        .unwrap();

    for (body, expected) in [
        (b"not-json".to_vec(), ErrorCode::DocumentProtocolError),
        (
            serde_json::to_vec(&serde_json::json!({"schemaVersion": 2, "files": []})).unwrap(),
            ErrorCode::DocumentLimitExceeded,
        ),
        (
            serde_json::to_vec(&serde_json::json!({
                "schemaVersion": 1,
                "files": [{"name": "../bad", "mimeType": "text/plain", "dataBase64": "eA=="}]
            }))
            .unwrap(),
            ErrorCode::DocumentInputInvalid,
        ),
        (
            serde_json::to_vec(&serde_json::json!({
                "schemaVersion": 1,
                "files": [{"name": "good.txt", "mimeType": "Text/plain", "dataBase64": "eA=="}]
            }))
            .unwrap(),
            ErrorCode::DocumentInputInvalid,
        ),
        (
            serde_json::to_vec(&serde_json::json!({
                "schemaVersion": 1,
                "files": [{"name": "good.txt", "mimeType": "text/plain", "dataBase64": "***"}]
            }))
            .unwrap(),
            ErrorCode::DocumentInputInvalid,
        ),
        (
            serde_json::to_vec(&serde_json::json!({
                "schemaVersion": 1,
                "files": [{"name": "good.txt", "mimeType": "text/plain", "dataBase64": ""}]
            }))
            .unwrap(),
            ErrorCode::DocumentLimitExceeded,
        ),
    ] {
        let response = base_service
            .handle(request(
                &fixture,
                Method::POST,
                "/internal/ai/to-markdown/v1/transform",
                Body::from(body),
            ))
            .await;
        assert_eq!(response.headers()[ERROR_HEADER], expected.as_str());
    }

    let config = DocumentParserConfig {
        max_batch_files: 1,
        max_input_bytes: 2,
        max_batch_bytes: 3,
        ..DocumentParserConfig::default()
    };
    let limited = DocumentParserBindingService::with_executable(
        fixture.storage.clone(),
        config,
        PathBuf::from("/usr/bin/false"),
    );
    for files in [
        serde_json::json!([
            {"name": "a.txt", "mimeType": "text/plain", "dataBase64": "YQ=="},
            {"name": "b.txt", "mimeType": "text/plain", "dataBase64": "Yg=="}
        ]),
        serde_json::json!([
            {"name": "a.txt", "mimeType": "text/plain", "dataBase64": "YWJj"}
        ]),
        serde_json::json!([
            {"name": "a.txt", "mimeType": "text/plain", "dataBase64": "YWI="},
            {"name": "b.txt", "mimeType": "text/plain", "dataBase64": "YWI="}
        ]),
    ] {
        let response = limited
            .handle(request(
                &fixture,
                Method::POST,
                "/internal/ai/to-markdown/v1/transform",
                Body::from(
                    serde_json::to_vec(&serde_json::json!({
                        "schemaVersion": 1,
                        "files": files
                    }))
                    .unwrap(),
                ),
            ))
            .await;
        assert_eq!(
            response.headers()[ERROR_HEADER],
            ErrorCode::DocumentLimitExceeded.as_str()
        );
    }

    let response = base_service
        .handle(request(
            &fixture,
            Method::PUT,
            "/internal/ai/to-markdown/v1/supported",
            Body::empty(),
        ))
        .await;
    assert_eq!(
        response.headers()[ERROR_HEADER],
        ErrorCode::DocumentProtocolError.as_str()
    );
    let invalid = base_service
        .parse_for_ai_search(fixture.account, "../bad", "text/plain", b"x".to_vec())
        .await
        .unwrap_err();
    assert_eq!(invalid.code(), ErrorCode::DocumentProtocolError);
}

#[tokio::test]
async fn saturated_parser_admission_fails_without_waiting() {
    let (fixture, service) = fixture().await;
    let permits = service.global.available_permits();
    let _held = service
        .global
        .clone()
        .acquire_many_owned(u32::try_from(permits).unwrap())
        .await
        .unwrap();
    let started = Instant::now();
    let error = service
        .parse_for_ai_search(
            fixture.account,
            "fixture.txt",
            "text/plain",
            b"fixture".to_vec(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::DocumentUnavailable);
    assert!(started.elapsed() < Duration::from_millis(100));
}

#[test]
fn expired_batch_deadline_is_rejected_before_spawn() {
    assert_eq!(
        remaining(Instant::now() - Duration::from_millis(1)).unwrap_err(),
        ErrorCode::DocumentTimeout
    );
}

#[tokio::test]
async fn parser_process_accepts_clean_stdout_and_rejects_spawn_stderr_exit_and_timeout() {
    let temporary = tempfile::tempdir().unwrap();
    let script = |name: &str, source: &str| {
        let path = temporary.path().join(name);
        std::fs::write(&path, source).unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&path, permissions).unwrap();
        path
    };
    let clean = script("clean.sh", "#!/bin/sh\nprintf ok\n");
    assert_eq!(
        run_parser_child(&clean, Vec::new(), Duration::from_secs(10), 128, 0, 0)
            .await
            .unwrap(),
        b"ok"
    );

    let stderr = script("stderr.sh", "#!/bin/sh\nprintf diagnostic >&2\n");
    assert_eq!(
        run_parser_child(&stderr, Vec::new(), Duration::from_secs(10), 128, 0, 0).await,
        Err(ErrorCode::DocumentUnavailable)
    );
    let failed = script("failed.sh", "#!/bin/sh\nexit 7\n");
    assert_eq!(
        run_parser_child(&failed, Vec::new(), Duration::from_secs(10), 128, 0, 0).await,
        Err(ErrorCode::DocumentUnavailable)
    );
    let sleeping = script("sleep.sh", "#!/bin/sh\n/bin/sleep 5\n");
    assert_eq!(
        run_parser_child(&sleeping, Vec::new(), Duration::from_millis(10), 128, 0, 0,).await,
        Err(ErrorCode::DocumentTimeout)
    );
    assert_eq!(
        run_parser_child(
            &temporary.path().join("missing"),
            Vec::new(),
            Duration::from_secs(10),
            128,
            0,
            0,
        )
        .await,
        Err(ErrorCode::DocumentUnavailable)
    );
}

fn parser_output_executable(
    temporary: &tempfile::TempDir,
    name: &str,
    output: &ParseOutput,
) -> PathBuf {
    let frame = temporary.path().join(format!("{name}.frame"));
    std::fs::write(&frame, encode_output_frame(output).unwrap()).unwrap();
    let executable = temporary.path().join(format!("{name}.sh"));
    std::fs::write(
        &executable,
        format!("#!/bin/sh\nexec /bin/cat '{}'\n", frame.display()),
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&executable, permissions).unwrap();
    executable
}

fn parser_success(format: DocumentFormat, markdown: &str) -> ParseSuccess {
    ParseSuccess {
        version: open_compute_document_parser::PROTOCOL_VERSION,
        format,
        detected_content_type: format.mime_type().to_string(),
        markdown: markdown.to_string(),
        markdown_sha256: hex::encode(Sha256::digest(markdown.as_bytes())),
        page_count: None,
        sheet_count: None,
        sheet_names: None,
        metadata: DocumentMetadata::default(),
        warnings: Vec::new(),
        parser_contract_sha256: PARSER_CONTRACT_SHA256.to_string(),
    }
}

#[tokio::test]
async fn valid_child_frames_drive_markdown_text_pdf_and_ai_search_success_paths() {
    let (fixture, _) = fixture().await;
    let authority = ParserAuthority {
        account: fixture.account,
        version: fixture.version,
    };
    let temporary = tempfile::tempdir().unwrap();

    let text_success = parser_success(DocumentFormat::Text, "# Heading\n\n- item\n");
    let executable = parser_output_executable(
        &temporary,
        "text",
        &ParseOutput::Success(text_success.clone()),
    );
    let service = DocumentParserBindingService::with_executable(
        fixture.storage.clone(),
        DocumentParserConfig::default(),
        executable,
    );
    // Keep deadlines well above a local fork/exec of the fixture helper so the
    // Gate's parallel host load cannot turn a success-path unit into a timeout.
    let deadline = || Instant::now() + Duration::from_secs(30);
    let markdown = service
        .convert_one(
            authority,
            "note.txt".to_string(),
            "text/plain".to_string(),
            b"ignored".to_vec(),
            &ConversionOptions::default(),
            deadline(),
        )
        .await;
    let ConversionResponse::Success {
        format: OutputFormat::Markdown,
        data,
        tokens,
        ..
    } = markdown
    else {
        panic!("valid child output was not returned: {markdown:?}");
    };
    assert_eq!(data, text_success.markdown);
    assert!(tokens > 0);

    let options: ConversionOptions = serde_json::from_value(serde_json::json!({
        "output": {"format": "text"}
    }))
    .unwrap();
    let text = service
        .convert_one(
            authority,
            "note.txt".to_string(),
            "text/plain".to_string(),
            b"ignored".to_vec(),
            &options,
            deadline(),
        )
        .await;
    let ConversionResponse::Success {
        format: OutputFormat::Text,
        data,
        ..
    } = text
    else {
        panic!("text output was not returned: {text:?}");
    };
    assert_eq!(data, "Heading\nitem");
    assert_eq!(
        service
            .parse_for_ai_search(
                fixture.account,
                "note.txt",
                "text/plain",
                b"ignored".to_vec()
            )
            .await
            .unwrap(),
        text_success
    );

    let mut pdf_success = parser_success(DocumentFormat::Pdf, "body\n");
    pdf_success.metadata.title = Some("Fixture".to_string());
    let executable = parser_output_executable(
        &temporary,
        "pdf",
        &ParseOutput::Success(pdf_success.clone()),
    );
    let pdf_service = DocumentParserBindingService::with_executable(
        fixture.storage.clone(),
        DocumentParserConfig::default(),
        executable,
    );
    let ConversionResponse::Success { data, .. } = pdf_service
        .convert_one(
            authority,
            "fixture.pdf".to_string(),
            "application/pdf".to_string(),
            b"ignored".to_vec(),
            &ConversionOptions::default(),
            deadline(),
        )
        .await
    else {
        panic!("PDF output was not returned");
    };
    assert!(data.starts_with("---\ntitle: \"Fixture\""));

    let constrained = DocumentParserConfig {
        max_output_bytes: 6,
        ..DocumentParserConfig::default()
    };
    let constrained_service = DocumentParserBindingService::with_executable(
        fixture.storage.clone(),
        constrained,
        parser_output_executable(&temporary, "pdf-small", &ParseOutput::Success(pdf_success)),
    );
    let ConversionResponse::Error { error, .. } = constrained_service
        .convert_one(
            authority,
            "fixture.pdf".to_string(),
            "application/pdf".to_string(),
            b"ignored".to_vec(),
            &ConversionOptions::default(),
            deadline(),
        )
        .await
    else {
        panic!("expanded PDF metadata exceeded no limit");
    };
    assert_eq!(error, ErrorCode::DocumentLimitExceeded.as_str());

    let too_small = DocumentParserConfig {
        max_output_bytes: 1,
        ..DocumentParserConfig::default()
    };
    let too_small_service = DocumentParserBindingService::with_executable(
        fixture.storage.clone(),
        too_small,
        parser_output_executable(
            &temporary,
            "text-small",
            &ParseOutput::Success(text_success),
        ),
    );
    let ConversionResponse::Error { error, .. } = too_small_service
        .convert_one(
            authority,
            "note.txt".to_string(),
            "text/plain".to_string(),
            b"ignored".to_vec(),
            &ConversionOptions::default(),
            deadline(),
        )
        .await
    else {
        panic!("oversized parser output was not rejected");
    };
    assert_eq!(error, ErrorCode::DocumentLimitExceeded.as_str());
}
