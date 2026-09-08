//! Homologous Dashboard session exchange endpoints.

use crate::auth::{bearer_matches, bearer_token};
use crate::dashboard_auth::SessionIssue;
use crate::http::{HttpState, REQUEST_ID_HEADER, platform_error_response};
use axum::Json;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use open_compute_core::{ErrorCode, PlatformError, RequestId};
use serde::Deserialize;
use std::time::SystemTime;

#[derive(Debug, Deserialize)]
pub(crate) struct ExchangeBody {
    /// One-time login code from `#login=<code>`.
    pub code: String,
}

/// `POST /operator/session/exchange` — consume a one-time login code.
pub(crate) async fn exchange_login_code(
    State(state): State<HttpState>,
    request: Request,
) -> Response {
    let request_id = request_id_from(&request);
    if !same_origin_csrf(&request) {
        return csrf_rejected(request_id);
    }
    let Some(auth) = state.dashboard_auth() else {
        return platform_error_response(
            &PlatformError::new(
                ErrorCode::PlatformUnavailable,
                "dashboard sessions are not enabled",
            ),
            request_id,
        );
    };
    let Ok(body) = axum::body::to_bytes(request.into_body(), 4 * 1024).await else {
        return platform_error_response(
            &PlatformError::new(ErrorCode::ConfigInvalid, "request body is invalid"),
            request_id,
        );
    };
    let parsed: ExchangeBody = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => {
            return platform_error_response(
                &PlatformError::new(ErrorCode::ConfigInvalid, "login exchange body is invalid"),
                request_id,
            );
        }
    };
    match auth.exchange_login_code(parsed.code.trim(), SystemTime::now()) {
        Ok(session) => session_response(&session, request_id),
        Err(err) => platform_error_response(&err, request_id),
    }
}

/// `POST /operator/session` — mint a short browser session from a long-lived admin Bearer.
pub(crate) async fn mint_session_from_admin(
    State(state): State<HttpState>,
    request: Request,
) -> Response {
    let request_id = request_id_from(&request);
    if !same_origin_csrf(&request) {
        return csrf_rejected(request_id);
    }
    let Some(auth) = state.dashboard_auth() else {
        return platform_error_response(
            &PlatformError::new(
                ErrorCode::PlatformUnavailable,
                "dashboard sessions are not enabled",
            ),
            request_id,
        );
    };
    let presented = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    let Some(admin) = state.admin_secret() else {
        return platform_error_response(
            &PlatformError::new(
                ErrorCode::AdminAuthRequired,
                "admin authentication is required",
            ),
            request_id,
        );
    };
    if !bearer_matches(presented, admin) {
        return platform_error_response(
            &PlatformError::new(
                ErrorCode::AdminAuthRequired,
                "admin authentication is required",
            ),
            request_id,
        );
    }
    match auth.issue_session_from_admin(SystemTime::now()) {
        Ok(session) => session_response(&session, request_id),
        Err(err) => platform_error_response(&err, request_id),
    }
}

fn session_response(session: &SessionIssue, request_id: RequestId) -> Response {
    let mut response = (
        StatusCode::OK,
        Json(serde_json::json!({
            "session_token": session.session_token,
            "expires_at_ms": session.expires_at_ms,
        })),
    )
        .into_response();
    if let Ok(value) = HeaderValue::from_str(&request_id.to_string()) {
        response.headers_mut().insert(REQUEST_ID_HEADER, value);
    }
    response
}

fn csrf_rejected(request_id: RequestId) -> Response {
    platform_error_response(
        &PlatformError::new(
            ErrorCode::AdminAuthRequired,
            "same-origin session exchange is required",
        ),
        request_id,
    )
}

fn request_id_from(request: &Request) -> RequestId {
    request
        .extensions()
        .get::<RequestId>()
        .copied()
        .unwrap_or_else(RequestId::generate)
}

/// Accept browser same-origin POSTs; fail closed without Origin / Sec-Fetch-Site.
pub(crate) fn same_origin_csrf(request: &Request) -> bool {
    if let Some(site) = request
        .headers()
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok())
    {
        if site.eq_ignore_ascii_case("same-origin") {
            return true;
        }
        if !site.eq_ignore_ascii_case("none") {
            return false;
        }
    }
    let Some(host) = request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let Some(origin) = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    origin_matches_host(origin, host)
}

fn origin_matches_host(origin: &str, host: &str) -> bool {
    let Ok(url) = url::Url::parse(origin) else {
        return false;
    };
    let Some(origin_host) = url.host_str() else {
        return false;
    };
    let origin_authority = match url.port() {
        Some(port) => format!("{origin_host}:{port}"),
        None => origin_host.to_owned(),
    };
    origin_authority.eq_ignore_ascii_case(host)
}

/// Return true when Authorization carries a live Dashboard browser session.
#[must_use]
pub(crate) fn dashboard_session_authorized(state: &HttpState, request: &Request) -> bool {
    let Some(auth) = state.dashboard_auth() else {
        return false;
    };
    let Some(token) = bearer_token(
        request
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
    ) else {
        return false;
    };
    auth.session_valid(token, SystemTime::now())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dashboard_auth::DashboardAuth;
    use crate::health::HealthCoordinator;
    use crate::metrics::MetricsRegistry;
    use axum::body::Body;
    use axum::extract::State;
    use axum::http::{Method, Request, header};
    use open_compute_core::config::MetricsConfig;
    use open_compute_core::{SecretString, StartupId};
    use std::sync::Arc;

    fn metrics() -> Arc<MetricsRegistry> {
        Arc::new(MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd").unwrap())
    }

    #[test]
    fn origin_host_match_ignores_default_scheme_port() {
        assert!(origin_matches_host(
            "http://127.0.0.1:8787",
            "127.0.0.1:8787"
        ));
        assert!(!origin_matches_host(
            "http://evil.example",
            "127.0.0.1:8787"
        ));
    }

    #[test]
    fn same_origin_csrf_accepts_sec_fetch_site_and_origin() {
        let same = Request::builder()
            .method(Method::POST)
            .uri("/operator/session")
            .header("sec-fetch-site", "same-origin")
            .body(Body::empty())
            .unwrap();
        assert!(same_origin_csrf(&same));
        let cross = Request::builder()
            .method(Method::POST)
            .uri("/operator/session")
            .header("sec-fetch-site", "cross-site")
            .body(Body::empty())
            .unwrap();
        assert!(!same_origin_csrf(&cross));
        let via_origin = Request::builder()
            .method(Method::POST)
            .uri("/operator/session")
            .header(header::HOST, "127.0.0.1:8787")
            .header(header::ORIGIN, "http://127.0.0.1:8787")
            .body(Body::empty())
            .unwrap();
        assert!(same_origin_csrf(&via_origin));
        let missing = Request::builder()
            .method(Method::POST)
            .uri("/operator/session")
            .body(Body::empty())
            .unwrap();
        assert!(!same_origin_csrf(&missing));
    }

    #[tokio::test]
    async fn exchange_and_mint_session_cover_auth_paths() {
        let auth = Arc::new(DashboardAuth::new(StartupId::generate()));
        let issued = auth.issue_login_code(SystemTime::now()).unwrap();
        let state = HttpState::for_test(
            HealthCoordinator::new(),
            metrics(),
            false,
            Some(SecretString::new("admin-secret")),
        )
        .with_dashboard_auth(auth.clone());

        let csrf = Request::builder()
            .method(Method::POST)
            .uri("/operator/session/exchange")
            .header(header::HOST, "127.0.0.1:8787")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(format!(r#"{{"code":"{}"}}"#, issued.code)))
            .unwrap();
        let rejected = exchange_login_code(State(state.clone()), csrf).await;
        assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);

        let ok_req = Request::builder()
            .method(Method::POST)
            .uri("/operator/session/exchange")
            .header(header::HOST, "127.0.0.1:8787")
            .header(header::ORIGIN, "http://127.0.0.1:8787")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(format!(r#"{{"code":"{}"}}"#, issued.code)))
            .unwrap();
        let accepted = exchange_login_code(State(state.clone()), ok_req).await;
        assert_eq!(accepted.status(), StatusCode::OK);

        let bad_json = Request::builder()
            .method(Method::POST)
            .uri("/operator/session/exchange")
            .header(header::HOST, "127.0.0.1:8787")
            .header(header::ORIGIN, "http://127.0.0.1:8787")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("{"))
            .unwrap();
        assert_eq!(
            exchange_login_code(State(state.clone()), bad_json)
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );

        let mint = Request::builder()
            .method(Method::POST)
            .uri("/operator/session")
            .header(header::HOST, "127.0.0.1:8787")
            .header(header::ORIGIN, "http://127.0.0.1:8787")
            .header(header::AUTHORIZATION, "Bearer admin-secret")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            mint_session_from_admin(State(state.clone()), mint)
                .await
                .status(),
            StatusCode::OK
        );

        let no_auth = Request::builder()
            .method(Method::POST)
            .uri("/operator/session")
            .header(header::HOST, "127.0.0.1:8787")
            .header(header::ORIGIN, "http://127.0.0.1:8787")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            mint_session_from_admin(State(state.clone()), no_auth)
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );

        let bare = HttpState::for_test(HealthCoordinator::new(), metrics(), false, None);
        let req = Request::builder()
            .method(Method::POST)
            .uri("/operator/session/exchange")
            .header("sec-fetch-site", "same-origin")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"code":"x"}"#))
            .unwrap();
        assert_eq!(
            exchange_login_code(State(bare.clone()), req).await.status(),
            StatusCode::SERVICE_UNAVAILABLE
        );

        let with_session = state.clone();
        let session = auth.issue_session_from_admin(SystemTime::now()).unwrap();
        let authorized = Request::builder()
            .method(Method::GET)
            .uri("/operator/")
            .header(
                header::AUTHORIZATION,
                format!("Bearer {}", session.session_token),
            )
            .body(Body::empty())
            .unwrap();
        assert!(dashboard_session_authorized(&with_session, &authorized));
        assert!(!dashboard_session_authorized(&bare, &authorized));
    }

    #[tokio::test]
    async fn exchange_rejects_oversized_body_and_invalid_code() {
        let auth = Arc::new(DashboardAuth::new(StartupId::generate()));
        let state = HttpState::for_test(
            HealthCoordinator::new(),
            metrics(),
            false,
            Some(SecretString::new("admin-secret")),
        )
        .with_dashboard_auth(auth);

        let oversized = Request::builder()
            .method(Method::POST)
            .uri("/operator/session/exchange")
            .header("sec-fetch-site", "same-origin")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(vec![b'a'; 5 * 1024]))
            .unwrap();
        assert_eq!(
            exchange_login_code(State(state.clone()), oversized)
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );

        let bad_code = Request::builder()
            .method(Method::POST)
            .uri("/operator/session/exchange")
            .header("sec-fetch-site", "same-origin")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"code":"deadbeef"}"#))
            .unwrap();
        assert_eq!(
            exchange_login_code(State(state.clone()), bad_code)
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );

        let mint_csrf = Request::builder()
            .method(Method::POST)
            .uri("/operator/session")
            .header(header::AUTHORIZATION, "Bearer admin-secret")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            mint_session_from_admin(State(state), mint_csrf)
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }
}
