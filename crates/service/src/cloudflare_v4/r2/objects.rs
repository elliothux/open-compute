//! Official R2 raw-object HTTP operations.

use super::super::storage::{iso_timestamp, require_no_query, require_query_fields};
use super::{attach_request_id, bucket, header_text, jurisdiction};
use crate::cloudflare_v4::{
    V4Error, V4Permission, error_response, request_context, success_response,
};
use crate::http::HttpState;
use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use open_compute_artifacts::{R2HttpMetadata, R2PutOptions, R2StorageClass, UserObjectKey};

pub(super) async fn list(request: Request) -> Response {
    let context = match request_context(&request) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Err(error) = context.require(V4Permission::Read) {
        return error_response(error, context.request_id());
    }
    if let Err(error) = require_query_fields(
        &request,
        &["per_page", "prefix", "delimiter", "cursor", "start_after"],
    ) {
        return error_response(error, context.request_id());
    }
    error_response(V4Error::Unsupported, context.request_id())
}

pub(super) async fn get(
    State(state): State<HttpState>,
    Path((account_id, bucket_name, object_key)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let (context, account_id, bucket) =
        match bucket(&state, &request, &account_id, &bucket_name, false) {
            Ok(value) => value,
            Err(response) => return response,
        };
    if let Err(error) = require_no_query(&request) {
        return error_response(error, context.request_id());
    }
    let key = match UserObjectKey::parse(&object_key) {
        Ok(value) => value,
        Err(error) => return error_response(V4Error::from(&error), context.request_id()),
    };
    let Some(api) = state.r2_api() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let binding = match api.binding() {
        Ok(value) => value,
        Err(error) => return error_response(V4Error::from(&error), context.request_id()),
    };
    let result = binding
        .operator_object_get(account_id, bucket.resource.id, &key)
        .await;
    let Some((metadata, body)) = (match result {
        Ok(value) => value,
        Err(error) => return error_response(V4Error::from(&error), context.request_id()),
    }) else {
        return error_response(V4Error::NotFound, context.request_id());
    };
    let if_none_match = match header_text(request.headers(), "if-none-match") {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let if_modified_since = match header_text(request.headers(), "if-modified-since") {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let modified_since = match if_modified_since.as_deref() {
        Some(value) => match httpdate::parse_http_date(value) {
            Ok(value) => Some(value),
            Err(_) => return error_response(V4Error::InvalidRequest, context.request_id()),
        },
        None => None,
    };
    let uploaded = match u64::try_from(metadata.uploaded).ok().and_then(|millis| {
        std::time::UNIX_EPOCH.checked_add(std::time::Duration::from_millis(millis))
    }) {
        Some(value) => value,
        None => return error_response(V4Error::Internal, context.request_id()),
    };
    let not_modified = if_none_match
        .as_deref()
        .is_some_and(|value| etag_matches(value, &metadata.http_etag))
        || if_none_match.is_none() && modified_since.is_some_and(|time| uploaded <= time);
    if not_modified {
        let mut response = StatusCode::NOT_MODIFIED.into_response();
        if let Err(error) = add_headers(&mut response, &metadata) {
            return error_response(error, context.request_id());
        }
        response.headers_mut().remove(header::CONTENT_LENGTH);
        response.headers_mut().remove(header::CONTENT_TYPE);
        attach_request_id(&mut response, context.request_id());
        return response;
    }
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = StatusCode::OK;
    if let Err(error) = add_headers(&mut response, &metadata) {
        return error_response(error, context.request_id());
    }
    attach_request_id(&mut response, context.request_id());
    response
}

fn weak_etag(value: &str) -> &str {
    value.strip_prefix("W/").unwrap_or(value)
}

fn etag_matches(condition: &str, current: &str) -> bool {
    condition.split(',').any(|candidate| {
        let candidate = candidate.trim();
        candidate == "*" || weak_etag(candidate) == weak_etag(current)
    })
}

pub(super) async fn put(
    State(state): State<HttpState>,
    Path((account_id, bucket_name, object_key)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let (context, account_id, bucket) =
        match bucket(&state, &request, &account_id, &bucket_name, true) {
            Ok(value) => value,
            Err(response) => return response,
        };
    if let Err(error) = require_no_query(&request) {
        return error_response(error, context.request_id());
    }
    let key = match UserObjectKey::parse(&object_key) {
        Ok(value) => value,
        Err(error) => return error_response(V4Error::from(&error), context.request_id()),
    };
    let (options, expected_length) = match put_options(request.headers()) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let Some(api) = state.r2_api() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let binding = match api.binding() {
        Ok(value) => value,
        Err(error) => return error_response(V4Error::from(&error), context.request_id()),
    };
    let request_id = context.request_id();
    match binding
        .operator_object_put(
            account_id,
            bucket.resource.id,
            &key,
            request_id,
            options,
            expected_length,
            request.into_body(),
        )
        .await
    {
        Ok(Some(metadata)) => {
            let uploaded = match iso_timestamp(metadata.uploaded) {
                Ok(value) => value,
                Err(error) => return error_response(error, request_id),
            };
            success_response(
                context,
                serde_json::json!({
                    "etag": metadata.etag,
                    "key": metadata.key,
                    "size": metadata.size.to_string(),
                    "storage_class": metadata.storage_class,
                    "uploaded": uploaded,
                    "version": metadata.version,
                }),
            )
        }
        Ok(None) => error_response(V4Error::Conflict, request_id),
        Err(error) => error_response(V4Error::from(&error), request_id),
    }
}

pub(super) async fn delete(
    State(state): State<HttpState>,
    Path((account_id, bucket_name, object_key)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let (context, account_id, bucket) =
        match bucket(&state, &request, &account_id, &bucket_name, true) {
            Ok(value) => value,
            Err(response) => return response,
        };
    if let Err(error) = require_no_query(&request) {
        return error_response(error, context.request_id());
    }
    let key = match UserObjectKey::parse(&object_key) {
        Ok(value) => value,
        Err(error) => return error_response(V4Error::from(&error), context.request_id()),
    };
    if let Err(error) = data_catalog_check(request.headers()) {
        return error_response(error, context.request_id());
    }
    let Some(api) = state.r2_api() else {
        return error_response(V4Error::Unavailable, context.request_id());
    };
    let binding = match api.binding() {
        Ok(value) => value,
        Err(error) => return error_response(V4Error::from(&error), context.request_id()),
    };
    match binding
        .operator_object_delete(account_id, bucket.resource.id, &key)
        .await
    {
        Ok(true) => success_response(context, serde_json::json!({ "key": key.as_str() })),
        Ok(false) => error_response(V4Error::NotFound, context.request_id()),
        Err(error) => error_response(V4Error::from(&error), context.request_id()),
    }
}

fn put_options(headers: &HeaderMap) -> Result<(R2PutOptions, Option<u64>), V4Error> {
    jurisdiction(headers)?;
    data_catalog_check(headers)?;
    let storage_class = header_text(headers, "cf-r2-storage-class")?
        .map(|value| R2StorageClass::parse(&value))
        .transpose()
        .map_err(|error| V4Error::from(&error))?
        .unwrap_or_default();
    let text = |name: &'static str| header_text(headers, name);
    let cache_expiry = text("expires")?
        .map(|value| {
            httpdate::parse_http_date(&value)
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .and_then(|duration| i64::try_from(duration.as_millis()).ok())
                .ok_or(V4Error::InvalidRequest)
        })
        .transpose()?;
    let content_length = text("content-length")?
        .map(|value| value.parse().map_err(|_| V4Error::InvalidRequest))
        .transpose()?;
    Ok((
        R2PutOptions {
            http_metadata: R2HttpMetadata {
                content_type: text("content-type")?,
                content_language: text("content-language")?,
                content_disposition: text("content-disposition")?,
                content_encoding: text("content-encoding")?,
                cache_control: text("cache-control")?,
                cache_expiry,
            },
            storage_class,
            ..R2PutOptions::default()
        },
        content_length,
    ))
}

fn add_headers(
    response: &mut Response,
    metadata: &open_compute_artifacts::R2ObjectMetadata,
) -> Result<(), V4Error> {
    response.headers_mut().insert(
        header::ETAG,
        HeaderValue::from_str(&metadata.http_etag).map_err(|_| V4Error::Internal)?,
    );
    response.headers_mut().insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&metadata.size.to_string()).map_err(|_| V4Error::Internal)?,
    );
    response.headers_mut().insert(
        "cf-r2-storage-class",
        HeaderValue::from_str(&metadata.storage_class).map_err(|_| V4Error::Internal)?,
    );
    let duration = u64::try_from(metadata.uploaded)
        .map(std::time::Duration::from_millis)
        .map_err(|_| V4Error::Internal)?;
    let time = std::time::UNIX_EPOCH
        .checked_add(duration)
        .ok_or(V4Error::Internal)?;
    response.headers_mut().insert(
        header::LAST_MODIFIED,
        HeaderValue::from_str(&httpdate::fmt_http_date(time)).map_err(|_| V4Error::Internal)?,
    );
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    if let Some(http) = &metadata.http_metadata {
        for (name, value) in [
            (header::CONTENT_TYPE, http.content_type.as_deref()),
            (header::CONTENT_LANGUAGE, http.content_language.as_deref()),
            (
                header::CONTENT_DISPOSITION,
                http.content_disposition.as_deref(),
            ),
            (header::CONTENT_ENCODING, http.content_encoding.as_deref()),
            (header::CACHE_CONTROL, http.cache_control.as_deref()),
        ] {
            if let Some(value) = value {
                response.headers_mut().insert(
                    name,
                    HeaderValue::from_str(value).map_err(|_| V4Error::Internal)?,
                );
            }
        }
        if let Some(expiry) = http.cache_expiry {
            let duration = u64::try_from(expiry)
                .map(std::time::Duration::from_millis)
                .map_err(|_| V4Error::Internal)?;
            let time = std::time::UNIX_EPOCH
                .checked_add(duration)
                .ok_or(V4Error::Internal)?;
            response.headers_mut().insert(
                header::EXPIRES,
                HeaderValue::from_str(&httpdate::fmt_http_date(time))
                    .map_err(|_| V4Error::Internal)?,
            );
        }
    }
    Ok(())
}

fn data_catalog_check(headers: &HeaderMap) -> Result<(), V4Error> {
    match header_text(headers, "cf-r2-data-catalog-check")?.as_deref() {
        None | Some("true" | "false") => Ok(()),
        Some(_) => Err(V4Error::InvalidRequest),
    }
}

#[cfg(test)]
mod tests {
    use super::{data_catalog_check, etag_matches, put_options};
    use axum::http::{HeaderMap, HeaderValue};

    #[test]
    fn pinned_wrangler_object_headers_are_preserved_and_validated() {
        let mut headers = HeaderMap::new();
        for (name, value) in [
            ("cache-control", "public, max-age=60"),
            ("content-disposition", "attachment; filename=data.bin"),
            ("content-encoding", "gzip"),
            ("content-language", "en"),
            ("content-length", "7"),
            ("content-type", "application/octet-stream"),
            ("expires", "Wed, 21 Oct 2037 07:28:00 GMT"),
            ("cf-r2-jurisdiction", "default"),
            ("cf-r2-storage-class", "Standard"),
            ("cf-r2-data-catalog-check", "true"),
        ] {
            headers.insert(name, HeaderValue::from_static(value));
        }
        let (options, length) = put_options(&headers).unwrap();
        assert_eq!(length, Some(7));
        assert_eq!(options.storage_class.as_str(), "Standard");
        assert_eq!(
            options.http_metadata.content_type.as_deref(),
            Some("application/octet-stream")
        );
        assert_eq!(
            options.http_metadata.content_language.as_deref(),
            Some("en")
        );
        assert_eq!(
            options.http_metadata.content_encoding.as_deref(),
            Some("gzip")
        );
        assert_eq!(
            options.http_metadata.content_disposition.as_deref(),
            Some("attachment; filename=data.bin")
        );
        assert_eq!(
            options.http_metadata.cache_control.as_deref(),
            Some("public, max-age=60")
        );
        assert!(options.http_metadata.cache_expiry.is_some());
    }

    #[test]
    fn malformed_or_duplicate_control_headers_fail_closed() {
        let mut headers = HeaderMap::new();
        headers.insert("cf-r2-data-catalog-check", HeaderValue::from_static("yes"));
        assert!(data_catalog_check(&headers).is_err());

        let mut headers = HeaderMap::new();
        headers.append("cf-r2-jurisdiction", HeaderValue::from_static("default"));
        headers.append("cf-r2-jurisdiction", HeaderValue::from_static("default"));
        assert!(put_options(&headers).is_err());
    }

    #[test]
    fn conditional_get_uses_star_and_weak_etag_comparison() {
        assert!(etag_matches("*", "\"abc\""));
        assert!(etag_matches("W/\"abc\"", "\"abc\""));
        assert!(etag_matches("\"other\", W/\"abc\"", "\"abc\""));
        assert!(!etag_matches("\"other\"", "\"abc\""));
    }
}
