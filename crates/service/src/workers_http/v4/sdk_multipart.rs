//! Exact multipart normalization for the pinned `cloudflare@7.1.0` SDK.

use super::multipart::{MAX_METADATA_BYTES, RawPart, invalid, too_large, validate_part_name};
use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderValue, header};
use bytes::Bytes;
use futures::{StreamExt as _, stream};
use open_compute_core::PlatformError;
use serde_json::{Map, Number, Value};

const MAX_BOUNDARY_BYTES: usize = 70;
const MAX_FIELD_NAME_BYTES: usize = 4 * 1024;
const MAX_METADATA_DEPTH: usize = 32;

/// Recover the boundary omitted by the pinned SDK's typed Worker upload.
pub(super) async fn normalize_request(request: Request) -> Result<Request, PlatformError> {
    let is_sdk_upload = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("application/javascript"));
    if !is_sdk_upload {
        return Ok(request);
    }

    let (mut parts, body) = request.into_parts();
    let mut body = body.into_data_stream();
    let mut preserved = Vec::<Result<Bytes, axum::Error>>::new();
    let mut opening = Vec::with_capacity(MAX_BOUNDARY_BYTES + 2);
    'opening: loop {
        let chunk = body
            .next()
            .await
            .ok_or_else(invalid)?
            .map_err(|_| invalid())?;
        for byte in chunk.iter().copied() {
            if opening.last() == Some(&b'\r') && byte == b'\n' {
                opening.pop();
                preserved.push(Ok(chunk));
                break 'opening;
            }
            opening.push(byte);
            if opening.len() > MAX_BOUNDARY_BYTES + 2 {
                return Err(invalid());
            }
        }
        preserved.push(Ok(chunk));
    }
    let boundary = opening.strip_prefix(b"--").ok_or_else(invalid)?;
    if boundary.is_empty()
        || boundary.len() > MAX_BOUNDARY_BYTES
        || !boundary.iter().copied().all(valid_boundary_byte)
    {
        return Err(invalid());
    }
    let boundary = std::str::from_utf8(boundary).map_err(|_| invalid())?;
    parts.headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&format!("multipart/form-data; boundary={boundary}"))
            .map_err(|_| invalid())?,
    );
    Ok(Request::from_parts(
        parts,
        Body::from_stream(stream::iter(preserved).chain(body)),
    ))
}

fn valid_boundary_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'\'' | b'(' | b')' | b'+' | b'_' | b',' | b'-' | b'.' | b'/' | b':' | b'=' | b'?'
        )
}

pub(super) fn validate_metadata_field_name(name: &str) -> Result<(), PlatformError> {
    if name == "metadata" {
        return Ok(());
    }
    if name.len() > MAX_FIELD_NAME_BYTES
        || name.chars().any(char::is_control)
        || !name.starts_with("metadata[")
    {
        return Err(invalid());
    }
    let _ = metadata_path(name)?;
    Ok(())
}

pub(super) fn normalize_parts(parts: &mut Vec<RawPart>) -> Result<(), PlatformError> {
    for part in parts.iter_mut().filter(|part| part.name == "files[]") {
        let file_name = part.file_name.take().ok_or_else(invalid)?;
        validate_part_name(&file_name)?;
        part.name = file_name;
    }
    if parts.iter().any(|part| part.name == "metadata") {
        if parts.iter().any(|part| part.name.starts_with("metadata[")) {
            return Err(invalid());
        }
        return Ok(());
    }

    let mut builder = MetadataBuilder::default();
    let mut retained = Vec::with_capacity(parts.len());
    for part in parts.drain(..) {
        if !part.name.starts_with("metadata[") {
            retained.push(part);
            continue;
        }
        if part.content_type.is_some() || part.file_name.is_some() || part.bytes.is_empty() {
            return Err(invalid());
        }
        let path = metadata_path(&part.name)?;
        let value = String::from_utf8(part.bytes).map_err(|_| invalid())?;
        builder.insert(&path, value)?;
    }
    let Some(metadata) = builder.finish()? else {
        *parts = retained;
        return Ok(());
    };
    let bytes = serde_json::to_vec(&metadata).map_err(|_| invalid())?;
    if bytes.len() > MAX_METADATA_BYTES {
        return Err(too_large());
    }
    retained.push(RawPart {
        name: "metadata".to_owned(),
        file_name: None,
        content_type: Some("application/json".to_owned()),
        bytes,
    });
    *parts = retained;
    Ok(())
}

fn metadata_path(name: &str) -> Result<Vec<String>, PlatformError> {
    let mut rest = name.strip_prefix("metadata").ok_or_else(invalid)?;
    let mut path = Vec::new();
    while !rest.is_empty() {
        let inner = rest.strip_prefix('[').ok_or_else(invalid)?;
        let close = inner.find(']').ok_or_else(invalid)?;
        let segment = &inner[..close];
        if segment.contains('[')
            || segment.contains(']')
            || segment.len() > 1_024
            || segment.chars().any(char::is_control)
        {
            return Err(invalid());
        }
        path.push(segment.to_owned());
        if path.len() > MAX_METADATA_DEPTH {
            return Err(invalid());
        }
        rest = &inner[close + 1..];
    }
    if path.is_empty() || path[0].is_empty() {
        return Err(invalid());
    }
    Ok(path)
}

#[derive(Default)]
struct MetadataBuilder {
    root: Map<String, Value>,
    bindings: Vec<Map<String, Value>>,
    binding: Option<Map<String, Value>>,
}

impl MetadataBuilder {
    fn insert(&mut self, path: &[String], raw: String) -> Result<(), PlatformError> {
        match path {
            [field]
                if matches!(
                    field.as_str(),
                    "main_module" | "body_part" | "compatibility_date"
                ) =>
            {
                insert_unique(&mut self.root, field, Value::String(raw))
            }
            [field, item]
                if item.is_empty()
                    && matches!(field.as_str(), "compatibility_flags" | "keep_bindings") =>
            {
                push_string(&mut self.root, field, raw)
            }
            [annotations, key]
                if annotations == "annotations"
                    && matches!(key.as_str(), "workers/tag" | "workers/message") =>
            {
                insert_object_value(&mut self.root, annotations, key, Value::String(raw))
            }
            [bindings, item, field] if bindings == "bindings" && item.is_empty() => {
                self.insert_binding(field, &[], raw)
            }
            [bindings, item, field, tail @ ..]
                if bindings == "bindings" && item.is_empty() && !tail.is_empty() =>
            {
                self.insert_binding(field, tail, raw)
            }
            [assets, jwt] if assets == "assets" && jwt == "jwt" => {
                insert_nested(&mut self.root, assets, &["jwt"], Value::String(raw))
            }
            [assets, config, field]
                if assets == "assets"
                    && config == "config"
                    && matches!(
                        field.as_str(),
                        "html_handling" | "not_found_handling" | "_redirects" | "_headers"
                    ) =>
            {
                insert_nested(
                    &mut self.root,
                    assets,
                    &["config", field],
                    Value::String(raw),
                )
            }
            [assets, config, run]
                if assets == "assets" && config == "config" && run == "run_worker_first" =>
            {
                insert_nested(
                    &mut self.root,
                    assets,
                    &["config", "run_worker_first"],
                    Value::Bool(parse_bool(&raw)?),
                )
            }
            [assets, config, run, item]
                if assets == "assets"
                    && config == "config"
                    && run == "run_worker_first"
                    && item.is_empty() =>
            {
                push_nested_string(&mut self.root, assets, &["config", "run_worker_first"], raw)
            }
            [observability, enabled]
                if observability == "observability" && enabled == "enabled" =>
            {
                insert_nested(
                    &mut self.root,
                    observability,
                    &["enabled"],
                    Value::Bool(parse_bool(&raw)?),
                )
            }
            [cache, field]
                if cache == "cache_options"
                    && matches!(field.as_str(), "enabled" | "cross_version_cache") =>
            {
                insert_nested(
                    &mut self.root,
                    cache,
                    &[field],
                    Value::Bool(parse_bool(&raw)?),
                )
            }
            [exports, name, field]
                if exports == "exports"
                    && matches!(
                        field.as_str(),
                        "type"
                            | "state"
                            | "storage"
                            | "renamed_to"
                            | "container"
                            | "transferred_to"
                            | "transfer_from"
                    ) =>
            {
                insert_dynamic_nested(&mut self.root, exports, name, &[field], Value::String(raw))
            }
            [exports, name, cache, enabled]
                if exports == "exports" && cache == "cache" && enabled == "enabled" =>
            {
                insert_dynamic_nested(
                    &mut self.root,
                    exports,
                    name,
                    &["cache", "enabled"],
                    Value::Bool(parse_bool(&raw)?),
                )
            }
            // The SDK omits array indices for migration steps. Multiple disjoint
            // steps cannot be reconstructed without changing their meaning.
            [migrations, ..] if migrations == "migrations" => Err(invalid()),
            _ => Err(invalid()),
        }
    }

    fn insert_binding(
        &mut self,
        field: &str,
        tail: &[String],
        raw: String,
    ) -> Result<(), PlatformError> {
        if field == "name" {
            if !tail.is_empty() {
                return Err(invalid());
            }
            if let Some(binding) = self.binding.take() {
                self.bindings.push(binding);
            }
            let mut binding = Map::new();
            insert_unique(&mut binding, "name", Value::String(raw))?;
            self.binding = Some(binding);
            return Ok(());
        }
        let binding = self.binding.as_mut().ok_or_else(invalid)?;
        if tail.is_empty()
            && matches!(
                field,
                "type"
                    | "text"
                    | "namespace_id"
                    | "bucket_name"
                    | "jurisdiction"
                    | "id"
                    | "internalEnv"
                    | "index_name"
                    | "namespace"
                    | "instance_name"
                    | "class_name"
                    | "script_name"
                    | "environment"
                    | "queue_name"
                    | "workflow_name"
                    | "service"
                    | "entrypoint"
                    | "cross_account_grant"
                    | "part"
            )
        {
            return insert_unique(binding, field, Value::String(raw));
        }
        if tail.is_empty() && matches!(field, "raw" | "staging") {
            return insert_unique(binding, field, Value::Bool(parse_bool(&raw)?));
        }
        if tail.is_empty() && field == "delivery_delay" {
            let number = raw.parse::<Number>().map_err(|_| invalid())?;
            return insert_unique(binding, field, Value::Number(number));
        }
        if matches!(field, "json" | "props") {
            let value = unambiguous_json_string(raw)?;
            return insert_json_value(binding, field, tail, value);
        }
        Err(invalid())
    }

    fn finish(mut self) -> Result<Option<Map<String, Value>>, PlatformError> {
        if let Some(binding) = self.binding.take() {
            self.bindings.push(binding);
        }
        if !self.bindings.is_empty() {
            insert_unique(
                &mut self.root,
                "bindings",
                Value::Array(self.bindings.into_iter().map(Value::Object).collect()),
            )?;
        }
        if self.root.is_empty() {
            Ok(None)
        } else {
            Ok(Some(self.root))
        }
    }
}

fn parse_bool(raw: &str) -> Result<bool, PlatformError> {
    match raw {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(invalid()),
    }
}

fn unambiguous_json_string(raw: String) -> Result<Value, PlatformError> {
    if raw == "null" || raw == "true" || raw == "false" || raw.parse::<Number>().is_ok() {
        return Err(invalid());
    }
    Ok(Value::String(raw))
}

fn insert_unique(
    object: &mut Map<String, Value>,
    key: impl Into<String>,
    value: Value,
) -> Result<(), PlatformError> {
    if object.insert(key.into(), value).is_some() {
        Err(invalid())
    } else {
        Ok(())
    }
}

fn push_string(
    object: &mut Map<String, Value>,
    key: &str,
    value: String,
) -> Result<(), PlatformError> {
    match object
        .entry(key.to_owned())
        .or_insert_with(|| Value::Array(Vec::new()))
    {
        Value::Array(values) if !values.iter().any(|item| item.as_str() == Some(&value)) => {
            values.push(Value::String(value));
            Ok(())
        }
        _ => Err(invalid()),
    }
}

fn insert_object_value(
    root: &mut Map<String, Value>,
    object: &str,
    key: &str,
    value: Value,
) -> Result<(), PlatformError> {
    let nested = object_at(root, object)?;
    insert_unique(nested, key, value)
}

fn insert_nested(
    root: &mut Map<String, Value>,
    object: &str,
    path: &[&str],
    value: Value,
) -> Result<(), PlatformError> {
    insert_json_value(
        object_at(root, object)?,
        path[0],
        &path[1..]
            .iter()
            .map(|s| (*s).to_owned())
            .collect::<Vec<_>>(),
        value,
    )
}

fn push_nested_string(
    root: &mut Map<String, Value>,
    object: &str,
    path: &[&str],
    value: String,
) -> Result<(), PlatformError> {
    let mut current = object_at(root, object)?;
    for segment in &path[..path.len() - 1] {
        current = object_at(current, segment)?;
    }
    push_string(current, path[path.len() - 1], value)
}

fn insert_dynamic_nested(
    root: &mut Map<String, Value>,
    object: &str,
    dynamic: &str,
    path: &[&str],
    value: Value,
) -> Result<(), PlatformError> {
    if dynamic.is_empty() || dynamic.len() > 255 {
        return Err(invalid());
    }
    let dynamic_object = object_at(object_at(root, object)?, dynamic)?;
    insert_json_value(
        dynamic_object,
        path[0],
        &path[1..]
            .iter()
            .map(|s| (*s).to_owned())
            .collect::<Vec<_>>(),
        value,
    )
}

fn object_at<'a>(
    object: &'a mut Map<String, Value>,
    key: &str,
) -> Result<&'a mut Map<String, Value>, PlatformError> {
    let value = object
        .entry(key.to_owned())
        .or_insert_with(|| Value::Object(Map::new()));
    value.as_object_mut().ok_or_else(invalid)
}

fn insert_json_value(
    object: &mut Map<String, Value>,
    field: &str,
    tail: &[String],
    value: Value,
) -> Result<(), PlatformError> {
    if tail.is_empty() {
        return insert_unique(object, field, value);
    }
    if tail.last().is_some_and(String::is_empty) {
        if tail.len() == 1 {
            return push_value_at(object, field, value);
        }
        let mut current = object_at(object, field)?;
        for segment in &tail[..tail.len() - 2] {
            if segment.is_empty() {
                return Err(invalid());
            }
            current = object_at(current, segment)?;
        }
        return push_value_at(current, &tail[tail.len() - 2], value);
    }
    let mut current = object_at(object, field)?;
    for segment in &tail[..tail.len() - 1] {
        if segment.is_empty() {
            return Err(invalid());
        }
        current = object_at(current, segment)?;
    }
    let leaf = tail.last().ok_or_else(invalid)?;
    insert_unique(current, leaf, value)
}

fn push_value_at(
    object: &mut Map<String, Value>,
    field: &str,
    value: Value,
) -> Result<(), PlatformError> {
    if field.is_empty() {
        return Err(invalid());
    }
    match object
        .entry(field.to_owned())
        .or_insert_with(|| Value::Array(Vec::new()))
    {
        Value::Array(values) => {
            values.push(value);
            Ok(())
        }
        _ => Err(invalid()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workers_http::v4::multipart::{MAX_BODY_BYTES, parse_worker_upload};
    use axum::Router;
    use axum::extract::{DefaultBodyLimit, FromRequest as _, Multipart};
    use axum::http::StatusCode;
    use axum::routing::post;
    use open_compute_workers::BundleLimits;
    use tower::ServiceExt as _;

    fn string_part(name: &str, bytes: &[u8]) -> RawPart {
        RawPart {
            name: name.to_owned(),
            file_name: None,
            content_type: None,
            bytes: bytes.to_vec(),
        }
    }

    #[test]
    fn rebuilds_pinned_sdk_flags_annotations_and_binding() {
        let mut parts = vec![
            string_part("metadata[main_module]", b"index.js"),
            string_part("metadata[compatibility_date]", b"2026-08-30"),
            string_part("metadata[compatibility_flags][]", b"nodejs_compat"),
            string_part("metadata[annotations][workers/tag]", b"sdk-typed"),
            string_part("metadata[bindings][][name]", b"MODE"),
            string_part("metadata[bindings][][type]", b"plain_text"),
            string_part("metadata[bindings][][text]", b"sdk"),
            RawPart {
                name: "files[]".to_owned(),
                file_name: Some("index.js".to_owned()),
                content_type: Some("application/javascript+module".to_owned()),
                bytes: b"export default {}".to_vec(),
            },
        ];
        normalize_parts(&mut parts).unwrap();
        let metadata = parts.iter().find(|part| part.name == "metadata").unwrap();
        let value: Value = serde_json::from_slice(&metadata.bytes).unwrap();
        assert_eq!(value["compatibility_flags"][0], "nodejs_compat");
        assert_eq!(value["annotations"]["workers/tag"], "sdk-typed");
        assert_eq!(value["bindings"][0]["name"], "MODE");
        assert_eq!(
            parts.iter().filter(|part| part.name == "index.js").count(),
            1
        );
    }

    #[test]
    fn rejects_unknown_duplicate_and_ambiguous_sdk_fields() {
        for fields in [
            vec![("metadata[unknown]", "x")],
            vec![
                ("metadata[main_module]", "a"),
                ("metadata[main_module]", "b"),
            ],
            vec![("metadata[bindings][][type]", "plain_text")],
            vec![
                ("metadata[bindings][][name]", "X"),
                ("metadata[bindings][][json]", "true"),
            ],
            vec![("metadata[migrations][new_tag]", "v1")],
        ] {
            let mut parts = fields
                .into_iter()
                .map(|(name, value)| string_part(name, value.as_bytes()))
                .collect();
            assert!(normalize_parts(&mut parts).is_err());
        }
    }

    #[tokio::test]
    async fn recovers_boundary_split_across_chunks_without_losing_bytes() {
        let boundary = "----open-compute-sdk-boundary";
        let chunks = [
            format!("--{}", &boundary[..8]).into_bytes(),
            format!("{}\r", &boundary[8..]).into_bytes(),
            b"\nContent-Disposition: form-data; name=\"metadata[main_module]\"\r\n\r\nindex.js\r\n"
                .to_vec(),
            format!("--{boundary}--\r\n").into_bytes(),
        ];
        let body = Body::from_stream(stream::iter(
            chunks.into_iter().map(Ok::<_, std::io::Error>),
        ));
        let request = Request::builder()
            .header(header::CONTENT_TYPE, "application/javascript")
            .body(body)
            .unwrap();
        let request = normalize_request(request).await.unwrap();
        assert_eq!(
            request.headers()[header::CONTENT_TYPE],
            format!("multipart/form-data; boundary={boundary}")
        );
        let mut multipart = Multipart::from_request(request, &()).await.unwrap();
        let field = multipart.next_field().await.unwrap().unwrap();
        assert_eq!(field.name(), Some("metadata[main_module]"));
        assert_eq!(field.text().await.unwrap(), "index.js");
    }

    async fn bounded_upload(request: Request) -> StatusCode {
        let request = match normalize_request(request).await {
            Ok(request) => request,
            Err(_) => return StatusCode::BAD_REQUEST,
        };
        let multipart = match Multipart::from_request(request, &()).await {
            Ok(multipart) => multipart,
            Err(_) => return StatusCode::BAD_REQUEST,
        };
        match parse_worker_upload(multipart, BundleLimits::default()).await {
            Ok(_) => StatusCode::NO_CONTENT,
            Err(_) => StatusCode::BAD_REQUEST,
        }
    }

    fn multipart_body(boundary: &str, module: &[u8]) -> Vec<u8> {
        let mut body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"metadata\"\r\nContent-Type: application/json\r\n\r\n{{\"main_module\":\"index.js\",\"compatibility_date\":\"2026-08-30\"}}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"index.js\"; filename=\"index.js\"\r\nContent-Type: application/javascript+module\r\n\r\n"
        )
        .into_bytes();
        body.extend_from_slice(module);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        body
    }

    #[tokio::test]
    async fn explicit_worker_limit_accepts_more_than_axum_default() {
        let boundary = "open-compute-large-worker";
        let mut module = vec![b' '; 2 * 1024 * 1024 + 1];
        module.extend_from_slice(b"\nexport default {};");
        let app = Router::new()
            .route("/", post(bounded_upload))
            .layer(DefaultBodyLimit::max(MAX_BODY_BYTES));
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/")
                    .header(
                        header::CONTENT_TYPE,
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .body(Body::from(multipart_body(boundary, &module)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn explicit_worker_limit_rejects_chunked_wire_overhead() {
        let boundary = "open-compute-over-limit";
        let chunks = [
            format!("--{boundary}\r\n").into_bytes(),
            vec![b'x'; MAX_BODY_BYTES],
            format!("\r\n--{boundary}--\r\n").into_bytes(),
        ];
        let body = Body::from_stream(stream::iter(
            chunks.into_iter().map(Ok::<_, std::io::Error>),
        ));
        let app = Router::new()
            .route("/", post(bounded_upload))
            .layer(DefaultBodyLimit::max(MAX_BODY_BYTES));
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/")
                    .header(
                        header::CONTENT_TYPE,
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .body(body)
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
