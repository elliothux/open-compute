//! Strict JSON request body boundary for Worker v4 handlers.

use crate::cloudflare_v4::V4Error;
use axum::body::to_bytes;
use axum::extract::Request;
use serde::Deserialize;

const MAX_JSON_BODY: usize = 1024 * 1024;

pub(super) async fn json_body<T: for<'de> Deserialize<'de>>(
    request: Request,
) -> Result<T, V4Error> {
    json_body_with_limit(request, MAX_JSON_BODY).await
}

pub(super) async fn json_body_with_limit<T: for<'de> Deserialize<'de>>(
    request: Request,
    limit: usize,
) -> Result<T, V4Error> {
    let mut content_types = request
        .headers()
        .get_all(axum::http::header::CONTENT_TYPE)
        .iter();
    let content_type = content_types
        .next()
        .and_then(|value| value.to_str().ok())
        .ok_or(V4Error::InvalidRequest)?;
    if content_types.next().is_some() || !valid_json_content_type(content_type) {
        return Err(V4Error::InvalidRequest);
    }
    let bytes = to_bytes(request.into_body(), limit)
        .await
        .map_err(|_| V4Error::InvalidRequest)?;
    serde_json::from_slice(&bytes).map_err(|_| V4Error::InvalidRequest)
}

fn valid_json_content_type(value: &str) -> bool {
    let mut parts = value.split(';');
    if parts.next().map(str::trim) != Some("application/json") {
        return false;
    }
    let mut saw_charset = false;
    for parameter in parts {
        let Some((name, value)) = parameter.trim().split_once('=') else {
            return false;
        };
        if saw_charset
            || !name.trim().eq_ignore_ascii_case("charset")
            || !value.trim().eq_ignore_ascii_case("utf-8")
        {
            return false;
        }
        saw_charset = true;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::valid_json_content_type;

    #[test]
    fn accepts_only_the_fixed_sdk_json_media_type() {
        assert!(valid_json_content_type("application/json"));
        assert!(valid_json_content_type("application/json; charset=utf-8"));
        for value in [
            "text/plain",
            "application/json-patch+json",
            "application/json; boundary=x",
            "application/json; charset=utf-8; charset=utf-8",
        ] {
            assert!(!valid_json_content_type(value));
        }
    }
}
