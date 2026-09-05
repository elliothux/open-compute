//! Exact multipart normalization for the pinned `cloudflare@7.1.0` SDK.

use super::model::WorkerUploadBinding;
use super::multipart::{
    MAX_BOUNDARY_BYTES, MAX_METADATA_BINDINGS, MAX_METADATA_BYTES, MAX_SDK_FIELD_NAME_BYTES,
    RawPart, invalid, too_large, validate_part_name,
};
use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderValue, header};
use bytes::Bytes;
use futures::{StreamExt as _, stream};
use open_compute_core::PlatformError;
use serde_json::{Map, Number, Value};

const MAX_METADATA_DEPTH: usize = 32;

/// Recover the boundary omitted by the pinned SDK's typed Worker upload.
pub(super) async fn normalize_request(request: Request) -> Result<Request, PlatformError> {
    let mut content_types = request.headers().get_all(header::CONTENT_TYPE).iter();
    let content_type = content_types
        .next()
        .and_then(|value| value.to_str().ok())
        .ok_or_else(invalid)?;
    if content_types.next().is_some() {
        return Err(invalid());
    }
    let is_sdk_upload = content_type.eq_ignore_ascii_case("application/javascript");
    if !is_sdk_upload {
        let boundary = multer::parse_boundary(content_type).map_err(|_| invalid())?;
        if boundary.is_empty()
            || boundary.len() > MAX_BOUNDARY_BYTES
            || !boundary.bytes().all(valid_boundary_byte)
        {
            return Err(invalid());
        }
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
    if name.len() > MAX_SDK_FIELD_NAME_BYTES
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
        if part.content_type.is_some() || part.file_name.is_some() {
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
    binding_fields: Vec<BindingField>,
}

#[derive(Clone)]
struct BindingField {
    path: Vec<String>,
    raw: String,
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
                self.binding_fields.push(BindingField {
                    path: vec![field.clone()],
                    raw,
                });
                Ok(())
            }
            [bindings, item, field, tail @ ..]
                if bindings == "bindings" && item.is_empty() && !tail.is_empty() =>
            {
                let mut path = Vec::with_capacity(tail.len() + 1);
                path.push(field.clone());
                path.extend_from_slice(tail);
                self.binding_fields.push(BindingField { path, raw });
                Ok(())
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

    fn finish(mut self) -> Result<Option<Map<String, Value>>, PlatformError> {
        let bindings = partition_bindings(&self.binding_fields)?;
        if !bindings.is_empty() {
            insert_unique(
                &mut self.root,
                "bindings",
                Value::Array(bindings.into_iter().map(Value::Object).collect()),
            )?;
        }
        if self.root.is_empty() {
            Ok(None)
        } else {
            Ok(Some(self.root))
        }
    }
}

fn partition_bindings(fields: &[BindingField]) -> Result<Vec<Map<String, Value>>, PlatformError> {
    if fields.is_empty() {
        return Ok(Vec::new());
    }
    let names = fields
        .iter()
        .filter(|field| binding_key(field, "name"))
        .count();
    let types = fields
        .iter()
        .filter(|field| binding_key(field, "type"))
        .count();
    if names == 0 || names != types || names > MAX_METADATA_BINDINGS {
        return Err(invalid());
    }
    let mut memo = vec![None; fields.len() + 1];
    let solutions = binding_partitions_from(fields, 0, &mut memo)?;
    if solutions.len() != 1 {
        return Err(invalid());
    }
    solutions
        .into_iter()
        .next()
        .ok_or_else(invalid)?
        .into_iter()
        .map(|(start, end)| build_binding(&fields[start..end]))
        .collect()
}

type BindingRanges = Vec<(usize, usize)>;

fn binding_partitions_from(
    fields: &[BindingField],
    start: usize,
    memo: &mut [Option<Vec<BindingRanges>>],
) -> Result<Vec<BindingRanges>, PlatformError> {
    if start == fields.len() {
        return Ok(vec![Vec::new()]);
    }
    if let Some(cached) = &memo[start] {
        return Ok(cached.clone());
    }
    let mut remaining_names = fields[start..]
        .iter()
        .filter(|field| binding_key(field, "name"))
        .count();
    let mut remaining_types = fields[start..]
        .iter()
        .filter(|field| binding_key(field, "type"))
        .count();
    if remaining_names == 0 || remaining_names != remaining_types {
        memo[start] = Some(Vec::new());
        return Ok(Vec::new());
    }
    let mut object = Map::new();
    let mut solutions = Vec::new();
    for end in start..fields.len() {
        if insert_binding_field(&mut object, &fields[end]).is_err() {
            break;
        }
        if binding_key(&fields[end], "name") {
            remaining_names -= 1;
        }
        if binding_key(&fields[end], "type") {
            remaining_types -= 1;
        }
        if remaining_names != remaining_types || (remaining_names == 0 && end + 1 != fields.len()) {
            continue;
        }
        if normalized_binding(object.clone()).is_err() {
            continue;
        }
        for suffix in binding_partitions_from(fields, end + 1, memo)? {
            let mut solution = Vec::with_capacity(suffix.len() + 1);
            solution.push((start, end + 1));
            solution.extend(suffix);
            solutions.push(solution);
            if solutions.len() == 2 {
                memo[start] = Some(solutions.clone());
                return Ok(solutions);
            }
        }
    }
    memo[start] = Some(solutions.clone());
    Ok(solutions)
}

fn binding_key(field: &BindingField, key: &str) -> bool {
    field.path.len() == 1 && field.path[0] == key
}

fn build_binding(fields: &[BindingField]) -> Result<Map<String, Value>, PlatformError> {
    let mut object = Map::new();
    for field in fields {
        insert_binding_field(&mut object, field)?;
    }
    normalized_binding(object)
}

fn normalized_binding(
    mut binding: Map<String, Value>,
) -> Result<Map<String, Value>, PlatformError> {
    if binding.get("type").and_then(Value::as_str) == Some("d1") {
        if let Some(database_id) = binding.remove("database_id")
            && binding.insert("id".to_owned(), database_id).is_some()
        {
            return Err(invalid());
        }
    } else if binding.contains_key("database_id") {
        return Err(invalid());
    }
    serde_json::from_value::<WorkerUploadBinding>(Value::Object(binding.clone()))
        .map_err(|_| invalid())?;
    Ok(binding)
}

fn insert_binding_field(
    binding: &mut Map<String, Value>,
    field: &BindingField,
) -> Result<(), PlatformError> {
    let [name, tail @ ..] = field.path.as_slice() else {
        return Err(invalid());
    };
    if tail.is_empty()
        && matches!(
            name.as_str(),
            "name"
                | "type"
                | "text"
                | "namespace_id"
                | "bucket_name"
                | "jurisdiction"
                | "id"
                | "database_id"
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
        return insert_unique(binding, name, Value::String(field.raw.clone()));
    }
    if tail.is_empty() && matches!(name.as_str(), "raw" | "staging") {
        return insert_unique(binding, name, Value::Bool(parse_bool(&field.raw)?));
    }
    if tail.is_empty() && name == "delivery_delay" {
        let number = field.raw.parse::<Number>().map_err(|_| invalid())?;
        return insert_unique(binding, name, Value::Number(number));
    }
    if matches!(name.as_str(), "json" | "props") {
        let value = unambiguous_json_string(field.raw.clone())?;
        return insert_json_value(binding, name, tail, value);
    }
    Err(invalid())
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
#[path = "sdk_multipart_tests.rs"]
mod tests;
