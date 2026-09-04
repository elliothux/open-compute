//! Official KV raw-value and metadata operations.

use super::{MAX_VALUE_BODY, namespace};
use crate::binding_backend::KvBindingExecutor;
use crate::cloudflare_v4::storage::{context, require_no_query, strict_query};
use crate::cloudflare_v4::{HttpError, V4Error, V4Permission, error_response, success_response};
use crate::http::{HttpState, REQUEST_ID_HEADER};
use crate::kv_backend::{KvCommand, KvCommandResult};
use crate::resource_binding::management_binding;
use axum::body::to_bytes;
use axum::extract::{FromRequest, Multipart, Path, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use open_compute_core::{BindingKind, RequestId, ResourceId};
use open_compute_storage::KV_MAX_METADATA_BYTES;
use serde_json::Value;

pub(super) async fn get(
    State(state): State<HttpState>,
    Path((account_id, namespace_id, key_name)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let boundary = match context(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    if let Err(error) = require_no_query(&request) {
        return error_response(error, boundary.request_id());
    }
    let (context, entry) = match entry(&state, &request, &account_id, &namespace_id, &key_name) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    let Some(entry) = entry else {
        return error_response(V4Error::NotFound, context.request_id());
    };
    let mut response = (StatusCode::OK, entry.value).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    if let Some(expiration) = entry.expires_at_ms {
        let Ok(expiration) = u64::try_from(expiration / 1000) else {
            return error_response(V4Error::Internal, context.request_id());
        };
        let Ok(value) = HeaderValue::from_str(&expiration.to_string()) else {
            return error_response(V4Error::Internal, context.request_id());
        };
        response.headers_mut().insert("expiration", value);
    }
    attach_request_id(&mut response, context.request_id());
    response
}

pub(super) async fn metadata(
    State(state): State<HttpState>,
    Path((account_id, namespace_id, key_name)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let boundary = match context(&request, V4Permission::Read) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    if let Err(error) = require_no_query(&request) {
        return error_response(error, boundary.request_id());
    }
    let (context, entry) = match entry(&state, &request, &account_id, &namespace_id, &key_name) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    let Some(entry) = entry else {
        return error_response(V4Error::NotFound, context.request_id());
    };
    let metadata = match entry.metadata_json.as_deref() {
        Some(value) => match serde_json::from_slice::<Value>(value) {
            Ok(value) => value,
            Err(_) => return error_response(V4Error::Internal, context.request_id()),
        },
        None => Value::Null,
    };
    success_response(context, metadata)
}

#[derive(Default)]
struct ExpirationQuery {
    expiration: Option<u64>,
    expiration_ttl: Option<u64>,
}

pub(super) async fn put(
    State(state): State<HttpState>,
    Path((account_id, namespace_id, key_name)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let (context, account_id, record) =
        match namespace(&state, &request, &account_id, &namespace_id, true) {
            Ok(value) => value,
            Err(response) => return response.into_response(),
        };
    let query = match expiration_query(&request) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let mut content_types = request.headers().get_all(header::CONTENT_TYPE).iter();
    let content_type = match content_types.next() {
        Some(value) => match value.to_str() {
            Ok(value) => Some(value.to_owned()),
            Err(_) => return error_response(V4Error::InvalidRequest, context.request_id()),
        },
        None => None,
    };
    if content_types.next().is_some() {
        return error_response(V4Error::InvalidRequest, context.request_id());
    }
    let multipart = match content_type.as_deref() {
        Some(value) if value.starts_with("multipart/form-data;") => true,
        Some(value) if value.starts_with("multipart/form-data") => {
            return error_response(V4Error::InvalidRequest, context.request_id());
        }
        _ => false,
    };
    let (value, metadata, metadata_present) = if multipart {
        match read_multipart(request).await {
            Ok(value) => value,
            Err(error) => return error_response(error, context.request_id()),
        }
    } else {
        match to_bytes(request.into_body(), MAX_VALUE_BODY).await {
            Ok(value) => (value.to_vec(), None, false),
            Err(_) => return error_response(V4Error::InvalidRequest, context.request_id()),
        }
    };
    let command = KvCommand::Put {
        key: key_name,
        value,
        expiration: query.expiration,
        expiration_ttl: query.expiration_ttl,
        metadata,
        metadata_present,
    };
    mutate(&state, context, account_id, record.resource.id, command)
}

pub(super) async fn delete(
    State(state): State<HttpState>,
    Path((account_id, namespace_id, key_name)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let (context, account_id, record) =
        match namespace(&state, &request, &account_id, &namespace_id, true) {
            Ok(value) => value,
            Err(response) => return response.into_response(),
        };
    if let Err(error) = require_no_query(&request) {
        return error_response(error, context.request_id());
    }
    mutate(
        &state,
        context,
        account_id,
        record.resource.id,
        KvCommand::Delete { key: key_name },
    )
}

fn entry(
    state: &HttpState,
    request: &Request,
    account_id: &str,
    namespace_id: &str,
    key: &str,
) -> Result<
    (
        crate::cloudflare_v4::V4RequestContext,
        Option<open_compute_storage::KvEntry>,
    ),
    HttpError,
> {
    let (context, account_id, record) = namespace(state, request, account_id, namespace_id, false)?;
    let result = execute(
        state,
        account_id,
        record.resource.id,
        KvCommand::Get {
            keys: vec![key.to_owned()],
            cache_ttl: None,
        },
    )
    .map_err(|error| HttpError::from_response(error_response(error, context.request_id())))?;
    match result {
        KvCommandResult::Entries(mut entries) if entries.len() == 1 => {
            Ok((context, entries.pop().flatten()))
        }
        _ => Err(HttpError::from_response(error_response(
            V4Error::Internal,
            context.request_id(),
        ))),
    }
}

pub(super) fn execute(
    state: &HttpState,
    account_id: open_compute_core::AccountId,
    resource_id: ResourceId,
    command: KvCommand,
) -> Result<KvCommandResult, V4Error> {
    let api = state.kv_api().ok_or(V4Error::Unavailable)?;
    let binding = management_binding(
        api.storage(),
        account_id,
        resource_id,
        BindingKind::KvNamespace,
    )
    .map_err(|error| V4Error::from(&error))?;
    api.executor()
        .execute(&binding, command)
        .map_err(|error| V4Error::from(&error))
}

fn mutate(
    state: &HttpState,
    context: crate::cloudflare_v4::V4RequestContext,
    account_id: open_compute_core::AccountId,
    resource_id: ResourceId,
    command: KvCommand,
) -> Response {
    match execute(state, account_id, resource_id, command) {
        Ok(KvCommandResult::Mutation) => success_response(context, ()),
        Ok(_) => error_response(V4Error::Internal, context.request_id()),
        Err(error) => error_response(error, context.request_id()),
    }
}

async fn read_multipart(request: Request) -> Result<(Vec<u8>, Option<Value>, bool), V4Error> {
    let mut multipart = Multipart::from_request(request, &())
        .await
        .map_err(|_| V4Error::InvalidRequest)?;
    let mut value = None;
    let mut metadata = None;
    let mut metadata_present = false;
    let mut direct_metadata = false;
    let mut metadata_bytes = 0_usize;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|_| V4Error::InvalidRequest)?
    {
        let name = field.name().map(str::to_owned);
        let content_type = field.content_type().map(str::to_owned);
        match name.as_deref() {
            Some("value") if value.is_none() => {
                let bytes = field.bytes().await.map_err(|_| V4Error::InvalidRequest)?;
                if bytes.len() > MAX_VALUE_BODY {
                    return Err(V4Error::Official(
                        crate::cloudflare_v4::wire::V4OfficialError::RequestTooLarge,
                    ));
                }
                value = Some(bytes.to_vec());
            }
            Some("metadata") if !metadata_present => {
                if !valid_metadata_content_type(content_type.as_deref(), true) {
                    return Err(V4Error::InvalidRequest);
                }
                let bytes = field.bytes().await.map_err(|_| V4Error::InvalidRequest)?;
                metadata_bytes = metadata_bytes
                    .checked_add(bytes.len())
                    .ok_or(V4Error::InvalidRequest)?;
                if metadata_bytes > KV_MAX_METADATA_BYTES {
                    return Err(V4Error::InvalidRequest);
                }
                metadata =
                    Some(serde_json::from_slice(&bytes).map_err(|_| V4Error::InvalidRequest)?);
                metadata_present = true;
                direct_metadata = true;
            }
            Some(name) if name.starts_with("metadata[") && !direct_metadata => {
                if !valid_metadata_content_type(content_type.as_deref(), false) {
                    return Err(V4Error::InvalidRequest);
                }
                let path = metadata_path(name)?;
                let bytes = field.bytes().await.map_err(|_| V4Error::InvalidRequest)?;
                metadata_bytes = metadata_bytes
                    .checked_add(bytes.len())
                    .ok_or(V4Error::InvalidRequest)?;
                if metadata_bytes > KV_MAX_METADATA_BYTES {
                    return Err(V4Error::InvalidRequest);
                }
                let text =
                    String::from_utf8(bytes.to_vec()).map_err(|_| V4Error::InvalidRequest)?;
                let metadata = metadata.get_or_insert(Value::Null);
                insert_metadata(metadata, &path, Value::String(text))?;
                metadata_present = true;
            }
            _ => return Err(V4Error::InvalidRequest),
        }
    }
    if let Some(metadata) = metadata.as_ref() {
        open_compute_storage::canonical_metadata(metadata).map_err(|_| V4Error::InvalidRequest)?;
    }
    Ok((
        value.ok_or(V4Error::InvalidRequest)?,
        metadata,
        metadata_present,
    ))
}

fn valid_metadata_content_type(content_type: Option<&str>, direct_json: bool) -> bool {
    match content_type {
        None => true,
        Some(value) => {
            let mut parts = value.split(';').map(str::trim);
            let media_type = parts.next();
            let charset_valid = parts.all(|part| part.eq_ignore_ascii_case("charset=utf-8"));
            charset_valid
                && (media_type == Some("text/plain")
                    || direct_json && media_type == Some("application/json"))
        }
    }
}

fn metadata_path(name: &str) -> Result<Vec<String>, V4Error> {
    let mut remainder = name
        .strip_prefix("metadata")
        .ok_or(V4Error::InvalidRequest)?;
    let mut path = Vec::new();
    while !remainder.is_empty() {
        let value = remainder
            .strip_prefix('[')
            .and_then(|value| value.split_once(']'))
            .ok_or(V4Error::InvalidRequest)?;
        if value.0.contains(['[', ']']) {
            return Err(V4Error::InvalidRequest);
        }
        path.push(value.0.to_owned());
        remainder = value.1;
    }
    if path.is_empty()
        || path
            .iter()
            .enumerate()
            .any(|(index, segment)| segment.is_empty() && index + 1 != path.len())
    {
        return Err(V4Error::InvalidRequest);
    }
    Ok(path)
}

fn insert_metadata(target: &mut Value, path: &[String], value: Value) -> Result<(), V4Error> {
    let (segment, rest) = path.split_first().ok_or(V4Error::InvalidRequest)?;
    if segment.is_empty() {
        if !rest.is_empty() {
            return Err(V4Error::InvalidRequest);
        }
        if target.is_null() {
            *target = Value::Array(Vec::new());
        }
        target
            .as_array_mut()
            .ok_or(V4Error::InvalidRequest)?
            .push(value);
        return Ok(());
    }
    if target.is_null() {
        *target = Value::Object(serde_json::Map::new());
    }
    let object = target.as_object_mut().ok_or(V4Error::InvalidRequest)?;
    if rest.is_empty() {
        if object.insert(segment.clone(), value).is_some() {
            return Err(V4Error::InvalidRequest);
        }
        return Ok(());
    }
    let child = object.entry(segment.clone()).or_insert(Value::Null);
    insert_metadata(child, rest, value)
}

fn expiration_query(request: &Request) -> Result<ExpirationQuery, V4Error> {
    let mut values = strict_query(request)?;
    let parse = |value: Option<String>| {
        value
            .map(|value| value.parse().map_err(|_| V4Error::InvalidRequest))
            .transpose()
    };
    let expiration = parse(values.remove("expiration"))?;
    let expiration_ttl = parse(values.remove("expiration_ttl"))?;
    if !values.is_empty() || expiration_ttl.is_some_and(|value| value < 60) {
        return Err(V4Error::InvalidRequest);
    }
    Ok(ExpirationQuery {
        expiration,
        expiration_ttl,
    })
}

fn attach_request_id(response: &mut Response, request_id: RequestId) {
    if let Ok(value) = HeaderValue::from_str(&request_id.to_string()) {
        response.headers_mut().insert(REQUEST_ID_HEADER, value);
    }
}

#[cfg(test)]
mod tests {
    use super::{insert_metadata, metadata_path};
    use serde_json::json;

    #[test]
    fn sdk_metadata_paths_reject_duplicates_and_hierarchy_conflicts() {
        let mut value = serde_json::Value::Null;
        insert_metadata(
            &mut value,
            &metadata_path("metadata[nested][key]").unwrap(),
            json!("v"),
        )
        .unwrap();
        assert_eq!(value, json!({"nested": {"key": "v"}}));
        assert!(
            insert_metadata(
                &mut value,
                &metadata_path("metadata[nested][key]").unwrap(),
                json!("other")
            )
            .is_err()
        );
        assert!(
            insert_metadata(
                &mut value,
                &metadata_path("metadata[nested][key][child]").unwrap(),
                json!("other")
            )
            .is_err()
        );
    }
}
