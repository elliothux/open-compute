use super::*;

#[test]
fn explicit_cache_default_ttl_is_status_bounded() {
    let now = 1_000;
    assert_eq!(
        cache_deadlines(&[], now, CacheSurface::CacheApiDefault, 200).unwrap(),
        Some((7_201_000, 7_201_000, 7_201_000))
    );
    assert_eq!(
        cache_deadlines(&[], now, CacheSurface::CacheApiDefault, 201).unwrap(),
        None
    );
    assert_eq!(
        cache_deadlines(&[], now, CacheSurface::Automatic, 200)
            .unwrap_err()
            .code(),
        ErrorCode::CachePutRejected
    );
}

#[test]
fn request_policy_and_error_helpers_cover_the_complete_protocol_matrix() {
    let authority = CacheAuthority {
        account: AccountId::generate(),
        worker: WorkerId::generate(),
        version: VersionId::generate(),
        entrypoint: "main_$1".to_owned(),
        automatic_enabled: true,
        cross_version_cache: false,
    };
    let automatic = CacheRequest {
        namespace: "automatic".to_owned(),
        name: None,
        url: "HTTPS://EXAMPLE.TEST:443/path".to_owned(),
        method: "GET".to_owned(),
        headers: vec![
            ("Accept-Language".to_owned(), "en".to_owned()),
            ("accept-language".to_owned(), "fr".to_owned()),
        ],
    }
    .resolve(&authority, true)
    .unwrap();
    assert_eq!(automatic.0.surface, CacheSurface::Automatic);
    assert_eq!(automatic.0.entrypoint.as_deref(), Some("main_$1"));
    assert_eq!(automatic.0.version_scope, authority.version.to_string());
    assert_eq!(automatic.0.canonical_url, "https://example.test/path");
    assert_eq!(automatic.1["accept-language"], "en, fr");

    let mut shared = authority.clone();
    shared.cross_version_cache = true;
    let named = CacheRequest {
        namespace: "named".to_owned(),
        name: Some("pages".to_owned()),
        url: "http://example.test:80/page".to_owned(),
        method: "HEAD".to_owned(),
        headers: Vec::new(),
    }
    .resolve(&shared, false)
    .unwrap();
    assert_eq!(named.0.surface, CacheSurface::CacheApiNamed);
    assert_eq!(named.0.method, CacheMethod::Head);
    assert_eq!(named.0.cache_name.as_deref(), Some("pages"));
    assert_eq!(named.0.version_scope, "shared");

    for request in [
        CacheRequest {
            namespace: "unknown".to_owned(),
            name: None,
            url: "https://example.test/".to_owned(),
            method: "GET".to_owned(),
            headers: Vec::new(),
        },
        CacheRequest {
            namespace: "named".to_owned(),
            name: None,
            url: "https://example.test/".to_owned(),
            method: "GET".to_owned(),
            headers: Vec::new(),
        },
        CacheRequest {
            namespace: "default".to_owned(),
            name: Some("unexpected".to_owned()),
            url: "https://example.test/".to_owned(),
            method: "GET".to_owned(),
            headers: Vec::new(),
        },
        CacheRequest {
            namespace: "default".to_owned(),
            name: None,
            url: "ftp://example.test/".to_owned(),
            method: "GET".to_owned(),
            headers: Vec::new(),
        },
        CacheRequest {
            namespace: "default".to_owned(),
            name: None,
            url: "https://example.test/#fragment".to_owned(),
            method: "GET".to_owned(),
            headers: Vec::new(),
        },
        CacheRequest {
            namespace: "default".to_owned(),
            name: None,
            url: "not-a-url".to_owned(),
            method: "POST".to_owned(),
            headers: Vec::new(),
        },
    ] {
        assert!(request.resolve(&authority, false).is_err());
    }
    assert_eq!(
        CacheRequest {
            namespace: "default".to_owned(),
            name: None,
            url: "https://example.test/".to_owned(),
            method: "HEAD".to_owned(),
            headers: Vec::new(),
        }
        .resolve(&authority, true)
        .unwrap_err()
        .code(),
        ErrorCode::CachePutRejected
    );

    assert!(valid_entrypoint("a_$9"));
    for value in ["", "9bad", "bad-name", &"x".repeat(129)] {
        assert!(!valid_entrypoint(value));
    }
    let mut headers = HeaderMap::new();
    headers.insert("flag", HeaderValue::from_static("true"));
    assert!(bool_header(&headers, "flag").unwrap());
    headers.insert("flag", HeaderValue::from_static("false"));
    assert!(!bool_header(&headers, "flag").unwrap());
    headers.insert("flag", HeaderValue::from_static("maybe"));
    assert_eq!(
        bool_header(&headers, "flag").unwrap_err().code(),
        ErrorCode::CacheProtocolError
    );
    assert_eq!(
        text_header(&headers, "missing").unwrap_err().code(),
        ErrorCode::CacheProtocolError
    );
    assert_eq!(
        parse_header::<u64>(&headers, "flag").unwrap_err().code(),
        ErrorCode::CacheProtocolError
    );
    assert_eq!(add_seconds(5, 2).unwrap(), 2_005);
    assert_eq!(
        add_seconds(i64::MAX, 1).unwrap_err().code(),
        ErrorCode::CacheLimitExceeded
    );

    for status in [
        CacheLookupStatus::Hit,
        CacheLookupStatus::Miss,
        CacheLookupStatus::Expired,
        CacheLookupStatus::Updating,
        CacheLookupStatus::Stale,
        CacheLookupStatus::StaleIfError,
    ] {
        assert!(!lookup_status(status).is_empty());
    }
    let lookup = open_compute_storage::CacheLookup {
        status: CacheLookupStatus::Updating,
        response: None,
        fence_generation: 7,
        refresh_token: Some("11".repeat(16)),
    };
    let mut lookup_headers = HeaderMap::new();
    insert_lookup_headers(&mut lookup_headers, &lookup).unwrap();
    assert_eq!(lookup_headers["x-open-compute-cache-status"], "UPDATING");
    assert_eq!(lookup_headers["x-open-compute-cache-fence"], "7");
    assert!(lookup_headers.contains_key("x-open-compute-cache-refresh-token"));

    for path in [
        "/internal/cache/v1/match",
        "/internal/cache/v1/put",
        "/internal/cache/v1/delete",
        "/internal/cache/v1/purge",
    ] {
        assert!(cache_metric_operation(path).is_some());
    }
    assert!(cache_metric_operation("/internal/cache/v1/unknown").is_none());

    for (code, status) in [
        (ErrorCode::CacheKeyInvalid, StatusCode::BAD_REQUEST),
        (ErrorCode::CacheProtocolError, StatusCode::BAD_REQUEST),
        (
            ErrorCode::CachePutRejected,
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (ErrorCode::CacheLimitExceeded, StatusCode::PAYLOAD_TOO_LARGE),
        (ErrorCode::CacheUnavailable, StatusCode::SERVICE_UNAVAILABLE),
        (
            ErrorCode::CacheResultUnknown,
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (ErrorCode::CacheCorrupt, StatusCode::INTERNAL_SERVER_ERROR),
        (ErrorCode::Internal, StatusCode::INTERNAL_SERVER_ERROR),
    ] {
        let response = cache_error(&PlatformError::new(code, "test"));
        assert_eq!(response.status(), status);
        assert_eq!(response.headers()[ERROR_HEADER], code.as_str());
    }
    for code in [
        ErrorCode::ArtifactIntegrityError,
        ErrorCode::CacheEntryCorrupt,
        ErrorCode::PathInvalid,
    ] {
        assert_eq!(
            cache_artifact_error(&PlatformError::new(code, "test")).code(),
            ErrorCode::CacheCorrupt
        );
    }
    assert_eq!(
        cache_artifact_error(&PlatformError::new(ErrorCode::ArtifactUnavailable, "test")).code(),
        ErrorCode::CacheUnavailable
    );
}

#[tokio::test]
async fn framed_staging_rejects_incomplete_metadata_and_oversized_bodies() {
    let directory = tempfile::TempDir::new().unwrap();
    for bytes in [Vec::new(), 0_u32.to_be_bytes().to_vec(), {
        let mut bytes = 5_u32.to_be_bytes().to_vec();
        bytes.extend_from_slice(b"abc");
        bytes
    }] {
        assert_eq!(
            stage_framed_body(Body::from(bytes), directory.path().to_path_buf(), 8)
                .await
                .err()
                .unwrap()
                .code(),
            ErrorCode::CacheProtocolError
        );
    }
    let mut oversized = 2_u32.to_be_bytes().to_vec();
    oversized.extend_from_slice(b"{}");
    oversized.extend_from_slice(b"12345");
    assert_eq!(
        stage_framed_body(Body::from(oversized), directory.path().to_path_buf(), 4)
            .await
            .err()
            .unwrap()
            .code(),
        ErrorCode::CacheLimitExceeded
    );
    let mut valid = 2_u32.to_be_bytes().to_vec();
    valid.extend_from_slice(b"{}");
    valid.extend_from_slice(b"body");
    let staged = stage_framed_body(Body::from(valid), directory.path().to_path_buf(), 4)
        .await
        .unwrap();
    assert_eq!(staged.metadata, b"{}");
    assert_eq!(staged.size, 4);
    assert_eq!(staged.sha256, hex::encode(Sha256::digest(b"body")));
    assert!(staged.path.exists());
    drop(staged);
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}
