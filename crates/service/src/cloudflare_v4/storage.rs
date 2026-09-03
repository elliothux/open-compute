//! Shared account scope, public resource identifiers, and request parsing for storage adapters.

use super::accounts::{AccountAuthority, V4ResourceKind};
use super::{V4Error, V4Permission, V4RequestContext, error_response, request_context};
use crate::http::HttpState;
use axum::body::to_bytes;
use axum::extract::Request;
use axum::response::Response;
use open_compute_core::{AccountId, RequestId, ResourceId};
use serde::de::DeserializeOwned;
use std::collections::BTreeMap;

pub(super) const MAX_JSON_BODY: usize = 1024 * 1024;

pub(super) fn context(
    request: &Request,
    permission: V4Permission,
) -> Result<V4RequestContext, Response> {
    let context = request_context(request)?;
    context
        .require(permission)
        .map_err(|error| error_response(error, context.request_id()))?;
    Ok(context)
}

pub(super) fn account(state: &HttpState, public_id: &str) -> Result<AccountId, V4Error> {
    state
        .cloudflare_v4_account()
        .ok_or(V4Error::Unavailable)?
        .resolve(public_id)
}

pub(super) fn resolve_resource_id<'a, T: 'a>(
    authority: &AccountAuthority,
    kind: V4ResourceKind,
    public_id: &str,
    records: impl IntoIterator<Item = &'a T>,
    resource: impl Fn(&T) -> ResourceId,
) -> Result<ResourceId, V4Error> {
    if public_id.len() != 32 || !public_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(V4Error::NotFound);
    }
    records
        .into_iter()
        .map(resource)
        .find(|id| authority.matches_public_resource_id(kind, *id, public_id))
        .ok_or(V4Error::NotFound)
}

pub(super) async fn json<T: DeserializeOwned>(
    request: Request,
    request_id: RequestId,
) -> Result<T, Response> {
    json_with_limit(request, request_id, MAX_JSON_BODY).await
}

pub(super) async fn json_with_limit<T: DeserializeOwned>(
    request: Request,
    request_id: RequestId,
    limit: usize,
) -> Result<T, Response> {
    let mut content_types = request
        .headers()
        .get_all(axum::http::header::CONTENT_TYPE)
        .iter();
    let content_type = content_types.next();
    let valid_content_type = content_types.next().is_none()
        && content_type
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                let mut parts = value.split(';').map(str::trim);
                parts.next() == Some("application/json")
                    && parts.all(|part| part.eq_ignore_ascii_case("charset=utf-8"))
            });
    if !valid_content_type {
        return Err(error_response(V4Error::InvalidRequest, request_id));
    }
    let bytes = to_bytes(request.into_body(), limit)
        .await
        .map_err(|_| error_response(V4Error::InvalidRequest, request_id))?;
    serde_json::from_slice(&bytes).map_err(|_| error_response(V4Error::InvalidRequest, request_id))
}

pub(super) fn now_ms() -> Result<i64, V4Error> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| V4Error::Internal)
        .and_then(|duration| i64::try_from(duration.as_millis()).map_err(|_| V4Error::Internal))
}

pub(super) fn iso_timestamp(timestamp_ms: i64) -> Result<String, V4Error> {
    jiff::Timestamp::from_millisecond(timestamp_ms)
        .map(|timestamp| timestamp.to_string())
        .map_err(|_| V4Error::Internal)
}

pub(super) fn strict_query(request: &Request) -> Result<BTreeMap<String, String>, V4Error> {
    let mut result = BTreeMap::new();
    let Some(query) = request.uri().query() else {
        return Ok(result);
    };
    for field in query.split('&') {
        if field.is_empty() {
            return Err(V4Error::InvalidRequest);
        }
        let (key, value) = field.split_once('=').unwrap_or((field, ""));
        let key = decode_query_component(key)?;
        let value = decode_query_component(value)?;
        if key.is_empty() || result.insert(key, value).is_some() {
            return Err(V4Error::InvalidRequest);
        }
    }
    Ok(result)
}

pub(super) fn require_no_query(request: &Request) -> Result<(), V4Error> {
    if strict_query(request)?.is_empty() {
        Ok(())
    } else {
        Err(V4Error::InvalidRequest)
    }
}

pub(super) fn require_query_fields(request: &Request, allowed: &[&str]) -> Result<(), V4Error> {
    let values = strict_query(request)?;
    if values
        .keys()
        .all(|key| allowed.iter().any(|allowed| key == allowed))
    {
        Ok(())
    } else {
        Err(V4Error::InvalidRequest)
    }
}

fn decode_query_component(value: &str) -> Result<String, V4Error> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => decoded.push(b' '),
            b'%' => {
                if index + 2 >= bytes.len() {
                    return Err(V4Error::InvalidRequest);
                }
                let high = hex_digit(bytes[index + 1]).ok_or(V4Error::InvalidRequest)?;
                let low = hex_digit(bytes[index + 2]).ok_or(V4Error::InvalidRequest)?;
                decoded.push((high << 4) | low);
                index += 2;
            }
            byte => decoded.push(byte),
        }
        index += 1;
    }
    String::from_utf8(decoded).map_err(|_| V4Error::InvalidRequest)
}

const fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
