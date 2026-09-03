//! Installation-managed AI Search credential metadata.

use super::*;
use crate::cloudflare_v4::storage::iso_timestamp;
use axum::extract::{Path, State};
use serde_json::json;
use sha2::{Digest as _, Sha256};

const TOKEN_NAME: &str = "open-compute installation-managed credential";

pub(super) async fn list(
    State(state): State<HttpState>,
    Path(public_account): Path<String>,
    request: Request,
) -> Response {
    let (context, account, api) =
        match authenticated(&state, &request, V4Permission::Read, &public_account) {
            Ok(value) => value,
            Err(response) => return response,
        };
    if api.ai_search().is_none() {
        return error_response(V4Error::Unavailable, context.request_id());
    }
    let query = match query(&request, &["page", "per_page", "search"]) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let (page_number, per_page) = match page(&query, 20, 1, 100) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    if page_number > 100 {
        return error_response(V4Error::InvalidRequest, context.request_id());
    }
    let search = query.get("search").map(|value| value.to_lowercase());
    if search
        .as_ref()
        .is_some_and(|value| value.chars().count() > 256)
    {
        return error_response(V4Error::InvalidRequest, context.request_id());
    }
    let included = search
        .as_ref()
        .is_none_or(|value| TOKEN_NAME.contains(value.as_str()));
    let total = usize::from(included);
    let (start, end) = match page_bounds(page_number, per_page, total) {
        Ok(value) => value,
        Err(error) => return error_response(error, context.request_id()),
    };
    let result = if start < end {
        let id = stable_token_id(&account.to_string());
        let created_at = match iso_timestamp(api.storage().identity().created_at_ms) {
            Ok(value) => value,
            Err(error) => return error_response(error, context.request_id()),
        };
        vec![json!({
            "id": id,
            "cf_api_id": id,
            "created_at": created_at,
            "modified_at": created_at,
            "name": TOKEN_NAME,
            "enabled": true,
            "legacy": false,
        })]
    } else {
        Vec::new()
    };
    result_info_response(
        context,
        result,
        json!({
            "page": page_number,
            "per_page": per_page,
            "count": end - start,
            "total_count": total,
        }),
    )
}

fn stable_token_id(account: &str) -> String {
    let digest = Sha256::digest(format!("open-compute:ai-search-token:{account}"));
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    uuid::Uuid::from_bytes(bytes).to_string()
}

fn page_bounds(page: u64, per_page: u32, total: usize) -> Result<(usize, usize), V4Error> {
    let start = page
        .checked_sub(1)
        .and_then(|value| value.checked_mul(u64::from(per_page)))
        .and_then(|value| usize::try_from(value).ok())
        .ok_or(V4Error::InvalidRequest)?;
    Ok((
        start.min(total),
        start
            .saturating_add(usize::try_from(per_page).map_err(|_| V4Error::InvalidRequest)?)
            .min(total),
    ))
}
