//! Strict Static Assets HTTP wire helpers shared by upload handlers.

use crate::cloudflare_v4::V4Error;
use axum::extract::Request;
use axum::http::header;
use open_compute_core::{ErrorCode, PlatformError};

pub(super) fn valid_bulk_query(query: Option<&str>) -> bool {
    let Some(query) = query else {
        return false;
    };
    let mut pairs = url::form_urlencoded::parse(query.as_bytes());
    matches!(pairs.next(), Some((key, value)) if key == "base64" && value == "true")
        && pairs.next().is_none()
}

pub(super) fn canonical_path(path: &str) -> String {
    if path.starts_with('/') {
        path.to_owned()
    } else {
        format!("/{path}")
    }
}

pub(super) fn base64_size(size: u64) -> u64 {
    size.saturating_add(2).div_euclid(3).saturating_mul(4)
}

pub(super) fn valid_multipart_content_type(request: &Request) -> bool {
    let mut values = request.headers().get_all(header::CONTENT_TYPE).iter();
    let valid = values
        .next()
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("multipart/form-data; boundary="));
    valid && values.next().is_none()
}

pub(super) fn normalize_content_type(value: Option<&str>) -> String {
    match value {
        Some("application/null") | None => "application/octet-stream".to_owned(),
        Some(value) => value.to_owned(),
    }
}

pub(super) fn invalid() -> PlatformError {
    PlatformError::new(
        ErrorCode::AssetManifestInvalid,
        "Static Assets request is invalid",
    )
}

pub(super) fn v4_error(error: V4Error) -> PlatformError {
    PlatformError::new(
        match error {
            V4Error::AuthenticationRequired => ErrorCode::AdminAuthRequired,
            _ => ErrorCode::PlatformUnavailable,
        },
        "Static Assets token could not be processed",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bulk_query_is_closed_and_order_independent_by_construction() {
        assert!(valid_bulk_query(Some("base64=true")));
        for query in [
            None,
            Some(""),
            Some("base64=false"),
            Some("base64=true&base64=true"),
            Some("unknown=true"),
        ] {
            assert!(!valid_bulk_query(query));
        }
    }
}
