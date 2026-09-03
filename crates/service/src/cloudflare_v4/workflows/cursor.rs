//! Query-bound opaque Workflow instance cursors.

use crate::cloudflare_v4::V4Error;
use base64::Engine as _;
use open_compute_core::{AccountId, WorkflowInstanceId};
use open_compute_storage::PlatformStorage;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Payload {
    version: u8,
    account: String,
    workflow: String,
    query: String,
    created_at_ms: i64,
    instance_id: String,
    expires_at_ms: i64,
}

pub(super) struct Position {
    pub(super) created_at_ms: i64,
    pub(super) instance_id: WorkflowInstanceId,
}

pub(super) fn seal(
    storage: &PlatformStorage,
    account: AccountId,
    workflow: &str,
    query: &str,
    position: &Position,
    expires_at_ms: i64,
) -> Result<String, V4Error> {
    let payload = serde_json::to_vec(&Payload {
        version: 1,
        account: account.to_string(),
        workflow: workflow.to_owned(),
        query: query.to_owned(),
        created_at_ms: position.created_at_ms,
        instance_id: position.instance_id.to_string(),
        expires_at_ms,
    })
    .map_err(|_| V4Error::Internal)?;
    let signature = storage.crypto().sign_workflow_cursor(&payload);
    Ok(format!(
        "{}.{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload),
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature)
    ))
}

pub(super) fn open(
    storage: &PlatformStorage,
    token: &str,
    account: AccountId,
    workflow: &str,
    query: &str,
    now_ms: i64,
) -> Result<Position, V4Error> {
    let (payload, signature) = token.split_once('.').ok_or(V4Error::InvalidRequest)?;
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| V4Error::InvalidRequest)?;
    let signature = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| V4Error::InvalidRequest)?;
    if !storage
        .crypto()
        .verify_workflow_cursor(&payload, &signature)
    {
        return Err(V4Error::InvalidRequest);
    }
    let payload: Payload = serde_json::from_slice(&payload).map_err(|_| V4Error::InvalidRequest)?;
    if payload.version != 1
        || payload.account != account.to_string()
        || payload.workflow != workflow
        || payload.query != query
        || payload.expires_at_ms < now_ms
    {
        return Err(V4Error::InvalidRequest);
    }
    Ok(Position {
        created_at_ms: payload.created_at_ms,
        instance_id: payload
            .instance_id
            .parse()
            .map_err(|_| V4Error::InvalidRequest)?,
    })
}
