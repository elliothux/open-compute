use super::*;
use crate::p3_3_test_support::RuntimeFeatureFixture;
use axum::http::Request;
use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
use open_compute_core::MetricsConfig;
use open_compute_images::RasterFormat;
use open_compute_workers::{
    DeploymentImagesInput, DeploymentRuntimeFeatures, DeploymentVersionMetadataInput,
};

async fn fixture(config: ImagesConfig) -> (RuntimeFeatureFixture, ImageBindingService, Vec<u8>) {
    let fixture = RuntimeFeatureFixture::create(DeploymentRuntimeFeatures {
        cache: Default::default(),
        images: Some(DeploymentImagesInput {
            binding: "IMAGES".to_owned(),
        }),
        version_metadata: Some(DeploymentVersionMetadataInput {
            binding: "VERSION".to_owned(),
            tag: Some("release-1".to_owned()),
        }),
    })
    .await;
    let service = ImageBindingService::new(fixture.storage.clone(), config);
    let mut png = Vec::new();
    DynamicImage::ImageRgba8(RgbaImage::from_pixel(2, 2, Rgba([10, 20, 30, 255])))
        .write_to(&mut std::io::Cursor::new(&mut png), ImageFormat::Png)
        .unwrap();
    (fixture, service, png)
}

fn request(fixture: &RuntimeFeatureFixture, path: &str, body: Body) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header(ACCOUNT_HEADER, fixture.account.to_string())
        .header(WORKER_HEADER, fixture.worker.to_string())
        .header(DEPLOYMENT_HEADER, fixture.deployment.to_string())
        .header(
            DESCRIPTOR_HEADER,
            fixture.images_descriptor_sha256.as_deref().unwrap(),
        )
        .header(GENERATION_HEADER, "test-generation")
        .body(body)
        .unwrap()
}

async fn response_bytes(response: Response) -> Vec<u8> {
    to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec()
}

async fn input_session(
    fixture: &RuntimeFeatureFixture,
    service: &ImageBindingService,
    png: &[u8],
) -> String {
    let response = service
        .handle(request(
            fixture,
            "/internal/images/v1/input",
            Body::from(png.to_vec()),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let value: serde_json::Value = serde_json::from_slice(&response_bytes(response).await).unwrap();
    value["session"].as_str().unwrap().to_owned()
}

fn draw_frame(metadata: &serde_json::Value, body: &[u8]) -> Body {
    let metadata = serde_json::to_vec(&metadata).unwrap();
    let mut bytes = Vec::with_capacity(4 + metadata.len() + body.len());
    bytes.extend_from_slice(&u32::try_from(metadata.len()).unwrap().to_be_bytes());
    bytes.extend_from_slice(&metadata);
    bytes.extend_from_slice(body);
    Body::from(bytes)
}

#[tokio::test]
async fn images_wire_executes_info_ordered_transform_draw_and_every_output_codec() {
    let (fixture, service, png) = fixture(ImagesConfig::default()).await;
    let info = service
        .handle(request(
            &fixture,
            "/internal/images/v1/info",
            Body::from(png.clone()),
        ))
        .await;
    assert_eq!(info.status(), StatusCode::OK);
    let value: serde_json::Value = serde_json::from_slice(&response_bytes(info).await).unwrap();
    assert_eq!(value["format"], "png");
    assert_eq!(value["width"], 2);
    assert_eq!(value["height"], 2);

    let session = input_session(&fixture, &service, &png).await;
    let transform = service
        .handle(request(
            &fixture,
            &format!("/internal/images/v1/session/{session}/transform"),
            Body::from(
                br##"{"width":4,"height":3,"fit":"pad","gravity":"bottom-right","background":"#102030ff","rotate":90,"flip":"horizontal","blur":0.5}"##
                    .as_slice(),
            ),
        ))
        .await;
    assert_eq!(transform.status(), StatusCode::NO_CONTENT);
    let draw = service
        .handle(request(
            &fixture,
            &format!("/internal/images/v1/session/{session}/draw"),
            draw_frame(
                &serde_json::json!({"left":1,"top":1,"opacity":0.5,"repeat":false,"blend":"over"}),
                &png,
            ),
        ))
        .await;
    assert_eq!(draw.status(), StatusCode::NO_CONTENT);
    let output = service
        .handle(request(
            &fixture,
            &format!("/internal/images/v1/session/{session}/output"),
            Body::from(br#"{"format":"png","anim":false}"#.as_slice()),
        ))
        .await;
    assert_eq!(
        output.status(),
        StatusCode::OK,
        "{:?}",
        output.headers().get(ERROR_HEADER)
    );
    assert_eq!(output.headers()[header::CONTENT_TYPE], "image/png");
    let transformed = response_bytes(output).await;
    let transformed_info = ImageEngine::new(ImagesConfig::default())
        .info(&transformed)
        .unwrap();
    assert_eq!((transformed_info.width, transformed_info.height), (3, 4));

    let consumed = service
        .handle(request(
            &fixture,
            &format!("/internal/images/v1/session/{session}/output"),
            Body::from(br#"{"format":"png"}"#.as_slice()),
        ))
        .await;
    assert_eq!(consumed.status(), StatusCode::BAD_REQUEST);

    for (format, content_type, detectable) in [
        ("jpeg", "image/jpeg", Some(RasterFormat::Jpeg)),
        ("png", "image/png", Some(RasterFormat::Png)),
        ("webp", "image/webp", Some(RasterFormat::Webp)),
        ("avif", "image/avif", None),
    ] {
        let session = input_session(&fixture, &service, &png).await;
        let options = serde_json::to_vec(&serde_json::json!({
            "format": format,
            "quality": (format == "jpeg").then_some(80),
            "anim": false,
        }))
        .unwrap();
        let output = service
            .handle(request(
                &fixture,
                &format!("/internal/images/v1/session/{session}/output"),
                Body::from(options),
            ))
            .await;
        assert_eq!(output.status(), StatusCode::OK, "{format}");
        assert_eq!(output.headers()[header::CONTENT_TYPE], content_type);
        let bytes = response_bytes(output).await;
        assert!(!bytes.is_empty());
        if let Some(expected) = detectable {
            assert_eq!(
                ImageEngine::new(ImagesConfig::default())
                    .info(&bytes)
                    .unwrap()
                    .format,
                expected,
            );
        } else {
            assert!(bytes.windows(4).any(|window| window == b"ftyp"));
        }
    }
}

#[tokio::test]
async fn images_wire_enforces_input_session_and_authority_limits() {
    let config = ImagesConfig {
        max_sessions: 1,
        session_ttl_ms: 100,
        ..ImagesConfig::default()
    };
    let (fixture, service, png) = fixture(config).await;
    let _session = input_session(&fixture, &service, &png).await;
    let limited = service
        .handle(request(
            &fixture,
            "/internal/images/v1/input",
            Body::from(png.clone()),
        ))
        .await;
    assert_eq!(limited.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        limited.headers()[ERROR_HEADER],
        ErrorCode::ImageLimitExceeded.as_str()
    );

    tokio::time::sleep(Duration::from_millis(150)).await;
    let _replacement = input_session(&fixture, &service, &png).await;

    let invalid = service
        .handle(request(
            &fixture,
            "/internal/images/v1/info",
            Body::from("not-an-image"),
        ))
        .await;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        invalid.headers()[ERROR_HEADER],
        ErrorCode::ImageInputInvalid.as_str()
    );

    let mut forged = request(&fixture, "/internal/images/v1/info", Body::from(png));
    forged.headers_mut().insert(
        HeaderName::from_static(DESCRIPTOR_HEADER),
        HeaderValue::from_static("00"),
    );
    let denied = service.handle(forged).await;
    assert_eq!(denied.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        denied.headers()[ERROR_HEADER],
        ErrorCode::ImageProtocolError.as_str()
    );
}

#[tokio::test]
async fn images_sessions_are_generation_scoped_and_clear_staged_files() {
    let (fixture, service, png) = fixture(ImagesConfig::default()).await;
    let session = input_session(&fixture, &service, &png).await;
    let staged = service
        .sessions
        .lock()
        .unwrap()
        .get(&session)
        .unwrap()
        .base
        .clone();
    assert!(staged.exists());

    let mut stale = request(
        &fixture,
        &format!("/internal/images/v1/session/{session}/transform"),
        Body::from(br#"{"width":1}"#.as_slice()),
    );
    stale.headers_mut().insert(
        HeaderName::from_static(GENERATION_HEADER),
        HeaderValue::from_static("next-generation"),
    );
    let denied = service.handle(stale).await;
    assert_eq!(denied.status(), StatusCode::BAD_REQUEST);
    assert_eq!(service.capacity().unwrap().active_sessions, 0);
    assert!(!staged.exists());

    let _session = input_session(&fixture, &service, &png).await;
    assert_eq!(service.capacity().unwrap().active_sessions, 1);
    service.clear_sessions().unwrap();
    assert_eq!(service.capacity().unwrap().active_sessions, 0);
}

#[test]
fn image_option_compilation_covers_every_supported_shape_and_rejection() {
    for value in [
        serde_json::json!({"width": 2, "fit": "cover", "gravity": "top-left", "background": "#01020304"}),
        serde_json::json!({"height": 3, "background": "#010203"}),
        serde_json::json!({"rotate": 180}),
        serde_json::json!({"flip": "horizontal"}),
        serde_json::json!({"flip": "vertical"}),
        serde_json::json!({"flip": "both"}),
        serde_json::json!({"background": "#01020304"}),
        serde_json::json!({"blur": 0.5}),
    ] {
        let request: TransformRequest = serde_json::from_value(value).unwrap();
        assert!(!request.operations().unwrap().is_empty());
    }
    for value in [
        serde_json::json!({}),
        serde_json::json!({"fit": "cover"}),
        serde_json::json!({"gravity": "top"}),
        serde_json::json!({"flip": "diagonal"}),
        serde_json::json!({"background": "010203"}),
        serde_json::json!({"background": "#01"}),
        serde_json::json!({"background": "#not-hex"}),
    ] {
        let request: TransformRequest = serde_json::from_value(value).unwrap();
        assert_eq!(
            request.operations().unwrap_err().code(),
            ErrorCode::ImageOptionUnsupported
        );
    }
    assert!(
        serde_json::from_value::<TransformRequest>(serde_json::json!({"unknown": true})).is_err()
    );

    let default_draw: DrawRequest = serde_json::from_value(serde_json::json!({})).unwrap();
    assert_eq!(default_draw.opacity, 1.0);
    default_draw.validate().unwrap();
    for value in [
        serde_json::json!({"repeat": true}),
        serde_json::json!({"opacity": -0.1}),
        serde_json::json!({"opacity": 1.1}),
        serde_json::json!({"blend": "multiply"}),
    ] {
        let request: DrawRequest = serde_json::from_value(value).unwrap();
        assert_eq!(
            request.validate().unwrap_err().code(),
            ErrorCode::ImageOptionUnsupported
        );
    }
    for blend in ["normal", "over"] {
        let request: DrawRequest =
            serde_json::from_value(serde_json::json!({"blend": blend})).unwrap();
        request.validate().unwrap();
    }
}

#[test]
fn image_protocol_helpers_map_paths_headers_metrics_and_errors() {
    let session = Uuid::now_v7().to_string();
    assert_eq!(
        parse_session_path(&format!("/internal/images/v1/session/{session}/transform")).unwrap(),
        (session.clone(), "transform")
    );
    for path in [
        "/internal/images/v1/session/not-a-uuid/transform",
        "/internal/images/v1/session/",
        "/internal/images/v1/session/00000000-0000-0000-0000-000000000000/draw/extra",
        "/wrong-prefix",
    ] {
        assert_eq!(
            parse_session_path(path).unwrap_err().code(),
            ErrorCode::ImageProtocolError
        );
    }

    let mut headers = HeaderMap::new();
    headers.insert("number", HeaderValue::from_static("7"));
    assert_eq!(text_header(&headers, "number").unwrap(), "7");
    assert_eq!(parse_header::<u64>(&headers, "number").unwrap(), 7);
    headers.insert("number", HeaderValue::from_static("bad"));
    assert_eq!(
        parse_header::<u64>(&headers, "number").unwrap_err().code(),
        ErrorCode::ImageProtocolError
    );
    assert_eq!(
        text_header(&headers, "missing").unwrap_err().code(),
        ErrorCode::ImageProtocolError
    );

    for path in [
        "/internal/images/v1/input",
        "/internal/images/v1/info",
        "/internal/images/v1/session/id/transform",
        "/internal/images/v1/session/id/draw",
        "/internal/images/v1/session/id/output",
    ] {
        assert!(image_metric_operation(path).is_some());
    }
    assert!(image_metric_operation("/internal/images/v1/unknown").is_none());

    for (code, status) in [
        (ErrorCode::ImageInputInvalid, StatusCode::BAD_REQUEST),
        (ErrorCode::ImageOptionUnsupported, StatusCode::BAD_REQUEST),
        (ErrorCode::ImageProtocolError, StatusCode::BAD_REQUEST),
        (
            ErrorCode::ImageFormatUnsupported,
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ),
        (ErrorCode::ImageLimitExceeded, StatusCode::PAYLOAD_TOO_LARGE),
        (ErrorCode::ImageTimeout, StatusCode::GATEWAY_TIMEOUT),
        (ErrorCode::ImageUnavailable, StatusCode::SERVICE_UNAVAILABLE),
    ] {
        let response = image_error(&PlatformError::new(code, "test"));
        assert_eq!(response.status(), status);
        assert_eq!(response.headers()[ERROR_HEADER], code.as_str());
    }
    assert_eq!(invalid_input().code(), ErrorCode::ImageInputInvalid);
    assert_eq!(option().code(), ErrorCode::ImageOptionUnsupported);
    assert_eq!(limit().code(), ErrorCode::ImageLimitExceeded);
    assert_eq!(timeout().code(), ErrorCode::ImageTimeout);
    assert_eq!(unavailable().code(), ErrorCode::ImageUnavailable);
    assert_eq!(protocol().code(), ErrorCode::ImageProtocolError);
}

#[tokio::test]
async fn image_staging_enforces_framing_and_removes_every_temporary_file() {
    let directory = tempfile::TempDir::new().unwrap();
    let staged = stage_body(
        Body::from("image"),
        directory.path().to_path_buf(),
        5,
        "input",
    )
    .await
    .unwrap();
    assert_eq!(staged.size, 5);
    assert!(staged.path.as_ref().unwrap().exists());
    drop(staged);
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    assert_eq!(
        stage_body(
            Body::from("too-large"),
            directory.path().to_path_buf(),
            2,
            "input",
        )
        .await
        .err()
        .unwrap()
        .code(),
        ErrorCode::ImageLimitExceeded
    );

    for bytes in [Vec::new(), 0_u32.to_be_bytes().to_vec(), {
        let mut bytes = 5_u32.to_be_bytes().to_vec();
        bytes.extend_from_slice(b"abc");
        bytes
    }] {
        assert_eq!(
            stage_framed(
                Body::from(bytes),
                directory.path().to_path_buf(),
                8,
                "overlay",
            )
            .await
            .err()
            .unwrap()
            .code(),
            ErrorCode::ImageProtocolError
        );
    }
    let mut oversized = 2_u32.to_be_bytes().to_vec();
    oversized.extend_from_slice(b"{}");
    oversized.extend_from_slice(b"12345");
    assert_eq!(
        stage_framed(
            Body::from(oversized),
            directory.path().to_path_buf(),
            4,
            "overlay",
        )
        .await
        .err()
        .unwrap()
        .code(),
        ErrorCode::ImageLimitExceeded
    );
    let mut valid = 2_u32.to_be_bytes().to_vec();
    valid.extend_from_slice(b"{}");
    valid.extend_from_slice(b"body");
    let staged = stage_framed(
        Body::from(valid),
        directory.path().to_path_buf(),
        4,
        "overlay",
    )
    .await
    .unwrap();
    assert_eq!(staged.metadata, b"{}");
    assert_eq!(staged.size, 4);
    assert!(staged.path.as_ref().unwrap().exists());
    drop(staged);
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn images_metrics_record_success_failure_limit_and_session_lifecycle() {
    let (fixture, service, png) = fixture(ImagesConfig {
        max_sessions: 1,
        ..ImagesConfig::default()
    })
    .await;
    let metrics =
        Arc::new(MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd").unwrap());
    let service = service.with_metrics(metrics.clone());
    let info = service
        .handle(request(
            &fixture,
            "/internal/images/v1/info",
            Body::from(png.clone()),
        ))
        .await;
    assert_eq!(info.status(), StatusCode::OK);
    let invalid = service
        .handle(request(
            &fixture,
            "/internal/images/v1/info",
            Body::from("not-image"),
        ))
        .await;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    let _session = input_session(&fixture, &service, &png).await;
    let limited = service
        .handle(request(
            &fixture,
            "/internal/images/v1/input",
            Body::from(png),
        ))
        .await;
    assert_eq!(limited.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let rendered = metrics.render(&open_compute_core::PlatformStatus::starting());
    assert!(rendered.contains("images_operations_total{operation=\"info\",outcome=\"success\"} 1"));
    assert!(rendered.contains("images_operations_total{operation=\"info\",outcome=\"failure\"} 1"));
    assert!(rendered.contains("images_operations_total{operation=\"input\",outcome=\"limit\"} 1"));
    assert!(rendered.contains("images_active_sessions 1"));
    assert!(rendered.contains("images_bytes_total{direction=\"input\"}"));
    service.clear_sessions().unwrap();
}
