//! Official KV bulk read, write, and delete operations.

use super::values::execute;
use super::{MAX_BULK_BODY, MAX_BULK_KEYS, MAX_VALUE_BODY, namespace};
use crate::cloudflare_v4::storage::{json, json_with_limit, now_ms, require_no_query};
use crate::cloudflare_v4::{V4Error, error_response, success_response};
use crate::http::HttpState;
use crate::kv_backend::{KvCommand, KvCommandResult};
use axum::extract::{Path, Request, State};
use axum::response::Response;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BulkPut {
    key: String,
    value: String,
    #[serde(default)]
    base64: bool,
    expiration: Option<u64>,
    expiration_ttl: Option<u64>,
    #[serde(default)]
    metadata: Option<Value>,
}

pub(super) async fn update(
    State(state): State<HttpState>,
    Path((account_id, namespace_id)): Path<(String, String)>,
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
    let values =
        match json_with_limit::<Vec<BulkPut>>(request, context.request_id(), MAX_BULK_BODY).await {
            Ok(value) if value.len() <= MAX_BULK_KEYS => value,
            Ok(_) => return error_response(V4Error::InvalidRequest, context.request_id()),
            Err(response) => return response.into_response(),
        };
    let now_seconds = match now_ms()
        .and_then(|value| u64::try_from(value / 1000).map_err(|_| V4Error::Internal))
    {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let Some(minimum_expiration) = now_seconds.checked_add(60) else {
        return error_response(V4Error::Internal, context.request_id());
    };
    let mut commands = Vec::with_capacity(values.len());
    for value in values {
        if open_compute_storage::validate_key(&value.key).is_err()
            || value.expiration_ttl.is_some_and(|ttl| ttl < 60)
            || value.expiration_ttl.is_none()
                && value
                    .expiration
                    .is_some_and(|expiration| expiration < minimum_expiration)
        {
            return error_response(V4Error::InvalidRequest, context.request_id());
        }
        let bytes = if value.base64 {
            match base64::engine::general_purpose::STANDARD.decode(value.value) {
                Ok(value) => value,
                Err(_) => {
                    return error_response(V4Error::InvalidRequest, context.request_id());
                }
            }
        } else {
            value.value.into_bytes()
        };
        if bytes.len() > MAX_VALUE_BODY
            || value
                .metadata
                .as_ref()
                .is_some_and(|metadata| open_compute_storage::canonical_metadata(metadata).is_err())
        {
            return error_response(V4Error::InvalidRequest, context.request_id());
        }
        commands.push(KvCommand::Put {
            key: value.key,
            value: bytes,
            expiration: value.expiration,
            expiration_ttl: value.expiration_ttl,
            metadata: value.metadata.clone(),
            metadata_present: value.metadata.is_some(),
        });
    }
    let mut successful_key_count = 0_usize;
    let mut unsuccessful_keys = Vec::new();
    for command in commands {
        let key = match &command {
            KvCommand::Put { key, .. } => key.clone(),
            _ => return error_response(V4Error::Internal, context.request_id()),
        };
        match execute(&state, account_id, record.resource.id, command) {
            Ok(KvCommandResult::Mutation) => successful_key_count += 1,
            Ok(_) => return error_response(V4Error::Internal, context.request_id()),
            Err(error @ (V4Error::Internal | V4Error::Unavailable)) => {
                return error_response(error, context.request_id());
            }
            Err(_) => unsuccessful_keys.push(key),
        }
    }
    success_response(
        context,
        BulkMutationResult {
            successful_key_count,
            unsuccessful_keys,
        },
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BulkGet {
    keys: Vec<String>,
    #[serde(default)]
    r#type: BulkType,
    #[serde(default, rename = "withMetadata")]
    with_metadata: bool,
}

#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
enum BulkType {
    #[default]
    Text,
    Json,
}

pub(super) async fn get(
    State(state): State<HttpState>,
    Path((account_id, namespace_id)): Path<(String, String)>,
    request: Request,
) -> Response {
    let (context, account_id, record) =
        match namespace(&state, &request, &account_id, &namespace_id, false) {
            Ok(value) => value,
            Err(response) => return response.into_response(),
        };
    if let Err(error) = require_no_query(&request) {
        return error_response(error, context.request_id());
    }
    let body = match json::<BulkGet>(request, context.request_id()).await {
        Ok(value) if value.keys.len() <= 100 => value,
        Ok(_) => return error_response(V4Error::InvalidRequest, context.request_id()),
        Err(response) => return response.into_response(),
    };
    if body
        .keys
        .iter()
        .any(|key| open_compute_storage::validate_key(key).is_err())
    {
        return error_response(V4Error::InvalidRequest, context.request_id());
    }
    let result = execute(
        &state,
        account_id,
        record.resource.id,
        KvCommand::Get {
            keys: body.keys.clone(),
            cache_ttl: None,
        },
    );
    let entries = match result {
        Ok(KvCommandResult::Entries(value)) => value,
        Ok(_) => return error_response(V4Error::Internal, context.request_id()),
        Err(error) => return error_response(error, context.request_id()),
    };
    if entries.len() != body.keys.len() {
        return error_response(V4Error::Internal, context.request_id());
    }
    let mut values = BTreeMap::new();
    for (key, entry) in body.keys.into_iter().zip(entries) {
        let value = match entry {
            None => Value::Null,
            Some(entry) => {
                let parsed = match body.r#type {
                    BulkType::Text => String::from_utf8(entry.value).map(Value::String).ok(),
                    BulkType::Json => serde_json::from_slice(&entry.value).ok(),
                };
                let Some(parsed) = parsed else {
                    return error_response(V4Error::InvalidRequest, context.request_id());
                };
                if body.with_metadata {
                    serde_json::json!({
                        "value": parsed,
                        "metadata": match entry.metadata_json.as_deref() {
                            Some(value) => match serde_json::from_slice::<Value>(value) {
                                Ok(value) => Some(value),
                                Err(_) => return error_response(V4Error::Internal, context.request_id()),
                            },
                            None => None,
                        },
                        "expiration": match entry.expires_at_ms {
                            Some(value) => match u64::try_from(value / 1000) {
                                Ok(value) => Some(value),
                                Err(_) => return error_response(V4Error::Internal, context.request_id()),
                            },
                            None => None,
                        },
                    })
                } else {
                    parsed
                }
            }
        };
        values.insert(key, value);
    }
    success_response(context, serde_json::json!({ "values": values }))
}

pub(super) async fn delete(
    State(state): State<HttpState>,
    Path((account_id, namespace_id)): Path<(String, String)>,
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
    let keys = match json::<Vec<String>>(request, context.request_id()).await {
        Ok(value) if value.len() <= MAX_BULK_KEYS => value,
        Ok(_) => return error_response(V4Error::InvalidRequest, context.request_id()),
        Err(response) => return response.into_response(),
    };
    if keys
        .iter()
        .any(|key| open_compute_storage::validate_key(key).is_err())
    {
        return error_response(V4Error::InvalidRequest, context.request_id());
    }
    let mut successful_key_count = 0_usize;
    let mut unsuccessful_keys = Vec::new();
    for key in keys {
        match execute(
            &state,
            account_id,
            record.resource.id,
            KvCommand::Delete { key: key.clone() },
        ) {
            Ok(KvCommandResult::Mutation) => successful_key_count += 1,
            Ok(_) => return error_response(V4Error::Internal, context.request_id()),
            Err(error @ (V4Error::Internal | V4Error::Unavailable)) => {
                return error_response(error, context.request_id());
            }
            Err(_) => unsuccessful_keys.push(key),
        }
    }
    success_response(
        context,
        BulkMutationResult {
            successful_key_count,
            unsuccessful_keys,
        },
    )
}

#[derive(Serialize)]
struct BulkMutationResult {
    successful_key_count: usize,
    unsuccessful_keys: Vec<String>,
}

#[cfg(test)]
#[path = "bulk_tests.rs"]
mod tests;
