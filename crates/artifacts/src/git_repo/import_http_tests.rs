use super::*;

#[test]
fn import_http_statuses_preserve_artifacts_error_classes() {
    assert!(matches!(
        classify_status(reqwest::StatusCode::UNAUTHORIZED),
        RemoteFailure::AuthenticationRequired
    ));
    assert!(matches!(
        classify_status(reqwest::StatusCode::NOT_FOUND),
        RemoteFailure::NotFound
    ));
    assert!(matches!(
        classify_status(reqwest::StatusCode::BAD_GATEWAY),
        RemoteFailure::UpstreamUnavailable
    ));
    assert!(matches!(
        classify_status(reqwest::StatusCode::MOVED_PERMANENTLY),
        RemoteFailure::InvalidUrl
    ));
}

#[test]
fn import_url_ip_and_transport_failures_are_fail_closed() {
    assert_eq!(
        canonical_public_https_remote("https://example.com/repo.git").unwrap(),
        "https://example.com/repo.git"
    );
    for remote in [
        "https://example.com:444/repo.git",
        "https://example.com/repo.git#fragment",
        "https://[::1]/repo.git",
        "https://[::ffff:127.0.0.1]/repo.git",
    ] {
        assert_eq!(
            canonical_public_https_remote(remote).unwrap_err().code(),
            open_compute_core::ErrorCode::PathInvalid
        );
    }
    for private in [
        "0.1.2.3",
        "10.1.2.3",
        "100.64.0.1",
        "127.0.0.1",
        "169.254.1.1",
        "172.16.0.1",
        "192.0.0.1",
        "192.0.2.1",
        "192.88.99.1",
        "192.168.0.1",
        "198.18.0.1",
        "198.51.100.1",
        "203.0.113.1",
        "224.0.0.1",
        "::1",
        "2001:db8::1",
        "2002::1",
    ] {
        assert!(!public_ip(private.parse().unwrap()), "{private}");
    }
    for public in ["8.8.8.8", "1.1.1.1", "2606:4700:4700::1111"] {
        assert!(public_ip(public.parse().unwrap()), "{public}");
    }

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let mut http = PinnedHttp::new(
        "example.com".to_owned(),
        &[address],
        1024,
        Duration::from_millis(100),
    )
    .unwrap();
    assert!(http.configure(&()).is_ok());
    assert!(
        http.get("http://example.com/repo.git", "", ["accept: */*"])
            .is_err()
    );
    assert!(
        http.post(
            "https://other.example/repo.git",
            "",
            ["content-type: application/x-git-upload-pack-request"],
            PostBodyDataKind::BoundedAndFitsIntoMemory,
        )
        .is_err()
    );
    let mut response = http
        .get("https://example.com/repo.git", "", ["accept: */*"])
        .unwrap();
    let mut bytes = Vec::new();
    assert!(response.headers.read_to_end(&mut bytes).is_err());
    assert_eq!(
        http.failure_error().code(),
        open_compute_core::ErrorCode::ResourceUnavailable
    );

    for (failure, code) in [
        (
            RemoteFailure::InvalidUrl,
            open_compute_core::ErrorCode::ArtifactUnavailable,
        ),
        (
            RemoteFailure::AuthenticationRequired,
            open_compute_core::ErrorCode::BindingPermissionDenied,
        ),
        (
            RemoteFailure::NotFound,
            open_compute_core::ErrorCode::ResourceNotFound,
        ),
        (
            RemoteFailure::UpstreamUnavailable,
            open_compute_core::ErrorCode::ResourceUnavailable,
        ),
        (
            RemoteFailure::MemoryLimit,
            open_compute_core::ErrorCode::ResourceLimitExceeded,
        ),
    ] {
        record_failure(&http.failure, failure);
        assert_eq!(http.failure_error().code(), code);
    }
}
