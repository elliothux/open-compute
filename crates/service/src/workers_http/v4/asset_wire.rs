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

pub(super) fn single_content_type(request: &Request) -> Result<String, V4Error> {
    let mut values = request.headers().get_all(header::CONTENT_TYPE).iter();
    let value = values
        .next()
        .and_then(|value| value.to_str().ok())
        .ok_or(V4Error::InvalidRequest)?;
    if values.next().is_some() || !valid_media_type(value) {
        return Err(V4Error::InvalidRequest);
    }
    Ok(normalize_content_type(Some(value)))
}

fn valid_media_type(value: &str) -> bool {
    let media_type = value.split(';').next().map(str::trim).unwrap_or_default();
    let Some((kind, subtype)) = media_type.split_once('/') else {
        return false;
    };
    !kind.is_empty()
        && !subtype.is_empty()
        && kind.bytes().chain(subtype.bytes()).all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#' | b'$' | b'&' | b'^' | b'_' | b'.' | b'+' | b'-'
                )
        })
        && value.bytes().all(|byte| !byte.is_ascii_control())
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
