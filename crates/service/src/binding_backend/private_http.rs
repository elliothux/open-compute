use super::*;

const SESSION_HEADER: &str = "x-open-compute-private-service";
const PATH_HEADER: &str = "x-open-compute-private-path";

pub(super) async fn proxy(
    registry: &crate::service_invocations::ServiceInvocationRegistry,
    request: Request,
) -> Response {
    let Some(identity) = header_text(request.headers(), SESSION_HEADER) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(target) = registry.private_http_for_session(identity) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    forward(&target, request).await
}

async fn forward(
    target: &crate::local_extensions::PrivateHttpTarget,
    request: Request,
) -> Response {
    let Some(path) = header_text(request.headers(), PATH_HEADER) else {
        return backend_error(ErrorCode::BindingProtocolError, StatusCode::BAD_REQUEST);
    };
    let pathname = path.split_once('?').map_or(path, |(pathname, _)| pathname);
    let lowercase_path = pathname.to_ascii_lowercase();
    if !pathname.starts_with('/')
        || pathname.contains(['#', '\\'])
        || pathname.contains("..")
        || lowercase_path.contains("%2e")
        || !target.methods.contains(request.method().as_str())
        || !target.path_prefixes.iter().any(|prefix| {
            pathname == prefix
                || (pathname.starts_with(prefix)
                    && (prefix.ends_with('/')
                        || pathname.as_bytes().get(prefix.len()) == Some(&b'/')))
        })
    {
        return backend_error(ErrorCode::ServiceBindingDenied, StatusCode::FORBIDDEN);
    }
    let Ok(method) = Method::from_bytes(request.method().as_str().as_bytes()) else {
        return backend_error(ErrorCode::BindingProtocolError, StatusCode::BAD_REQUEST);
    };
    let mut outbound = target
        .client
        .request(method, format!("{}{path}", target.base_url));
    for (name, value) in request.headers() {
        if name.as_str().starts_with("x-open-compute-")
            || matches!(
                name.as_str(),
                "authorization"
                    | "cookie"
                    | "host"
                    | "connection"
                    | "proxy-authorization"
                    | "transfer-encoding"
                    | "upgrade"
            )
        {
            continue;
        }
        outbound = outbound.header(name, value);
    }
    if let Some((name, value)) = &target.credential {
        outbound = outbound.header(name, value);
    }
    let body = Limited::new(request.into_body(), 8 * 1024 * 1024).into_data_stream();
    let upstream = match outbound.body(reqwest::Body::wrap_stream(body)).send().await {
        Ok(value) => value,
        Err(error) if caused_by_length_limit(&error) => {
            return backend_error(
                ErrorCode::BindingProtocolError,
                StatusCode::PAYLOAD_TOO_LARGE,
            );
        }
        Err(_) => return backend_error(ErrorCode::ServiceUnavailable, StatusCode::BAD_GATEWAY),
    };
    let status = upstream.status();
    let mut response = Response::builder().status(status);
    for (name, value) in upstream.headers() {
        let is_credential = target
            .credential
            .as_ref()
            .is_some_and(|(credential, _)| credential == name);
        if !is_credential
            && !matches!(
                name.as_str(),
                "connection"
                    | "transfer-encoding"
                    | "upgrade"
                    | "set-cookie"
                    | "www-authenticate"
                    | "proxy-authenticate"
            )
        {
            response = response.header(name, value);
        }
    }
    response
        .body(Body::from_stream(upstream.bytes_stream()))
        .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
}

fn caused_by_length_limit(error: &(dyn std::error::Error + 'static)) -> bool {
    let mut current = Some(error);
    while let Some(error) = current {
        if error.is::<http_body_util::LengthLimitError>() {
            return true;
        }
        current = error.source();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::any;
    use std::collections::BTreeSet;

    #[tokio::test]
    async fn private_http_proxy_streams_and_strips_internal_credentials() {
        let (_temp, _mock, _state, _account, storage) =
            crate::tests::initialized_worker_http_fixture().await;
        let registry = crate::service_invocations::ServiceInvocationRegistry::new(
            storage,
            open_compute_workers::VersionPins::new(),
        );
        assert_eq!(
            proxy(&registry, Request::builder().body(Body::empty()).unwrap(),)
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            proxy(
                &registry,
                Request::builder()
                    .header(SESSION_HEADER, "unknown")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let fixture = Router::new()
            .route(
                "/allowed",
                any(|request: Request| async move {
                    let internal = request
                        .headers()
                        .keys()
                        .any(|name| name.as_str().starts_with("x-open-compute-"));
                    let credential = header_text(request.headers(), "x-api-key")
                        .unwrap_or("")
                        .to_owned();
                    let body = to_bytes(request.into_body(), 1024).await.unwrap();
                    Response::builder()
                        .header("x-api-key", "must-not-leak")
                        .header(header::SET_COOKIE, "must-not-leak")
                        .header(header::WWW_AUTHENTICATE, "must-not-leak")
                        .header(header::CONNECTION, "close")
                        .body(Body::from(format!(
                            "{internal}:{credential}:{}",
                            String::from_utf8_lossy(&body)
                        )))
                        .unwrap()
                }),
            )
            .route(
                "/redirect",
                any(|| async {
                    Response::builder()
                        .status(StatusCode::FOUND)
                        .header(header::LOCATION, "http://127.0.0.1:1/escaped")
                        .body(Body::empty())
                        .unwrap()
                }),
            );
        let server = tokio::spawn(async move { axum::serve(listener, fixture).await.unwrap() });
        let mut credential = HeaderValue::from_static("operator-secret");
        credential.set_sensitive(true);
        let target = crate::local_extensions::PrivateHttpTarget {
            base_url: format!("http://{address}"),
            methods: BTreeSet::from(["POST".to_owned()]),
            path_prefixes: vec!["/allowed".to_owned(), "/redirect".to_owned()],
            credential: Some((HeaderName::from_static("x-api-key"), credential)),
            policy_revision: "a".repeat(64),
            grants: Vec::new(),
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
        };
        let request = Request::builder()
            .method(Method::POST)
            .uri("/internal/services/v1/private-http")
            .header(PATH_HEADER, "/allowed")
            .header(SESSION_HEADER, "session")
            .header(TOKEN_HEADER, "runtime-secret")
            .header(GENERATION_HEADER, "generation-secret")
            .header(header::AUTHORIZATION, "tenant-secret")
            .header(header::COOKIE, "tenant-cookie")
            .header("proxy-authorization", "tenant-proxy-secret")
            .header(header::CONNECTION, "keep-alive")
            .header(header::UPGRADE, "websocket")
            .body(Body::from("payload"))
            .unwrap();
        let response = forward(&target, request).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(!response.headers().contains_key("x-api-key"));
        assert!(!response.headers().contains_key(header::SET_COOKIE));
        assert!(!response.headers().contains_key(header::WWW_AUTHENTICATE));
        assert_eq!(
            to_bytes(response.into_body(), 1024).await.unwrap(),
            "false:operator-secret:payload"
        );

        let redirect = Request::builder()
            .method(Method::POST)
            .uri("/internal/services/v1/private-http")
            .header(PATH_HEADER, "/redirect")
            .body(Body::empty())
            .unwrap();
        assert_eq!(forward(&target, redirect).await.status(), StatusCode::FOUND);

        let missing_path = Request::builder()
            .method(Method::POST)
            .uri("/internal/services/v1/private-http")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            forward(&target, missing_path).await.status(),
            StatusCode::BAD_REQUEST
        );

        for (method, path) in [
            (Method::GET, "/allowed"),
            (Method::POST, "allowed"),
            (Method::POST, "/allowed/../escaped"),
            (Method::POST, "/allowed%2e%2e/escaped"),
            (Method::POST, "/allowed#fragment"),
            (Method::POST, "/allowed\\escaped"),
        ] {
            let denied = Request::builder()
                .method(method)
                .uri("/internal/services/v1/private-http")
                .header(PATH_HEADER, path)
                .body(Body::empty())
                .unwrap();
            assert_eq!(
                forward(&target, denied).await.status(),
                StatusCode::FORBIDDEN
            );
        }

        let prefix_bypass = Request::builder()
            .method(Method::POST)
            .uri("/internal/services/v1/private-http")
            .header(PATH_HEADER, "/allowed-elsewhere")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            forward(&target, prefix_bypass).await.status(),
            StatusCode::FORBIDDEN
        );
        let mut unavailable = target.clone();
        unavailable.base_url = "http://127.0.0.1:1".to_owned();
        let request = Request::builder()
            .method(Method::POST)
            .uri("/internal/services/v1/private-http")
            .header(PATH_HEADER, "/allowed")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            forward(&unavailable, request).await.status(),
            StatusCode::BAD_GATEWAY
        );
        server.abort();
    }
}
