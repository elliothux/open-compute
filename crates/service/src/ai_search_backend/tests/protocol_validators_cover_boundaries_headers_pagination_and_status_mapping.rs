use super::*;

#[test]
fn protocol_validators_cover_boundaries_headers_pagination_and_status_mapping() {
    assert!(validate_source("a.txt", "text/plain", 1, MAX_UPLOAD_BYTES as u64).is_ok());
    for (name, content_type, size) in [
        ("", "text/plain", 1),
        ("line\nbreak", "text/plain", 1),
        ("a.txt", "", 1),
        ("a.txt", "text/plain\nforged", 1),
        ("a.txt", "text/plain", 0),
        ("a.txt", "text/plain", MAX_UPLOAD_BYTES as u64 + 1),
    ] {
        assert_eq!(
            validate_source(name, content_type, size, MAX_UPLOAD_BYTES as u64)
                .unwrap_err()
                .code(),
            ErrorCode::BindingLimitExceeded
        );
    }

    assert_eq!(page_bounds(None, None, 3).unwrap(), (1, 50, 0, 3));
    assert_eq!(page_bounds(Some(9), Some(10), 3).unwrap(), (9, 10, 3, 3));
    assert_eq!(page_bounds(Some(1), Some(0), 3).unwrap(), (1, 0, 0, 0));
    for (page, per_page) in [(Some(0), Some(1)), (None, Some(101))] {
        assert_eq!(
            page_bounds(page, per_page, 3).unwrap_err().code(),
            ErrorCode::BindingProtocolError
        );
    }
    assert_eq!(
        pagination(2, 3, 10, 22),
        json!({
            "count": 2,
            "page": 3,
            "per_page": 10,
            "total_count": 22,
        })
    );

    let mut headers = HeaderMap::new();
    headers.insert("x-number", "42".parse().unwrap());
    headers.insert("x-digest", hex::encode([3_u8; 32]).parse().unwrap());
    headers.insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
    assert_eq!(parse_header::<u64>(&headers, "x-number").unwrap(), 42);
    assert_eq!(parse_digest(&headers, "x-digest").unwrap(), [3; 32]);
    assert!(content_type_is(&headers, "application/json"));
    assert!(!content_type_is(
        &headers,
        "application/json; charset=utf-8"
    ));
    assert!(header_text(&headers, "missing").is_err());
    headers.insert("x-number", "bad".parse().unwrap());
    assert!(parse_header::<u64>(&headers, "x-number").is_err());
    headers.insert("x-digest", "00".parse().unwrap());
    assert!(parse_digest(&headers, "x-digest").is_err());

    for (code, status) in [
        (ErrorCode::BindingPermissionDenied, StatusCode::FORBIDDEN),
        (ErrorCode::ResourceNotFound, StatusCode::NOT_FOUND),
        (
            ErrorCode::BindingLimitExceeded,
            StatusCode::PAYLOAD_TOO_LARGE,
        ),
        (
            ErrorCode::ResourceUnavailable,
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (ErrorCode::BindingProtocolError, StatusCode::BAD_REQUEST),
    ] {
        let error = PlatformError::new(code, "fixture");
        let response = error_response(&error);
        assert_eq!(response.status(), status);
        assert_eq!(
            response.headers()["x-open-compute-error-code"],
            code.as_str()
        );
    }
}
