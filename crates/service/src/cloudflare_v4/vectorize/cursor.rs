//! Bound opaque cursor for Vectorize list operations.

use crate::cloudflare_v4::V4Error;
use base64::Engine as _;
use open_compute_core::AccountId;
use open_compute_storage::PlatformStorage;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Payload {
    version: u8,
    account: String,
    index: String,
    count: usize,
    after: String,
    expires_at_ms: i64,
}

pub(super) fn seal(
    storage: &PlatformStorage,
    account: AccountId,
    index: &str,
    count: usize,
    after: &str,
    expires_at_ms: i64,
) -> Result<String, V4Error> {
    let payload = serde_json::to_vec(&Payload {
        version: 1,
        account: account.to_string(),
        index: index.to_owned(),
        count,
        after: after.to_owned(),
        expires_at_ms,
    })
    .map_err(|_| V4Error::Internal)?;
    let signature = storage.crypto().sign_vectorize_cursor(&payload);
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
    index: &str,
    count: usize,
    now_ms: i64,
) -> Result<String, V4Error> {
    let (payload, signature) = token.split_once('.').ok_or(V4Error::InvalidRequest)?;
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| V4Error::InvalidRequest)?;
    let signature = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| V4Error::InvalidRequest)?;
    if !storage
        .crypto()
        .verify_vectorize_cursor(&payload, &signature)
    {
        return Err(V4Error::InvalidRequest);
    }
    let payload: Payload = serde_json::from_slice(&payload).map_err(|_| V4Error::InvalidRequest)?;
    if payload.version != 1
        || payload.account != account.to_string()
        || payload.index != index
        || payload.count != count
        || payload.expires_at_ms < now_ms
    {
        return Err(V4Error::InvalidRequest);
    }
    Ok(payload.after)
}
