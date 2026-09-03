//! Official AI Search item upload, upsert, and raw download routes.

use super::*;
use crate::cloudflare_v4::storage::{json, require_no_query};
use crate::http::REQUEST_ID_HEADER;
use axum::extract::{FromRequest, Multipart, Path, State};
use axum::http::{HeaderMap, HeaderValue, header};
use bytes::{Bytes, BytesMut};
use serde::Deserialize;
use serde_json::{Map, json};

const MAX_FILE_BYTES: usize = 4 * 1024 * 1024;
const MAX_METADATA_BYTES: usize = 64 * 1024;

pub(super) async fn upload(
    State(state): State<HttpState>,
    Path((public_account, namespace, instance)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let (context, account, api) = match authenticated(
        &state,
        &request,
        V4Permission::ProductWrite,
        &public_account,
    ) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if !valid_namespace(&namespace) || !valid_instance(&instance) {
        return error_response(V4Error::NotFound, context.request_id());
    }
    if let Err(error) = require_no_query(&request) {
        return error_response(error, context.request_id());
    }
    if !valid_multipart_content_type(request.headers()) {
        return error_response(V4Error::InvalidRequest, context.request_id());
    }
    let upload = match read_upload(request).await {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let service = match api.ai_search() {
        Some(value) => value,
        None => return error_response(V4Error::Unavailable, context.request_id()),
    };
    match service
        .official_upload(
            account,
            &namespace,
            &instance,
            context.request_id(),
            upload.filename,
            upload.content_type,
            upload.metadata,
            upload.bytes,
            upload.wait_for_completion,
        )
        .await
    {
        Ok(mut value) => {
            item_namespace(&mut value, &namespace);
            success_response(context, value)
        }
        Err(error) => error_response(V4Error::from(&error), context.request_id()),
    }
}

pub(super) async fn index_by_key(
    State(state): State<HttpState>,
    Path((public_account, namespace, instance)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let (context, account, api) = match authenticated(
        &state,
        &request,
        V4Permission::ProductWrite,
        &public_account,
    ) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if !valid_namespace(&namespace) || !valid_instance(&instance) {
        return error_response(V4Error::NotFound, context.request_id());
    }
    if let Err(error) = require_no_query(&request) {
        return error_response(error, context.request_id());
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Upsert {
        key: String,
        next_action: String,
        #[serde(default, rename = "wait_for_completion")]
        _wait_for_completion: bool,
    }
    let body = match json::<Upsert>(request, context.request_id()).await {
        Ok(value) if valid_filename(&value.key) && value.next_action == "INDEX" => value,
        Ok(_) => return error_response(V4Error::InvalidRequest, context.request_id()),
        Err(response) => return response,
    };
    let listed = match call(
        &api,
        account,
        &namespace,
        context.request_id(),
        "items.list",
        Some(&instance),
        json!({"page": 1, "per_page": 1, "key": body.key, "source": "builtin"}),
    )
    .await
    {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let item_id = listed
        .get("result")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|item| item.get("id"))
        .and_then(Value::as_str);
    let Some(item_id) = item_id else {
        return error_response(V4Error::NotFound, context.request_id());
    };
    let result = call(
        &api,
        account,
        &namespace,
        context.request_id(),
        "item.sync",
        Some(&instance),
        json!({"itemId": item_id}),
    )
    .await;
    match result {
        Ok(mut value) => {
            item_namespace(&mut value, &namespace);
            respond(context, value)
        }
        Err(error) => error_response(error, context.request_id()),
    }
}

pub(super) async fn download(
    State(state): State<HttpState>,
    Path((public_account, namespace, instance, item_id)): Path<(String, String, String, String)>,
    request: Request,
) -> Response {
    let (context, account, api) =
        match authenticated(&state, &request, V4Permission::Read, &public_account) {
            Ok(value) => value,
            Err(response) => return response,
        };
    if !valid_namespace(&namespace) || !valid_instance(&instance) || !valid_object_id(&item_id) {
        return error_response(V4Error::NotFound, context.request_id());
    }
    if let Err(error) = require_no_query(&request) {
        return error_response(error, context.request_id());
    }
    let service = match api.ai_search() {
        Some(value) => value,
        None => return error_response(V4Error::Unavailable, context.request_id()),
    };
    let mut response = match service
        .official_download(
            account,
            &namespace,
            &instance,
            &item_id,
            context.request_id(),
        )
        .await
    {
        Ok(value) => value,
        Err(error) => return error_response(V4Error::from(&error), context.request_id()),
    };
    let request_id = match HeaderValue::from_str(&context.request_id().to_string()) {
        Ok(value) => value,
        Err(_) => return error_response(V4Error::Internal, context.request_id()),
    };
    response.headers_mut().insert(REQUEST_ID_HEADER, request_id);
    response
}

struct Upload {
    filename: String,
    content_type: String,
    metadata: Map<String, Value>,
    bytes: Bytes,
    wait_for_completion: bool,
}

async fn read_upload(request: Request) -> Result<Upload, V4Error> {
    let mut multipart = Multipart::from_request(request, &())
        .await
        .map_err(|_| V4Error::InvalidRequest)?;
    let mut file = None;
    let mut metadata = None;
    let mut wait = None;
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|_| V4Error::InvalidRequest)?
    {
        match field.name() {
            Some("file") if file.is_none() => {
                let filename = field
                    .file_name()
                    .filter(|value| valid_filename(value))
                    .ok_or(V4Error::InvalidRequest)?
                    .to_owned();
                let content_type = field
                    .content_type()
                    .unwrap_or("application/octet-stream")
                    .to_owned();
                if content_type.len() > 128 || content_type.chars().any(char::is_control) {
                    return Err(V4Error::InvalidRequest);
                }
                let bytes = read_field(&mut field, MAX_FILE_BYTES).await?;
                if bytes.is_empty() {
                    return Err(V4Error::InvalidRequest);
                }
                file = Some((filename, content_type, bytes.freeze()));
            }
            Some("metadata") if metadata.is_none() => {
                if !valid_metadata_content_type(field.content_type()) {
                    return Err(V4Error::InvalidRequest);
                }
                let bytes = read_field(&mut field, MAX_METADATA_BYTES).await?;
                let value: Map<String, Value> =
                    serde_json::from_slice(&bytes).map_err(|_| V4Error::InvalidRequest)?;
                metadata = Some(value);
            }
            Some("wait_for_completion") if wait.is_none() => {
                if !valid_part_content_type(field.content_type(), "text/plain") {
                    return Err(V4Error::InvalidRequest);
                }
                let bytes = read_field(&mut field, 5).await?;
                wait = Some(match bytes.as_ref() {
                    b"true" => true,
                    b"false" => false,
                    _ => return Err(V4Error::InvalidRequest),
                });
            }
            _ => return Err(V4Error::InvalidRequest),
        }
    }
    let (filename, content_type, bytes) = file.ok_or(V4Error::InvalidRequest)?;
    Ok(Upload {
        filename,
        content_type,
        metadata: metadata.unwrap_or_default(),
        bytes,
        wait_for_completion: wait.unwrap_or(false),
    })
}

async fn read_field(
    field: &mut axum::extract::multipart::Field<'_>,
    limit: usize,
) -> Result<BytesMut, V4Error> {
    let mut output = BytesMut::new();
    while let Some(chunk) = field.chunk().await.map_err(|_| V4Error::InvalidRequest)? {
        let length = output
            .len()
            .checked_add(chunk.len())
            .ok_or(V4Error::InvalidRequest)?;
        if length > limit {
            return Err(V4Error::Official(
                crate::cloudflare_v4::wire::V4OfficialError::RequestTooLarge,
            ));
        }
        output.extend_from_slice(&chunk);
    }
    Ok(output)
}

fn valid_filename(value: &str) -> bool {
    (1..=128).contains(&value.chars().count()) && !value.chars().any(char::is_control)
}

fn valid_multipart_content_type(headers: &HeaderMap) -> bool {
    let mut values = headers.get_all(header::CONTENT_TYPE).iter();
    let Some(value) = values.next().and_then(|value| value.to_str().ok()) else {
        return false;
    };
    if values.next().is_some() {
        return false;
    }
    let mut parts = value.split(';').map(str::trim);
    parts.next() == Some("multipart/form-data")
        && matches!((parts.next(), parts.next()), (Some(boundary), None)
            if boundary.strip_prefix("boundary=").is_some_and(valid_boundary))
}

fn valid_part_content_type(content_type: Option<&str>, expected: &str) -> bool {
    let Some(value) = content_type else {
        return true;
    };
    let mut parts = value.split(';').map(str::trim);
    if parts.next() != Some(expected) {
        return false;
    }
    match (parts.next(), parts.next()) {
        (None, None) => true,
        (Some(parameter), None) => parameter.eq_ignore_ascii_case("charset=utf-8"),
        _ => false,
    }
}

fn valid_metadata_content_type(content_type: Option<&str>) -> bool {
    content_type.is_none_or(|value| {
        valid_part_content_type(Some(value), "text/plain")
            || valid_part_content_type(Some(value), "application/json")
    })
}

fn valid_boundary(value: &str) -> bool {
    let value = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(value);
    (1..=70).contains(&value.len())
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'\''
                        | b'('
                        | b')'
                        | b'+'
                        | b'_'
                        | b','
                        | b'-'
                        | b'.'
                        | b'/'
                        | b':'
                        | b'='
                        | b'?'
                )
        })
}
