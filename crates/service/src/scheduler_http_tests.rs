use super::*;
use axum::body::Body;

fn request(uri: &str, body: Body) -> Request {
    Request::builder().uri(uri).body(body).unwrap()
}

#[tokio::test]
async fn operator_protocol_parsing_and_error_mapping_are_exact() {
    assert_eq!(
        requested_kind(&request("/v1/scheduler", Body::empty())).unwrap(),
        None
    );
    for kind in open_compute_core::SchedulerKind::ALL {
        assert_eq!(
            requested_kind(&request(
                &format!("/v1/scheduler?kind={}", kind.as_str()),
                Body::empty(),
            ))
            .unwrap(),
            Some(kind)
        );
    }
    for uri in [
        "/v1/scheduler?kind=missing",
        "/v1/scheduler?other=queue",
        "/v1/scheduler?kind=queue&kind=cron",
    ] {
        assert_eq!(
            requested_kind(&request(uri, Body::empty()))
                .unwrap_err()
                .code(),
            ErrorCode::SchedulerKindNotEnabled
        );
    }

    let generated = request_id(&request("/", Body::empty()));
    let expected = RequestId::generate();
    let mut supplied = request("/", Body::empty());
    supplied.extensions_mut().insert(expected);
    assert_eq!(request_id(&supplied), expected);
    assert_ne!(generated, expected);

    assert_eq!(
        read_generation(request("/", Body::from(r#"{"consumerGeneration":7}"#),))
            .await
            .unwrap()
            .consumer_generation,
        7
    );
    for body in [
        Body::from("{}"),
        Body::from(vec![b'x'; MAX_OPERATOR_BODY + 1]),
    ] {
        let Err(error) = read_generation(request("/", body)).await else {
            panic!("invalid generation body was accepted")
        };
        assert_eq!(error.code(), ErrorCode::ConfigInvalid);
    }

    for (code, status) in [
        (ErrorCode::SchedulerKindNotEnabled, StatusCode::BAD_REQUEST),
        (ErrorCode::ConfigInvalid, StatusCode::BAD_REQUEST),
        (
            ErrorCode::QueueConsumerGenerationStale,
            StatusCode::CONFLICT,
        ),
        (
            ErrorCode::SchedulerUnavailable,
            StatusCode::SERVICE_UNAVAILABLE,
        ),
    ] {
        let response = scheduler_error(&PlatformError::new(code, "unsafe detail"));
        assert_eq!(response.status(), status);
        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap()["code"],
            code.as_str()
        );
    }
    assert_eq!(
        kind_not_enabled().code(),
        ErrorCode::SchedulerKindNotEnabled
    );
    assert_eq!(invalid_operator_request().code(), ErrorCode::ConfigInvalid);
}
