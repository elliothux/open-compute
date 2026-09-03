//! Account- and query-bound opaque AI Search log cursors.

use crate::cloudflare_v4::V4Error;
use base64::Engine as _;
use open_compute_core::AccountId;
use open_compute_storage::PlatformStorage;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Payload {
    version: u8,
    account: String,
    namespace: String,
    instance: String,
    item: String,
    limit: u32,
    after: String,
    expires_at_ms: i64,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn seal(
    storage: &PlatformStorage,
    account: AccountId,
    namespace: &str,
    instance: &str,
    item: &str,
    limit: u32,
    after: &str,
    expires_at_ms: i64,
) -> Result<String, V4Error> {
    let payload = serde_json::to_vec(&Payload {
        version: 1,
        account: account.to_string(),
        namespace: namespace.to_owned(),
        instance: instance.to_owned(),
        item: item.to_owned(),
        limit,
        after: after.to_owned(),
        expires_at_ms,
    })
    .map_err(|_| V4Error::Internal)?;
    let signature = storage.crypto().sign_ai_search_cursor(&payload);
    Ok(format!(
        "{}.{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload),
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature)
    ))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn open(
    storage: &PlatformStorage,
    token: &str,
    account: AccountId,
    namespace: &str,
    instance: &str,
    item: &str,
    limit: u32,
    now_ms: i64,
) -> Result<String, V4Error> {
    if token.len() > 512 {
        return Err(V4Error::InvalidRequest);
    }
    let (payload, signature) = token.split_once('.').ok_or(V4Error::InvalidRequest)?;
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| V4Error::InvalidRequest)?;
    let signature = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| V4Error::InvalidRequest)?;
    if !storage
        .crypto()
        .verify_ai_search_cursor(&payload, &signature)
    {
        return Err(V4Error::InvalidRequest);
    }
    let payload: Payload = serde_json::from_slice(&payload).map_err(|_| V4Error::InvalidRequest)?;
    if payload.version != 1
        || payload.account != account.to_string()
        || payload.namespace != namespace
        || payload.instance != instance
        || payload.item != item
        || payload.limit != limit
        || payload.expires_at_ms < now_ms
    {
        return Err(V4Error::InvalidRequest);
    }
    Ok(payload.after)
}
